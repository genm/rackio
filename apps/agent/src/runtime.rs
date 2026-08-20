#[cfg(unix)]
use std::time::Instant;
use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};

use anyhow::{Context, anyhow};
use directories::ProjectDirs;
use rackio_core::{
    HealthSnapshot, HistoryResolution, MetricCapability, MetricStore, NodeInfo, NodeState,
    ProtocolVersion, SystemCollector,
};
use rackio_iroh::{
    EndpointConfig, NodeRuntime, PairingManager, PairingMdnsState, PeerRegistry, RemoteServer,
    load_or_create_secret_key,
};
use serde::{Deserialize, Serialize};
use tokio::{
    io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader},
    sync::{RwLock, watch},
};
use tracing_subscriber::{EnvFilter, layer::SubscriberExt as _, util::SubscriberInitExt as _};
use uuid::Uuid;

use crate::remote::{RemoteFleet, RemoteHistoryResolution, RemoteMachineSnapshot};

// The Windows local IPC listener has its own connect loop, so these bound the
// Unix accept loop only.
#[cfg(unix)]
const ACCEPT_RETRY_DELAY: Duration = Duration::from_millis(100);
#[cfg(unix)]
const ACCEPT_FAILURE_GRACE: Duration = Duration::from_secs(30);
const SHUTDOWN_FLUSH_TIMEOUT: Duration = Duration::from_secs(5);
const LOCAL_REQUEST_TIMEOUT: Duration = Duration::from_secs(5);
/// The largest `LocalCommand` is a pairing import, whose bundle the desktop
/// already caps at 16 KiB. 64 KiB leaves ample headroom for every command while
/// keeping a single request bounded.
const MAX_LOCAL_REQUEST_BYTES: u64 = 64 * 1024;
/// How far back a restart reads to refill the local trend. Generous enough to
/// cover a full `TrendWindow` at the two-second cadence; the window itself
/// caps how many of those samples are kept.
const LOCAL_TREND_SEED_MS: i64 = 15 * 60 * 1_000;

/// Resolve on an operator-initiated stop. `systemctl stop`, container runtimes
/// and package upgrades all send SIGTERM, so waiting only on Ctrl-C would skip
/// the shutdown path on every real service stop.
async fn shutdown_signal() -> anyhow::Result<()> {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{SignalKind, signal};
        let mut terminate = signal(SignalKind::terminate())?;
        tokio::select! {
            result = tokio::signal::ctrl_c() => result.map_err(anyhow::Error::from),
            _ = terminate.recv() => Ok(()),
        }
    }
    #[cfg(not(unix))]
    {
        tokio::signal::ctrl_c().await.map_err(anyhow::Error::from)
    }
}

#[derive(Debug, Clone)]
pub struct AppPaths {
    pub config: PathBuf,
    pub data: PathBuf,
    pub state: PathBuf,
    pub log: PathBuf,
    #[cfg(unix)]
    pub local_socket: PathBuf,
}

pub fn app_paths() -> anyhow::Result<AppPaths> {
    let config_override = std::env::var_os("RACKIO_CONFIG_DIR").map(PathBuf::from);
    let data_override = std::env::var_os("RACKIO_DATA_DIR").map(PathBuf::from);
    let state_override = std::env::var_os("RACKIO_STATE_DIR").map(PathBuf::from);
    // Service accounts may intentionally provide every owned path while OS
    // user-profile directories are unavailable. Only require ProjectDirs for
    // values that actually need a platform default.
    let dirs = if config_override.is_none() || data_override.is_none() || state_override.is_none() {
        Some(
            ProjectDirs::from("dev", "rackio", "rackio")
                .ok_or_else(|| anyhow!("OS application directories are unavailable"))?,
        )
    } else {
        None
    };
    let config_dir = match config_override {
        Some(path) => path,
        None => dirs
            .as_ref()
            .ok_or_else(|| anyhow!("OS config directory is unavailable"))?
            .config_dir()
            .to_path_buf(),
    };
    let data_dir = match data_override {
        Some(path) => path,
        None => dirs
            .as_ref()
            .ok_or_else(|| anyhow!("OS data directory is unavailable"))?
            .data_local_dir()
            .to_path_buf(),
    };
    let state_dir = if let Some(path) = state_override {
        path
    } else {
        let dirs = dirs
            .as_ref()
            .ok_or_else(|| anyhow!("OS state directory is unavailable"))?;
        dirs.state_dir()
            .unwrap_or_else(|| dirs.data_local_dir())
            .to_path_buf()
    };
    #[cfg(unix)]
    let local_socket = std::env::var_os("RACKIO_SOCKET")
        .map_or_else(|| state_dir.join("agent.sock"), PathBuf::from);
    let log_dir =
        std::env::var_os("RACKIO_LOG_DIR").map_or_else(|| state_dir.join("logs"), PathBuf::from);
    Ok(AppPaths {
        config: config_dir,
        data: data_dir,
        state: state_dir,
        log: log_dir,
        #[cfg(unix)]
        local_socket,
    })
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct AgentConfig {
    relay_url: Option<String>,
    /// Operator-defined local health thresholds. Empty by default: Rackio does
    /// not invent thresholds for a machine it knows nothing about, and
    /// `docs/operations.md` documents `warning`/`critical` as "a *configured*
    /// local health threshold was crossed".
    #[serde(default)]
    alerts: Vec<rackio_core::AlertRule>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "command", rename_all = "snake_case")]
pub enum LocalCommand {
    Status,
    FleetSnapshot,
    PairingCreate,
    PairingImport {
        bundle: String,
    },
    QueryHistory {
        endpoint_id: String,
        from_ms: i64,
        to_ms: i64,
        resolution: RemoteHistoryResolution,
    },
    PeerList,
    PeerRevoke {
        endpoint_id: String,
    },
    RelaySet {
        relay_url: Option<String>,
    },
    Doctor,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LocalResponse {
    pub ok: bool,
    pub data: Option<serde_json::Value>,
    pub error: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub warnings: Vec<String>,
}

impl LocalResponse {
    fn success<T: Serialize>(data: T) -> Self {
        match serde_json::to_value(data) {
            Ok(data) => Self {
                ok: true,
                data: Some(data),
                error: None,
                warnings: Vec::new(),
            },
            Err(error) => Self::failure(error),
        }
    }

    fn success_with_warning<T: Serialize>(data: T, warning: impl std::fmt::Display) -> Self {
        match serde_json::to_value(data) {
            Ok(data) => Self {
                ok: true,
                data: Some(data),
                error: None,
                warnings: vec![warning.to_string()],
            },
            Err(error) => Self::failure(error),
        }
    }

    fn failure(error: impl std::fmt::Display) -> Self {
        Self {
            ok: false,
            data: None,
            error: Some(error.to_string()),
            warnings: Vec::new(),
        }
    }
}

#[derive(Debug, Serialize)]
struct StatusPayload {
    node: NodeInfo,
    endpoint_id: String,
    direct_addresses: Vec<String>,
    relay_url: Option<String>,
    latest: Option<rackio_core::MetricSample>,
    health: HealthSnapshot,
    /// The local machine's live trend, in the same shape a remote snapshot
    /// carries, so viewers render every machine from one contract.
    trend: rackio_core::TrendWindow,
}

#[derive(Debug, Serialize)]
struct FleetPayload {
    local: StatusPayload,
    remotes: Vec<RemoteMachineSnapshot>,
}

pub async fn run_daemon(paths: AppPaths) -> anyhow::Result<()> {
    create_directories(&paths)?;
    init_logging(&paths)?;

    let config = load_config(&paths)?;
    let secret = load_or_create_secret_key(&paths.data.join("identity.key"))?;
    let endpoint = rackio_iroh::bind_endpoint(
        secret,
        &EndpointConfig {
            relay_urls: config.relay_url.clone().into_iter().collect(),
        },
    )
    .await?;
    let node_id = load_or_create_node_id(&paths.data.join("node-id"))?;
    // Probe the host before advertising what this machine can collect, and
    // hand the same collector to the sampler so the advertised capabilities
    // and the published samples come from one source.
    let collector = SystemCollector::new();
    let info = node_info(node_id, collector.capabilities());
    let (latest_tx, latest_rx) = watch::channel(None);
    let store = MetricStore::open(paths.data.join("metrics.sqlite3"))?;
    // Resume this machine's own trend from storage. Without it a restart shows
    // a blank chart for the local machine while every remote keeps the window
    // its registry persisted. A failed read degrades to an empty window — the
    // chart then says it is collecting, which is true.
    let seed_now_ms = rackio_core::Clock::new().now_ms();
    let trend = match store.query(
        seed_now_ms.saturating_sub(LOCAL_TREND_SEED_MS),
        seed_now_ms,
        HistoryResolution::Raw,
    ) {
        Ok(samples) => rackio_core::TrendWindow::from_samples(&samples),
        Err(error) => {
            tracing::warn!(error = %error, "local trend could not be resumed from storage");
            rackio_core::TrendWindow::default()
        }
    };
    let runtime = Arc::new(NodeRuntime {
        info,
        health: RwLock::new(healthy()),
        latest: latest_rx,
        trend: RwLock::new(trend),
        store: tokio::sync::Mutex::new(store),
        pairing: std::sync::Mutex::new(PairingManager::default()),
        pairing_mdns: Arc::new(PairingMdnsState::default()),
        peers: PeerRegistry::load(paths.data.join("peers.json"))?,
        active_connections: std::sync::Mutex::new(std::collections::BTreeMap::new()),
    });
    let server = RemoteServer::new(endpoint.clone(), Arc::clone(&runtime));
    let remote_fleet =
        RemoteFleet::load(endpoint.clone(), paths.data.join("monitored-machines.json"))?;
    remote_fleet.start()?;

    tracing::info!(
        endpoint_id = %endpoint.id(),
        relay_mode = if config.relay_url.is_some() { "self_hosted" } else { "direct_only" },
        "agent started"
    );

    let (shutdown_tx, shutdown_rx) = watch::channel(false);
    let mut sampler = tokio::spawn(sample_loop(
        Arc::clone(&runtime),
        collector,
        config.alerts.clone(),
        latest_tx,
        shutdown_rx,
    ));
    let mut remote = tokio::spawn(server.run());
    let mut local = tokio::spawn(run_local_server(
        paths.clone(),
        endpoint.clone(),
        runtime,
        remote_fleet,
    ));

    // Polling a `JoinHandle` that already resolved panics, so remember whether
    // the sampler is the branch that ended the select.
    let mut sampler_finished = false;
    let result = tokio::select! {
        signal = shutdown_signal() => signal,
        stopped = &mut sampler => {
            sampler_finished = true;
            Err(anyhow!("metric sampler stopped unexpectedly: {stopped:?}"))
        }
        stopped = &mut remote => Err(anyhow!("remote listener stopped unexpectedly: {stopped:?}")),
        stopped = &mut local => match stopped {
            Ok(Ok(())) => Err(anyhow!("local IPC listener stopped unexpectedly")),
            Ok(Err(error)) => Err(error.context("local IPC listener failed")),
            Err(error) => Err(error.into()),
        },
    };
    // Let the sampler commit its buffered batch before the process exits.
    // Aborting it outright would discard up to ten seconds of history on every
    // service stop, restart and upgrade.
    let _ = shutdown_tx.send(true);
    if !sampler_finished {
        match tokio::time::timeout(SHUTDOWN_FLUSH_TIMEOUT, &mut sampler).await {
            Ok(Ok(())) => {}
            Ok(Err(error)) => tracing::warn!(error = %error, "metric sampler ended abnormally"),
            Err(_) => {
                tracing::warn!("metric sampler did not flush within the shutdown timeout");
                sampler.abort();
            }
        }
    }
    endpoint.close().await;
    remote.abort();
    local.abort();
    if let Err(error) = &result {
        tracing::error!(error = %error, "agent stopped because a required task failed");
    } else {
        tracing::info!("agent stopped");
    }
    result
}

/// Reflect the collector's own errors in the published health snapshot, and
/// clear them again once every source reads. `state` returns to `Healthy` only
/// when no other subsystem is degraded.
async fn apply_collector_health(runtime: &NodeRuntime, errors: &[rackio_core::CollectorError]) {
    let degraded = !errors.is_empty();
    let mut health = runtime.health.write().await;
    if health.collector_degraded == degraded {
        return;
    }
    health.collector_degraded = degraded;
    if degraded {
        health.state = NodeState::Degraded;
        if !health
            .details
            .iter()
            .any(|detail| detail == "collector_degraded")
        {
            health.details.push(String::from("collector_degraded"));
        }
        tracing::warn!(
            sources = ?errors.iter().map(|error| error.source.as_str()).collect::<Vec<_>>(),
            "one or more metric sources are unreadable on this host"
        );
    } else {
        health
            .details
            .retain(|detail| detail != "collector_degraded");
        if !health.storage_degraded && !health.remote_listener_degraded {
            health.state = NodeState::Healthy;
        }
    }
}

/// Publish an operator-configured threshold breach as the machine's state.
///
/// Degradation still wins: a machine whose storage or collector is broken is
/// reported as `Degraded` rather than as a threshold warning, because the
/// underlying data is no longer trustworthy.
async fn apply_alert_health(runtime: &NodeRuntime, severity: Option<NodeState>) {
    let mut health = runtime.health.write().await;
    if health.collector_degraded || health.storage_degraded || health.remote_listener_degraded {
        return;
    }
    health.state = severity.unwrap_or(NodeState::Healthy);
}

/// Commit whatever is buffered so a graceful stop does not discard history.
/// A failure here is reported, never presented as a successful flush.
async fn flush_pending(runtime: &NodeRuntime, pending: &mut Vec<rackio_core::MetricSample>) {
    if pending.is_empty() {
        return;
    }
    match runtime.store.lock().await.insert_batch(pending) {
        Ok(()) => {
            tracing::info!(
                samples = pending.len(),
                "flushed buffered samples on shutdown"
            );
            pending.clear();
        }
        Err(error) => tracing::error!(
            error = %error,
            samples = pending.len(),
            "failed to flush buffered samples on shutdown; this history is lost"
        ),
    }
}

async fn sample_loop(
    runtime: Arc<NodeRuntime>,
    mut collector: SystemCollector,
    alert_rules: Vec<rackio_core::AlertRule>,
    latest: watch::Sender<Option<rackio_core::MetricSample>>,
    mut shutdown: watch::Receiver<bool>,
) {
    let clock = rackio_core::Clock::new();
    let mut alerts = rackio_core::AlertEvaluator::default();
    tokio::time::sleep(sysinfo::MINIMUM_CPU_UPDATE_INTERVAL).await;
    let mut ticker = tokio::time::interval(Duration::from_secs(2));
    let mut pending = Vec::with_capacity(5);
    let mut prune_counter = 0_u16;
    loop {
        tokio::select! {
            _ = ticker.tick() => {}
            changed = shutdown.changed() => {
                // A closed sender means the daemon is going away too.
                if changed.is_err() || *shutdown.borrow() {
                    flush_pending(&runtime, &mut pending).await;
                    return;
                }
                continue;
            }
        }
        let sample = collector.sample();
        // A source the collector could not read must be visible as a degraded
        // collector, not hidden behind an otherwise healthy snapshot.
        apply_collector_health(&runtime, &sample.errors).await;
        for signal in alerts.evaluate(&sample, &alert_rules) {
            tracing::info!(
                rule = %signal.rule_id,
                active = signal.active,
                severity = ?signal.severity,
                "local health threshold transition"
            );
        }
        apply_alert_health(&runtime, alerts.worst_active_severity(&alert_rules)).await;
        let _ = latest.send(Some(sample.clone()));
        runtime
            .trend
            .write()
            .await
            .push(rackio_core::TrendSample::from(&sample));
        pending.push(sample);
        if pending.len() >= 5 {
            let store_result = runtime.store.lock().await.insert_batch(&pending);
            match store_result {
                Ok(()) => {
                    pending.clear();
                    let mut health = runtime.health.write().await;
                    if health.storage_degraded {
                        health.storage_degraded = false;
                        health.details.retain(|detail| detail != "storage_degraded");
                        if !health.collector_degraded && !health.remote_listener_degraded {
                            health.state = NodeState::Healthy;
                        }
                    }
                }
                Err(error) => {
                    // A failed disk must not turn the live sampler into an unbounded queue.
                    pending.clear();
                    let mut health = runtime.health.write().await;
                    health.storage_degraded = true;
                    health.state = NodeState::Degraded;
                    if !health
                        .details
                        .iter()
                        .any(|detail| detail == "storage_degraded")
                    {
                        health.details.push(String::from("storage_degraded"));
                    }
                    tracing::error!(error = %error, "metric storage is degraded; live sampling continues");
                }
            }
        }
        prune_counter = prune_counter.saturating_add(1);
        if prune_counter >= 300 {
            prune_counter = 0;
            if let Err(error) = runtime
                .store
                .lock()
                .await
                // Monotonic: a forward clock step must not delete the whole
                // history, and a backward one must not stop pruning.
                .prune(clock.now_ms())
            {
                let mut health = runtime.health.write().await;
                health.storage_degraded = true;
                health.state = NodeState::Degraded;
                if !health
                    .details
                    .iter()
                    .any(|detail| detail == "storage_degraded")
                {
                    health.details.push(String::from("storage_degraded"));
                }
                tracing::warn!(error = %error, "metric history pruning failed");
            }
        }
    }
}

#[cfg(unix)]
async fn run_local_server(
    paths: AppPaths,
    endpoint: iroh::Endpoint,
    runtime: Arc<NodeRuntime>,
    remote_fleet: RemoteFleet,
) -> anyhow::Result<()> {
    use std::os::unix::fs::{FileTypeExt as _, PermissionsExt as _};
    use tokio::net::UnixListener;

    match fs::symlink_metadata(&paths.local_socket) {
        Ok(metadata) if metadata.file_type().is_socket() => fs::remove_file(&paths.local_socket)?,
        Ok(_) => anyhow::bail!(
            "refusing to replace non-socket path {}",
            paths.local_socket.display()
        ),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.into()),
    }
    let listener = UnixListener::bind(&paths.local_socket)?;
    let shared = std::env::var_os("RACKIO_SHARED_SOCKET").is_some();
    fs::set_permissions(
        &paths.local_socket,
        fs::Permissions::from_mode(if shared { 0o660 } else { 0o600 }),
    )?;
    // A per-connection accept failure (an aborted client, a momentary fd
    // shortage) must not stop metric collection and remote monitoring. A
    // listener that never recovers still has to fail closed, so failures are
    // only tolerated while some accept succeeds within the grace window.
    let mut failing_since: Option<Instant> = None;
    loop {
        let stream = match listener.accept().await {
            Ok((stream, _)) => {
                failing_since = None;
                stream
            }
            Err(error) => {
                let since = *failing_since.get_or_insert_with(Instant::now);
                if since.elapsed() >= ACCEPT_FAILURE_GRACE {
                    return Err(anyhow::Error::from(error).context(format!(
                        "local IPC listener failed continuously for {} seconds",
                        ACCEPT_FAILURE_GRACE.as_secs()
                    )));
                }
                tracing::warn!(error = %error, "local IPC accept failed; retrying");
                tokio::time::sleep(ACCEPT_RETRY_DELAY).await;
                continue;
            }
        };
        if let Err(error) = stream.peer_cred() {
            tracing::warn!(error = %error, "local IPC caller credentials unavailable");
            continue;
        }
        let paths = paths.clone();
        let endpoint = endpoint.clone();
        let runtime = Arc::clone(&runtime);
        let remote_fleet = remote_fleet.clone();
        tokio::spawn(serve_local_stream(
            stream,
            paths,
            endpoint,
            runtime,
            remote_fleet,
        ));
    }
}

#[cfg(windows)]
async fn run_local_server(
    paths: AppPaths,
    endpoint: iroh::Endpoint,
    runtime: Arc<NodeRuntime>,
    remote_fleet: RemoteFleet,
) -> anyhow::Result<()> {
    use rackio_windows_ipc::{PipeSecurity, VIEWER_GROUP_NAME, configured_pipe_name};
    use tokio::net::windows::named_pipe::ServerOptions;

    let security = PipeSecurity::for_local_group(VIEWER_GROUP_NAME)
        .context("Rackio viewer-group pipe ACL initialization failed")?;
    let pipe_name = configured_pipe_name()?;
    let mut first_options = ServerOptions::new();
    first_options
        .first_pipe_instance(true)
        .reject_remote_clients(true);
    let mut server = security.create_server(&first_options, &pipe_name)?;
    loop {
        server.connect().await?;
        let mut next_options = ServerOptions::new();
        next_options.reject_remote_clients(true);
        let next = security.create_server(&next_options, &pipe_name)?;
        let mut first_byte = [0_u8; 1];
        match tokio::time::timeout(Duration::from_secs(2), server.read_exact(&mut first_byte)).await
        {
            Ok(Ok(_)) => {}
            Ok(Err(error)) => {
                tracing::warn!(error = %error, "local named-pipe caller closed before authorization");
                server = next;
                continue;
            }
            Err(error) => {
                tracing::warn!(error = %error, "local named-pipe caller timed out before authorization");
                server = next;
                continue;
            }
        }
        if let Err(error) = security.verify_client_after_read(&server) {
            tracing::warn!(error = %error, "rejected unauthorized local named-pipe caller");
            server = next;
            continue;
        }
        tokio::spawn(serve_local_stream_prefixed(
            server,
            first_byte.to_vec(),
            paths.clone(),
            endpoint.clone(),
            Arc::clone(&runtime),
            remote_fleet.clone(),
        ));
        server = next;
    }
}

#[cfg(not(any(unix, windows)))]
async fn run_local_server(
    _paths: AppPaths,
    _endpoint: iroh::Endpoint,
    _runtime: Arc<NodeRuntime>,
    _remote_fleet: RemoteFleet,
) -> anyhow::Result<()> {
    anyhow::bail!("local IPC is unsupported on this platform")
}

/// Read one bounded, timely `LocalCommand`, or the failure to report instead.
///
/// `reader` must already be limited to [`MAX_LOCAL_REQUEST_BYTES`]; hitting
/// that limit shows up here as a line with no terminating newline, which is
/// rejected rather than parsed as a truncated command.
async fn read_local_request<R>(reader: &mut R) -> Result<LocalCommand, LocalResponse>
where
    R: tokio::io::AsyncBufRead + Unpin,
{
    let mut line = String::new();
    match tokio::time::timeout(
        LOCAL_REQUEST_TIMEOUT,
        tokio::io::AsyncBufReadExt::read_line(reader, &mut line),
    )
    .await
    {
        Err(_) => Err(LocalResponse::failure(
            "local request timed out before a complete command",
        )),
        Ok(Err(error)) => Err(LocalResponse::failure(error)),
        Ok(Ok(0)) => Err(LocalResponse::failure("empty local request")),
        Ok(Ok(_)) if !line.ends_with('\n') => Err(LocalResponse::failure(format!(
            "local request exceeded {MAX_LOCAL_REQUEST_BYTES} bytes without a newline"
        ))),
        Ok(Ok(_)) => {
            serde_json::from_str::<LocalCommand>(line.trim_end()).map_err(LocalResponse::failure)
        }
    }
}

// Only the Unix listener hands a bare stream over; the Windows listener has
// already consumed its first byte and uses the prefixed form.
#[cfg(unix)]
async fn serve_local_stream<S>(
    stream: S,
    paths: AppPaths,
    endpoint: iroh::Endpoint,
    runtime: Arc<NodeRuntime>,
    remote_fleet: RemoteFleet,
) where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + Send + 'static,
{
    serve_local_stream_prefixed(stream, Vec::new(), paths, endpoint, runtime, remote_fleet).await;
}

async fn serve_local_stream_prefixed<S>(
    stream: S,
    prefix: Vec<u8>,
    paths: AppPaths,
    endpoint: iroh::Endpoint,
    runtime: Arc<NodeRuntime>,
    remote_fleet: RemoteFleet,
) where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + Send + 'static,
{
    let (read, mut write) = tokio::io::split(stream);
    // Bound the request itself. On the shared socket every viewer-group member
    // can connect, and an unbounded `next_line` lets one of them park a task
    // forever or stream a line until the daemon runs out of memory — stopping
    // metric collection for everyone.
    let mut reader = BufReader::new(
        std::io::Cursor::new(prefix)
            .chain(read)
            .take(MAX_LOCAL_REQUEST_BYTES),
    );
    let response = match read_local_request(&mut reader).await {
        Ok(command) => handle_local(&paths, &endpoint, &runtime, &remote_fleet, command).await,
        Err(response) => response,
    };
    if let Ok(mut bytes) = serde_json::to_vec(&response) {
        bytes.push(b'\n');
        let _ = write.write_all(&bytes).await;
    }
}

async fn handle_local(
    paths: &AppPaths,
    endpoint: &iroh::Endpoint,
    runtime: &Arc<NodeRuntime>,
    remote_fleet: &RemoteFleet,
    command: LocalCommand,
) -> LocalResponse {
    match command {
        LocalCommand::Status | LocalCommand::Doctor => {
            let relay_url = match load_config(paths) {
                Ok(config) => config.relay_url,
                Err(error) => return LocalResponse::failure(error),
            };
            let latest = runtime.latest.borrow().clone();
            let health = runtime.health.read().await.clone();
            let trend = runtime.trend.read().await.clone();
            LocalResponse::success(StatusPayload {
                node: runtime.info.clone(),
                endpoint_id: endpoint.id().to_string(),
                direct_addresses: endpoint
                    .addr()
                    .ip_addrs()
                    .map(std::string::ToString::to_string)
                    .collect(),
                relay_url,
                latest,
                health,
                trend,
            })
        }
        LocalCommand::FleetSnapshot => {
            let relay_url = match load_config(paths) {
                Ok(config) => config.relay_url,
                Err(error) => return LocalResponse::failure(error),
            };
            let latest = runtime.latest.borrow().clone();
            let health = runtime.health.read().await.clone();
            let trend = runtime.trend.read().await.clone();
            let local = StatusPayload {
                node: runtime.info.clone(),
                endpoint_id: endpoint.id().to_string(),
                direct_addresses: endpoint
                    .addr()
                    .ip_addrs()
                    .map(std::string::ToString::to_string)
                    .collect(),
                relay_url,
                latest,
                health,
                trend,
            };
            LocalResponse::success(FleetPayload {
                local,
                remotes: remote_fleet.snapshots().await,
            })
        }
        LocalCommand::PairingCreate => create_pairing_bundle(paths, endpoint, runtime).await,
        LocalCommand::PairingImport { bundle } => {
            match remote_fleet.import_pairing(&bundle).await {
                Ok(machine) => LocalResponse::success(machine),
                Err(error) => LocalResponse::failure(error),
            }
        }
        LocalCommand::QueryHistory {
            endpoint_id,
            from_ms,
            to_ms,
            resolution,
        } => {
            if endpoint_id == endpoint.id().to_string() {
                handle_local_history(runtime, from_ms, to_ms, resolution).await
            } else {
                handle_remote_history(remote_fleet, &endpoint_id, from_ms, to_ms, resolution).await
            }
        }
        LocalCommand::PeerList => match runtime.peers.list() {
            Ok(peers) => LocalResponse::success(peers),
            Err(error) => LocalResponse::failure(error),
        },
        LocalCommand::PeerRevoke { endpoint_id } => match runtime.revoke_peer(&endpoint_id) {
            Ok(removed) => LocalResponse::success(serde_json::json!({ "revoked": removed })),
            Err(error) => LocalResponse::failure(error),
        },
        LocalCommand::RelaySet { relay_url } => {
            if let Err(error) = validate_relay_url(relay_url.as_deref()) {
                return LocalResponse::failure(error);
            }
            // Preserve every other configured value. Rebuilding the struct
            // from one field would silently discard the operator's alert
            // thresholds on the next relay change.
            let mut config = match load_config(paths) {
                Ok(config) => config,
                Err(error) => return LocalResponse::failure(error),
            };
            config.relay_url = relay_url;
            match save_config(paths, &config) {
                Ok(()) => LocalResponse::success(serde_json::json!({
                    "saved": true,
                    "restart_required": true
                })),
                Err(error) => LocalResponse::failure(error),
            }
        }
    }
}

async fn create_pairing_bundle(
    paths: &AppPaths,
    endpoint: &iroh::Endpoint,
    runtime: &Arc<NodeRuntime>,
) -> LocalResponse {
    let addresses = endpoint.addr().ip_addrs().copied().collect();
    let relay_urls = match load_config(paths) {
        Ok(config) => config.relay_url.into_iter().collect(),
        Err(error) => return LocalResponse::failure(error),
    };
    let encoded = match runtime.pairing.lock() {
        Ok(mut pairing) => {
            let bundle = pairing.open(runtime.info.node_id, endpoint.id(), addresses, relay_urls);
            match bundle.encode() {
                Ok(encoded) => encoded,
                Err(error) => return LocalResponse::failure(error),
            }
        }
        Err(_) => return LocalResponse::failure("pairing state lock is unavailable"),
    };
    match runtime.pairing_mdns.open(endpoint).await {
        Ok(generation) => {
            let pairing_mdns = Arc::clone(&runtime.pairing_mdns);
            tokio::spawn(async move {
                tokio::time::sleep(Duration::from_mins(5)).await;
                if pairing_mdns.close_if_generation(generation).await {
                    tracing::info!("pairing mDNS advertisement expired");
                }
            });
            LocalResponse::success(encoded)
        }
        Err(error) => {
            tracing::warn!(error = %error, "pairing window opened without LAN advertisement");
            LocalResponse::success_with_warning(encoded, error)
        }
    }
}

async fn handle_remote_history(
    remote_fleet: &RemoteFleet,
    endpoint_id: &str,
    from_ms: i64,
    to_ms: i64,
    resolution: RemoteHistoryResolution,
) -> LocalResponse {
    match remote_fleet
        .query_history(endpoint_id, from_ms, to_ms, resolution)
        .await
    {
        Ok(samples) => LocalResponse::success(samples),
        Err(error) => LocalResponse::failure(error),
    }
}

async fn handle_local_history(
    runtime: &Arc<NodeRuntime>,
    from_ms: i64,
    to_ms: i64,
    resolution: RemoteHistoryResolution,
) -> LocalResponse {
    const MAX_HISTORY_RANGE_MS: i64 = 7 * 24 * 60 * 60 * 1_000;
    if from_ms > to_ms || to_ms.saturating_sub(from_ms) > MAX_HISTORY_RANGE_MS {
        return LocalResponse::failure("history range is invalid or exceeds seven days");
    }
    let resolution = match resolution {
        RemoteHistoryResolution::Raw => HistoryResolution::Raw,
        RemoteHistoryResolution::Minute => HistoryResolution::Minute,
    };
    match runtime.store.lock().await.query(from_ms, to_ms, resolution) {
        Ok(samples) => LocalResponse::success(samples),
        Err(error) => LocalResponse::failure(error),
    }
}

#[cfg(unix)]
pub async fn request_local(
    paths: &AppPaths,
    command: LocalCommand,
) -> anyhow::Result<LocalResponse> {
    use tokio::net::UnixStream;

    let candidates = local_socket_candidates(paths, std::env::var_os("RACKIO_SOCKET").is_some());
    let mut connected = None;
    let mut errors = Vec::new();
    for candidate in candidates {
        match UnixStream::connect(&candidate).await {
            Ok(stream) => {
                connected = Some(stream);
                break;
            }
            Err(error) => errors.push(format!("{}: {error}", candidate.display())),
        }
    }
    let stream = connected.ok_or_else(|| {
        anyhow!(
            "daemon is not reachable at any local socket ({})",
            errors.join("; ")
        )
    })?;
    let (read, mut write) = stream.into_split();
    let mut request = serde_json::to_vec(&command)?;
    request.push(b'\n');
    write.write_all(&request).await?;
    let mut lines = BufReader::new(read).lines();
    let line = lines
        .next_line()
        .await?
        .ok_or_else(|| anyhow!("daemon closed the local connection without a response"))?;
    Ok(serde_json::from_str(&line)?)
}

#[cfg(unix)]
fn local_socket_candidates(paths: &AppPaths, explicit: bool) -> Vec<PathBuf> {
    if explicit {
        return vec![paths.local_socket.clone()];
    }
    let mut candidates = Vec::new();
    // Installed services use a machine-wide socket; developer daemons retain
    // their per-user socket as a fallback.
    #[cfg(target_os = "linux")]
    candidates.push(PathBuf::from("/run/rackio/agent.sock"));
    #[cfg(target_os = "macos")]
    candidates.push(PathBuf::from(
        "/Library/Application Support/Rackio/run/agent.sock",
    ));
    if !candidates.contains(&paths.local_socket) {
        candidates.push(paths.local_socket.clone());
    }
    candidates
}

#[cfg(windows)]
pub async fn request_local(
    _paths: &AppPaths,
    command: LocalCommand,
) -> anyhow::Result<LocalResponse> {
    let pipe_name = rackio_windows_ipc::configured_pipe_name()?;
    let stream = rackio_windows_ipc::connect_client(&pipe_name).await?;
    let (read, mut write) = tokio::io::split(stream);
    let mut request = serde_json::to_vec(&command)?;
    request.push(b'\n');
    write.write_all(&request).await?;
    let mut lines = BufReader::new(read).lines();
    let line = lines
        .next_line()
        .await?
        .ok_or_else(|| anyhow!("daemon closed the local connection without a response"))?;
    Ok(serde_json::from_str(&line)?)
}

#[cfg(not(any(unix, windows)))]
pub async fn request_local(
    _paths: &AppPaths,
    _command: LocalCommand,
) -> anyhow::Result<LocalResponse> {
    anyhow::bail!("local IPC is unsupported on this platform")
}

/// Build the advertised node information from what this host can actually
/// collect. Declaring a fixed list of `Supported` capabilities made a viewer
/// trust metrics the collector cannot read on a sandboxed or containerised
/// host.
fn node_info(node_id: Uuid, capabilities: Vec<MetricCapability>) -> NodeInfo {
    NodeInfo {
        node_id,
        display_name: sysinfo::System::host_name().unwrap_or_else(|| String::from("Unnamed node")),
        os: std::env::consts::OS.to_owned(),
        architecture: std::env::consts::ARCH.to_owned(),
        agent_version: env!("CARGO_PKG_VERSION").to_owned(),
        protocol: ProtocolVersion::V1,
        capabilities,
    }
}

fn healthy() -> HealthSnapshot {
    HealthSnapshot {
        state: NodeState::Healthy,
        collector_degraded: false,
        storage_degraded: false,
        remote_listener_degraded: false,
        details: Vec::new(),
    }
}

fn create_directories(paths: &AppPaths) -> anyhow::Result<()> {
    for path in [&paths.config, &paths.data, &paths.state, &paths.log] {
        fs::create_dir_all(path)?;
    }
    Ok(())
}

fn config_path(paths: &AppPaths) -> PathBuf {
    paths.config.join("config.json")
}

fn validate_relay_url(relay_url: Option<&str>) -> Result<(), &'static str> {
    if relay_url.is_some_and(|value| value.parse::<iroh::RelayUrl>().is_err()) {
        Err("relay URL is invalid")
    } else {
        Ok(())
    }
}

fn load_config(paths: &AppPaths) -> anyhow::Result<AgentConfig> {
    match fs::read(config_path(paths)) {
        Ok(bytes) => Ok(serde_json::from_slice(&bytes)?),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(AgentConfig::default()),
        Err(error) => Err(error.into()),
    }
}

fn save_config(paths: &AppPaths, config: &AgentConfig) -> anyhow::Result<()> {
    fs::create_dir_all(&paths.config)?;
    let target = config_path(paths);
    let mut file = tempfile::Builder::new()
        .prefix(".config-")
        .tempfile_in(&paths.config)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        file.as_file()
            .set_permissions(fs::Permissions::from_mode(0o600))?;
    }
    file.write_all(&serde_json::to_vec_pretty(config)?)?;
    file.as_file().sync_all()?;
    file.persist(target).map_err(|error| error.error)?;
    Ok(())
}

fn load_or_create_node_id(path: &Path) -> anyhow::Result<Uuid> {
    match fs::read_to_string(path) {
        Ok(value) => Ok(Uuid::parse_str(value.trim())?),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            let node_id = Uuid::new_v4();
            fs::write(path, node_id.to_string())?;
            Ok(node_id)
        }
        Err(error) => Err(error.into()),
    }
}

fn init_logging(paths: &AppPaths) -> anyhow::Result<()> {
    fs::create_dir_all(&paths.log)?;
    let file = tracing_appender::rolling::daily(&paths.log, "agent.jsonl");
    tracing_subscriber::registry()
        .with(EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")))
        .with(tracing_subscriber::fmt::layer().json().with_writer(file))
        .try_init()
        .context("structured logging initialization failed")
}

#[cfg(test)]
mod tests {
    #[cfg(unix)]
    use std::path::PathBuf;

    use rackio_core::{AlertRule, Comparison, NodeState};

    use super::{
        AgentConfig, AppPaths, MAX_LOCAL_REQUEST_BYTES, load_config, read_local_request,
        save_config, validate_relay_url,
    };

    fn test_paths(root: &std::path::Path) -> AppPaths {
        AppPaths {
            config: root.join("config"),
            data: root.join("data"),
            state: root.join("state"),
            log: root.join("log"),
            #[cfg(unix)]
            local_socket: root.join("agent.sock"),
        }
    }

    async fn read_request(payload: &[u8]) -> Result<super::LocalCommand, super::LocalResponse> {
        let mut reader = tokio::io::BufReader::new(tokio::io::AsyncReadExt::take(
            payload,
            MAX_LOCAL_REQUEST_BYTES,
        ));
        read_local_request(&mut reader).await
    }

    #[tokio::test]
    async fn accepts_a_well_formed_local_command() {
        let command = read_request(b"{\"command\":\"status\"}\n")
            .await
            .unwrap_or_else(|response| panic!("{response:?}"));
        assert!(matches!(command, super::LocalCommand::Status));
    }

    #[tokio::test]
    async fn rejects_a_local_request_that_never_terminates() {
        // A viewer-group member on the shared socket must not be able to grow
        // the daemon's memory with one endless line.
        let oversized =
            vec![b'x'; usize::try_from(MAX_LOCAL_REQUEST_BYTES).unwrap_or(usize::MAX) + 1];
        let Err(response) = read_request(&oversized).await else {
            panic!("an unterminated oversized request must be rejected");
        };
        assert!(!response.ok);
        assert!(
            response
                .error
                .unwrap_or_default()
                .contains("without a newline")
        );
    }

    #[tokio::test]
    async fn rejects_an_empty_local_request() {
        let Err(response) = read_request(b"").await else {
            panic!("an empty request must be rejected");
        };
        assert!(!response.ok);
    }

    #[cfg(unix)]
    use super::local_socket_candidates;

    #[test]
    fn relay_url_validation_fails_closed() {
        assert!(validate_relay_url(Some("not a relay URL")).is_err());
        assert!(validate_relay_url(Some("https://relay.example.test")).is_ok());
        assert!(validate_relay_url(None).is_ok());
    }

    #[test]
    fn a_missing_config_is_direct_only_without_invented_alerts() {
        let directory = tempfile::tempdir().unwrap_or_else(|error| panic!("{error}"));
        let config =
            load_config(&test_paths(directory.path())).unwrap_or_else(|error| panic!("{error}"));

        assert!(config.relay_url.is_none());
        assert!(config.alerts.is_empty());
    }

    #[test]
    fn config_round_trips_every_operator_owned_field() {
        let directory = tempfile::tempdir().unwrap_or_else(|error| panic!("{error}"));
        let paths = test_paths(directory.path());
        let expected = AgentConfig {
            relay_url: Some(String::from("https://relay.example.test")),
            alerts: vec![AlertRule {
                id: String::from("cpu-warning"),
                metric: String::from("cpu_percent"),
                comparison: Comparison::GreaterThanOrEqual,
                threshold: 80.0,
                consecutive_samples: 3,
                severity: NodeState::Warning,
            }],
        };

        save_config(&paths, &expected).unwrap_or_else(|error| panic!("{error}"));
        let actual = load_config(&paths).unwrap_or_else(|error| panic!("{error}"));

        assert_eq!(actual.relay_url, expected.relay_url);
        assert_eq!(actual.alerts, expected.alerts);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            let mode = std::fs::metadata(paths.config.join("config.json"))
                .unwrap_or_else(|error| panic!("{error}"))
                .permissions()
                .mode();
            assert_eq!(mode & 0o777, 0o600);
        }
    }

    #[test]
    fn an_invalid_config_fails_closed_instead_of_using_defaults() {
        let directory = tempfile::tempdir().unwrap_or_else(|error| panic!("{error}"));
        let paths = test_paths(directory.path());
        std::fs::create_dir_all(&paths.config).unwrap_or_else(|error| panic!("{error}"));
        std::fs::write(paths.config.join("config.json"), b"not json")
            .unwrap_or_else(|error| panic!("{error}"));

        assert!(load_config(&paths).is_err());
    }

    #[test]
    #[cfg(unix)]
    fn explicit_socket_does_not_fall_back_to_another_daemon() {
        let explicit = tempfile::tempdir()
            .unwrap_or_else(|error| panic!("{error}"))
            .path()
            .join("explicit.sock");
        let paths = AppPaths {
            config: PathBuf::from("/unused/config"),
            data: PathBuf::from("/unused/data"),
            state: PathBuf::from("/unused/state"),
            log: PathBuf::from("/unused/log"),
            local_socket: explicit.clone(),
        };
        let candidates = local_socket_candidates(&paths, true);

        assert_eq!(candidates, vec![explicit]);
    }
}
