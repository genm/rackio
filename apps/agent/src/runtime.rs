use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
    sync::Arc,
    time::{Duration, Instant},
};

use anyhow::{Context, anyhow};
use chrono::Utc;
use directories::ProjectDirs;
use rackio_core::{
    CapabilityState, HealthSnapshot, HistoryResolution, MetricCapability, MetricStore, NodeInfo,
    NodeState, ProtocolVersion, SystemCollector,
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

const ACCEPT_RETRY_DELAY: Duration = Duration::from_millis(100);
const ACCEPT_FAILURE_GRACE: Duration = Duration::from_secs(30);
const SHUTDOWN_FLUSH_TIMEOUT: Duration = Duration::from_secs(5);

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
    let info = node_info(node_id);
    let (latest_tx, latest_rx) = watch::channel(None);
    let runtime = Arc::new(NodeRuntime {
        info,
        health: RwLock::new(healthy()),
        latest: latest_rx,
        store: tokio::sync::Mutex::new(MetricStore::open(paths.data.join("metrics.sqlite3"))?),
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
    let mut sampler = tokio::spawn(sample_loop(Arc::clone(&runtime), latest_tx, shutdown_rx));
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
    latest: watch::Sender<Option<rackio_core::MetricSample>>,
    mut shutdown: watch::Receiver<bool>,
) {
    let mut collector = SystemCollector::new();
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
        let _ = latest.send(Some(sample.clone()));
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
                .prune(Utc::now().timestamp_millis())
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
    let mut lines = BufReader::new(std::io::Cursor::new(prefix).chain(read)).lines();
    let response = match lines.next_line().await {
        Ok(Some(line)) => match serde_json::from_str::<LocalCommand>(&line) {
            Ok(command) => handle_local(&paths, &endpoint, &runtime, &remote_fleet, command).await,
            Err(error) => LocalResponse::failure(error),
        },
        Ok(None) => LocalResponse::failure("empty local request"),
        Err(error) => LocalResponse::failure(error),
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
            })
        }
        LocalCommand::FleetSnapshot => {
            let relay_url = match load_config(paths) {
                Ok(config) => config.relay_url,
                Err(error) => return LocalResponse::failure(error),
            };
            let latest = runtime.latest.borrow().clone();
            let health = runtime.health.read().await.clone();
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
            let config = AgentConfig { relay_url };
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

fn node_info(node_id: Uuid) -> NodeInfo {
    let capabilities = ["cpu", "memory", "swap", "disk", "network"]
        .into_iter()
        .map(|name| MetricCapability {
            name: name.to_owned(),
            state: CapabilityState::Supported,
            detail: None,
        })
        .collect();
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

    use super::validate_relay_url;
    #[cfg(unix)]
    use super::{AppPaths, local_socket_candidates};

    #[test]
    fn relay_url_validation_fails_closed() {
        assert!(validate_relay_url(Some("not a relay URL")).is_err());
        assert!(validate_relay_url(Some("https://relay.example.test")).is_ok());
        assert!(validate_relay_url(None).is_ok());
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
