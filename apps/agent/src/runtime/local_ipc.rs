//! Authorized OS-local IPC transport, command dispatch, and CLI client.

#[cfg(unix)]
use std::{fs, path::PathBuf, time::Instant};
use std::{net::SocketAddr, sync::Arc, time::Duration};

#[cfg(windows)]
use anyhow::Context as _;
use anyhow::anyhow;
use rackio_core::{HealthSnapshot, HistoryResolution, NodeInfo};
use rackio_iroh::NodeRuntime;
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncBufReadExt as _, AsyncReadExt as _, AsyncWriteExt as _, BufReader};
use tokio::sync::watch;

use crate::remote::{RemoteFleet, RemoteHistoryResolution, RemoteMachineSnapshot};

use super::config::{
    AppPaths, add_advertise_address, load_config, parse_advertise_address,
    remove_advertise_address, save_config, validate_bind_port, validate_relay_url,
};

// The Windows local IPC listener has its own connect loop, so these bound the
// Unix accept loop only.
#[cfg(unix)]
const ACCEPT_RETRY_DELAY: Duration = Duration::from_millis(100);
#[cfg(unix)]
const ACCEPT_FAILURE_GRACE: Duration = Duration::from_secs(30);
const LOCAL_REQUEST_TIMEOUT: Duration = Duration::from_secs(5);
/// The largest `LocalCommand` is a pairing import, whose bundle the desktop
/// already caps at 16 KiB. 64 KiB leaves ample headroom for every command while
/// keeping a single request bounded.
const MAX_LOCAL_REQUEST_BYTES: u64 = 64 * 1024;

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
    BindPortSet {
        bind_port: Option<u16>,
    },
    /// Inspect and change this machine's local health thresholds.
    Alerts {
        #[serde(flatten)]
        alert: AlertCommand,
    },
    AdvertiseAddressAdd {
        address: String,
    },
    AdvertiseAddressRemove {
        address: String,
    },
    AdvertiseAddressList,
    Doctor,
}

/// The threshold operations, mirroring `rackio alerts` so one shape describes
/// the CLI, the IPC contract and the daemon handler.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "alerts", rename_all = "snake_case")]
pub enum AlertCommand {
    /// Every effective rule, including the ones switched off.
    List,
    /// Change one rule. Absent fields keep whatever the rule has now, so
    /// retuning a level never restates the rest of the rule.
    Set {
        id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        metric: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        comparison: Option<rackio_core::Comparison>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        threshold: Option<f64>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        consecutive_samples: Option<u32>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        severity: Option<rackio_core::NodeState>,
    },
    /// Switch one rule on or off without losing its level.
    RuleEnabled { id: String, enabled: bool },
    /// Drop the operator's changes to one rule, or to all of them, restoring
    /// the shipped levels.
    Reset {
        /// `None` resets every rule on this machine.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        id: Option<String>,
    },
    /// Switch local threshold evaluation on or off for this machine.
    Enabled { enabled: bool },
}

/// One effective rule as an operator surface reports it.
#[derive(Debug, Serialize)]
struct AlertRuleView {
    id: String,
    metric: String,
    comparison: rackio_core::Comparison,
    threshold: f64,
    consecutive_samples: u32,
    severity: rackio_core::NodeState,
    enabled: bool,
    source: rackio_core::AlertRuleSource,
}

impl From<rackio_core::ResolvedAlertRule> for AlertRuleView {
    fn from(resolved: rackio_core::ResolvedAlertRule) -> Self {
        Self {
            id: resolved.rule.id,
            metric: resolved.rule.metric,
            comparison: resolved.rule.comparison,
            threshold: resolved.rule.threshold,
            consecutive_samples: resolved.rule.consecutive_samples,
            severity: resolved.rule.severity,
            enabled: resolved.enabled,
            source: resolved.source,
        }
    }
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
    /// The configured fixed listen port, or `None` when this machine takes an
    /// ephemeral port that a restart will change.
    bind_port: Option<u16>,
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

/// Everything a local command may act on.
///
/// Bundled rather than threaded through each platform listener: three listener
/// variants and two stream helpers all carry the same handles, so every new
/// daemon-side capability would otherwise mean five identical signature edits.
pub(super) struct LocalContext {
    paths: AppPaths,
    endpoint: iroh::Endpoint,
    runtime: Arc<NodeRuntime>,
    remote_fleet: RemoteFleet,
    /// The live rule set the sampler evaluates. Publishing here is what makes a
    /// threshold change take effect without restarting the daemon.
    alert_rules: watch::Sender<Vec<rackio_core::AlertRule>>,
}

impl LocalContext {
    pub(super) fn new(
        paths: AppPaths,
        endpoint: iroh::Endpoint,
        runtime: Arc<NodeRuntime>,
        remote_fleet: RemoteFleet,
        alert_rules: watch::Sender<Vec<rackio_core::AlertRule>>,
    ) -> Arc<Self> {
        Arc::new(Self {
            paths,
            endpoint,
            runtime,
            remote_fleet,
            alert_rules,
        })
    }
}

#[cfg(unix)]
pub(super) async fn run_local_server(context: Arc<LocalContext>) -> anyhow::Result<()> {
    use std::os::unix::fs::{FileTypeExt as _, PermissionsExt as _};
    use tokio::net::UnixListener;

    let socket = &context.paths.local_socket;
    match fs::symlink_metadata(socket) {
        Ok(metadata) if metadata.file_type().is_socket() => fs::remove_file(socket)?,
        Ok(_) => anyhow::bail!("refusing to replace non-socket path {}", socket.display()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.into()),
    }
    let listener = UnixListener::bind(socket)?;
    let shared = std::env::var_os("RACKIO_SHARED_SOCKET").is_some();
    fs::set_permissions(
        socket,
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
        tokio::spawn(serve_local_stream(stream, Arc::clone(&context)));
    }
}

#[cfg(windows)]
pub(super) async fn run_local_server(context: Arc<LocalContext>) -> anyhow::Result<()> {
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
            Arc::clone(&context),
        ));
        server = next;
    }
}

#[cfg(not(any(unix, windows)))]
pub(super) async fn run_local_server(_context: Arc<LocalContext>) -> anyhow::Result<()> {
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
async fn serve_local_stream<S>(stream: S, context: Arc<LocalContext>)
where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + Send + 'static,
{
    serve_local_stream_prefixed(stream, Vec::new(), context).await;
}

async fn serve_local_stream_prefixed<S>(stream: S, prefix: Vec<u8>, context: Arc<LocalContext>)
where
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
        Ok(command) => handle_local(&context, command).await,
        Err(response) => response,
    };
    if let Ok(mut bytes) = serde_json::to_vec(&response) {
        bytes.push(b'\n');
        let _ = write.write_all(&bytes).await;
    }
}

async fn handle_local(context: &LocalContext, command: LocalCommand) -> LocalResponse {
    let LocalContext {
        paths,
        endpoint,
        runtime,
        remote_fleet,
        alert_rules,
    } = context;
    match command {
        LocalCommand::Status | LocalCommand::Doctor => {
            match local_status(paths, endpoint, runtime).await {
                Ok(local) => LocalResponse::success(local),
                Err(error) => LocalResponse::failure(error),
            }
        }
        LocalCommand::FleetSnapshot => match local_status(paths, endpoint, runtime).await {
            Ok(local) => LocalResponse::success(FleetPayload {
                local,
                remotes: remote_fleet.snapshots().await,
            }),
            Err(error) => LocalResponse::failure(error),
        },
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
        LocalCommand::Alerts { alert } => handle_alerts(paths, alert_rules, alert),
        LocalCommand::BindPortSet { bind_port } => {
            if let Err(error) = validate_bind_port(bind_port) {
                return LocalResponse::failure(error);
            }
            // Preserve every other configured value for the same reason the
            // relay setting does: one field must not discard the rest.
            let mut config = match load_config(paths) {
                Ok(config) => config,
                Err(error) => return LocalResponse::failure(error),
            };
            config.bind_port = bind_port;
            match save_config(paths, &config) {
                Ok(()) => LocalResponse::success(serde_json::json!({
                    "saved": true,
                    "restart_required": true
                })),
                Err(error) => LocalResponse::failure(error),
            }
        }
        LocalCommand::AdvertiseAddressAdd { address } => {
            update_advertise_addresses(paths, &address, add_advertise_address)
        }
        LocalCommand::AdvertiseAddressRemove { address } => {
            update_advertise_addresses(paths, &address, remove_advertise_address)
        }
        LocalCommand::AdvertiseAddressList => match load_config(paths) {
            Ok(config) => LocalResponse::success(serde_json::json!({
                "advertise_addresses": config.advertise_addresses
            })),
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

/// The operator's entry for `id`, created empty if this is their first change
/// to it. An override carrying only an id is a no-op, so an unused entry
/// changes nothing.
fn alert_entry<'a>(
    config: &'a mut super::config::AgentConfig,
    id: &str,
) -> &'a mut rackio_core::AlertRuleConfig {
    if let Some(index) = config.alerts.iter().position(|entry| entry.id == id) {
        return &mut config.alerts[index];
    }
    config.alerts.push(rackio_core::AlertRuleConfig::new(id));
    config
        .alerts
        .last_mut()
        .unwrap_or_else(|| unreachable!("the entry was just pushed"))
}

fn alert_listing(config: &super::config::AgentConfig) -> LocalResponse {
    match config.resolved_alert_rules() {
        Ok(rules) => LocalResponse::success(serde_json::json!({
            "alerts_enabled": config.alerts_enabled,
            "rules": rules.into_iter().map(AlertRuleView::from).collect::<Vec<_>>(),
        })),
        Err(error) => LocalResponse::failure(error),
    }
}

/// Apply a threshold change: validate, persist, then publish to the running
/// sampler.
///
/// Validation happens before the file is written, so a rejected change leaves
/// both the configuration and the running rules exactly as they were — a
/// daemon that refused to start on its own saved configuration would be a
/// machine an operator silenced by accident.
fn update_alerts(
    paths: &AppPaths,
    alert_rules: &watch::Sender<Vec<rackio_core::AlertRule>>,
    change: impl FnOnce(&mut super::config::AgentConfig),
) -> LocalResponse {
    let mut config = match load_config(paths) {
        Ok(config) => config,
        Err(error) => return LocalResponse::failure(error),
    };
    change(&mut config);
    let effective = match config.alert_rules() {
        Ok(rules) => rules,
        Err(error) => return LocalResponse::failure(error),
    };
    if let Err(error) = save_config(paths, &config) {
        return LocalResponse::failure(error);
    }
    if alert_rules.send(effective).is_err() {
        // The sampler is gone, so the saved rules are not the running ones.
        // Reporting success here would tell an operator their machine is
        // watching a threshold that nothing is evaluating.
        return LocalResponse::failure(
            "thresholds were saved but the sampler is not running; restart the daemon",
        );
    }
    tracing::info!(
        rules = alert_rules.borrow().len(),
        alerts_enabled = config.alerts_enabled,
        "local health thresholds changed"
    );
    alert_listing(&config)
}

fn handle_alerts(
    paths: &AppPaths,
    alert_rules: &watch::Sender<Vec<rackio_core::AlertRule>>,
    command: AlertCommand,
) -> LocalResponse {
    match command {
        AlertCommand::List => match load_config(paths) {
            Ok(config) => alert_listing(&config),
            Err(error) => LocalResponse::failure(error),
        },
        AlertCommand::Set {
            id,
            metric,
            comparison,
            threshold,
            consecutive_samples,
            severity,
        } => update_alerts(paths, alert_rules, |config| {
            let entry = alert_entry(config, &id);
            // Absent means "leave as is": an operator raising a threshold must
            // not silently reset the sample window along with it.
            if metric.is_some() {
                entry.metric = metric;
            }
            if comparison.is_some() {
                entry.comparison = comparison;
            }
            if threshold.is_some() {
                entry.threshold = threshold;
            }
            if consecutive_samples.is_some() {
                entry.consecutive_samples = consecutive_samples;
            }
            if severity.is_some() {
                entry.severity = severity;
            }
        }),
        AlertCommand::RuleEnabled { id, enabled } => update_alerts(paths, alert_rules, |config| {
            alert_entry(config, &id).enabled = Some(enabled);
        }),
        AlertCommand::Reset { id } => update_alerts(paths, alert_rules, |config| match id {
            Some(id) => config.alerts.retain(|entry| entry.id != id),
            None => config.alerts.clear(),
        }),
        AlertCommand::Enabled { enabled } => update_alerts(paths, alert_rules, |config| {
            config.alerts_enabled = enabled;
        }),
    }
}

/// Apply one operator change to the advertised addresses and persist it.
///
/// The address is parsed and the change applied entirely locally: nothing is
/// resolved, probed or connected to. An address that turns out to be wrong is
/// an ordinary unreachable candidate and surfaces as the existing
/// unreachable-machine state, never as a silent correction.
///
/// A restart is reported as required because the endpoint reads these
/// addresses when it binds: until then the change reaches new pairing bundles
/// but not `status.direct_addresses` and not path selection.
fn update_advertise_addresses(
    paths: &AppPaths,
    address: &str,
    update: fn(&mut Vec<SocketAddr>, SocketAddr) -> Result<(), String>,
) -> LocalResponse {
    let address = match parse_advertise_address(address) {
        Ok(address) => address,
        Err(error) => return LocalResponse::failure(error),
    };
    // Preserve every other configured value, for the same reason the relay and
    // listen-port commands do.
    let mut config = match load_config(paths) {
        Ok(config) => config,
        Err(error) => return LocalResponse::failure(error),
    };
    if let Err(error) = update(&mut config.advertise_addresses, address) {
        return LocalResponse::failure(error);
    }
    match save_config(paths, &config) {
        Ok(()) => LocalResponse::success(serde_json::json!({
            "saved": true,
            "restart_required": true,
            "advertise_addresses": config.advertise_addresses
        })),
        Err(error) => LocalResponse::failure(error),
    }
}

/// The direct candidates a pairing bundle carries: the addresses this machine
/// observes on its own interfaces, then the operator-configured ones it cannot
/// observe, such as a router's forwarded address.
///
/// Order is stable and duplicates are dropped, so a machine whose forwarded
/// address happens to match an interface address is offered once.
fn bundle_direct_addresses(observed: &[SocketAddr], advertised: &[SocketAddr]) -> Vec<SocketAddr> {
    let mut addresses: Vec<SocketAddr> = Vec::with_capacity(observed.len() + advertised.len());
    for address in observed.iter().chain(advertised.iter()) {
        if !addresses.contains(address) {
            addresses.push(*address);
        }
    }
    addresses
}

async fn local_status(
    paths: &AppPaths,
    endpoint: &iroh::Endpoint,
    runtime: &Arc<NodeRuntime>,
) -> anyhow::Result<StatusPayload> {
    let config = load_config(paths)?;
    // Read the watch channel into an owned value before the first await: its
    // guard is not `Send`, and holding it across one makes this whole task
    // unspawnable.
    let latest = runtime.latest.borrow().clone();
    Ok(StatusPayload {
        node: runtime.info.clone(),
        endpoint_id: endpoint.id().to_string(),
        direct_addresses: endpoint
            .addr()
            .ip_addrs()
            .map(std::string::ToString::to_string)
            .collect(),
        relay_url: config.relay_url,
        bind_port: config.bind_port,
        latest,
        health: runtime.health.read().await.clone(),
        trend: runtime.trend.read().await.clone(),
    })
}

async fn create_pairing_bundle(
    paths: &AppPaths,
    endpoint: &iroh::Endpoint,
    runtime: &Arc<NodeRuntime>,
) -> LocalResponse {
    let config = match load_config(paths) {
        Ok(config) => config,
        Err(error) => return LocalResponse::failure(error),
    };
    let observed: Vec<SocketAddr> = endpoint.addr().ip_addrs().copied().collect();
    let addresses = bundle_direct_addresses(&observed, &config.advertise_addresses);
    let relay_urls = config.relay_url.into_iter().collect();
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

#[cfg(test)]
mod tests {
    #[cfg(unix)]
    use std::path::PathBuf;

    use std::net::SocketAddr;

    use super::{MAX_LOCAL_REQUEST_BYTES, bundle_direct_addresses, read_local_request};

    fn address(value: &str) -> SocketAddr {
        value.parse().unwrap_or_else(|error| panic!("{error}"))
    }

    #[test]
    fn a_bundle_offers_interface_and_advertised_addresses_once_each() {
        let observed = [address("192.168.102.10:41641"), address("10.0.0.4:41641")];
        // The operator configured the router's forwarded address and, by
        // mistake, one address the machine already observes.
        let advertised = [
            address("198.51.100.7:41641"),
            address("192.168.102.10:41641"),
        ];

        let addresses = bundle_direct_addresses(&observed, &advertised);

        assert_eq!(
            addresses,
            vec![
                address("192.168.102.10:41641"),
                address("10.0.0.4:41641"),
                address("198.51.100.7:41641"),
            ]
        );
    }

    #[test]
    fn an_address_the_endpoint_already_publishes_is_still_offered_once() {
        // The configured addresses reach the endpoint as external addresses,
        // so the endpoint's own advertised set contains them too. The overlap
        // is now the normal case rather than an operator mistake, and the
        // bundle must not list the same candidate twice because of it.
        let observed = [
            address("192.168.102.10:41641"),
            address("198.51.100.7:41641"),
        ];
        let advertised = [address("198.51.100.7:41641")];

        assert_eq!(
            bundle_direct_addresses(&observed, &advertised),
            vec![
                address("192.168.102.10:41641"),
                address("198.51.100.7:41641"),
            ]
        );
    }

    #[test]
    fn a_bundle_without_advertised_addresses_is_unchanged() {
        let observed = [address("192.168.102.10:41641")];

        assert_eq!(
            bundle_direct_addresses(&observed, &[]),
            vec![address("192.168.102.10:41641")]
        );
    }

    async fn read_request(payload: &[u8]) -> Result<super::LocalCommand, super::LocalResponse> {
        let mut reader = tokio::io::BufReader::new(tokio::io::AsyncReadExt::take(
            payload,
            MAX_LOCAL_REQUEST_BYTES,
        ));
        read_local_request(&mut reader).await
    }

    fn context_paths(root: &std::path::Path) -> super::AppPaths {
        super::AppPaths {
            config: root.join("config"),
            data: root.join("data"),
            state: root.join("state"),
            log: root.join("log"),
            #[cfg(unix)]
            local_socket: root.join("agent.sock"),
        }
    }

    fn rule_named<'a>(
        response: &'a super::LocalResponse,
        id: &str,
    ) -> &'a serde_json::Map<String, serde_json::Value> {
        response
            .data
            .as_ref()
            .and_then(|data| data.get("rules"))
            .and_then(serde_json::Value::as_array)
            .and_then(|rules| {
                rules.iter().find_map(|rule| {
                    let rule = rule.as_object()?;
                    (rule.get("id")?.as_str()? == id).then_some(rule)
                })
            })
            .unwrap_or_else(|| panic!("{id} is missing from {response:?}"))
    }

    #[test]
    fn a_threshold_change_is_saved_and_handed_to_the_running_sampler() {
        // Thresholds are changed while watching a machine misbehave. A change
        // that only lands in a file the sampler already read would leave the
        // operator watching a level nothing is evaluating.
        let directory = tempfile::tempdir().unwrap_or_else(|error| panic!("{error}"));
        let paths = context_paths(directory.path());
        let (rules, _receiver) = super::watch::channel(Vec::new());

        let response = super::handle_alerts(
            &paths,
            &rules,
            super::AlertCommand::Set {
                id: String::from("disk-capacity-warning"),
                metric: None,
                comparison: None,
                threshold: Some(80.0),
                consecutive_samples: None,
                severity: None,
            },
        );

        assert!(response.ok, "{response:?}");
        assert_eq!(
            rule_named(&response, "disk-capacity-warning").get("threshold"),
            Some(&serde_json::json!(80.0))
        );
        let running = rules.borrow();
        let applied = running
            .iter()
            .find(|rule| rule.id == "disk-capacity-warning")
            .unwrap_or_else(|| panic!("the sampler did not receive the rule"));
        assert!((applied.threshold - 80.0).abs() < f64::EPSILON);
        // Only the change is persisted, so later releases still reach the
        // levels this operator never touched.
        let saved = std::fs::read_to_string(paths.config.join("config.json"))
            .unwrap_or_else(|error| panic!("{error}"));
        assert!(saved.contains("disk-capacity-warning"), "{saved}");
        assert!(!saved.contains("disk-capacity-critical"), "{saved}");
    }

    #[test]
    fn a_rejected_change_leaves_both_the_file_and_the_running_rules_alone() {
        // Saving first and validating later would let one bad command wedge the
        // daemon out of starting, silencing the machine.
        let directory = tempfile::tempdir().unwrap_or_else(|error| panic!("{error}"));
        let paths = context_paths(directory.path());
        let (rules, _receiver) = super::watch::channel(Vec::new());

        let response = super::handle_alerts(
            &paths,
            &rules,
            super::AlertCommand::Set {
                id: String::from("disk-capacity-warning"),
                metric: Some(String::from("gpu_percent")),
                comparison: None,
                threshold: None,
                consecutive_samples: None,
                severity: None,
            },
        );

        assert!(!response.ok);
        assert!(
            response
                .error
                .as_deref()
                .is_some_and(|error| error.contains("gpu_percent")),
            "{response:?}"
        );
        assert!(rules.borrow().is_empty(), "nothing may reach the sampler");
        assert!(!paths.config.join("config.json").exists());
    }

    #[test]
    fn switching_alerting_off_hands_the_sampler_an_empty_rule_set() {
        let directory = tempfile::tempdir().unwrap_or_else(|error| panic!("{error}"));
        let paths = context_paths(directory.path());
        let (rules, _receiver) = super::watch::channel(rackio_core::default_alert_rules());

        let off = super::handle_alerts(
            &paths,
            &rules,
            super::AlertCommand::Enabled { enabled: false },
        );

        assert!(off.ok, "{off:?}");
        assert!(rules.borrow().is_empty());
        // The rules stay listed, or an operator could not turn them back on.
        assert!(
            rule_named(&off, "disk-capacity-warning")
                .get("enabled")
                .is_some()
        );

        let on = super::handle_alerts(
            &paths,
            &rules,
            super::AlertCommand::Enabled { enabled: true },
        );
        assert!(on.ok, "{on:?}");
        assert_eq!(
            rules.borrow().len(),
            rackio_core::default_alert_rules().len()
        );
    }

    #[test]
    fn a_change_that_cannot_reach_the_sampler_is_not_reported_as_applied() {
        // The sampler task is gone: the file now says something the machine is
        // not doing, and the operator has to hear that.
        let directory = tempfile::tempdir().unwrap_or_else(|error| panic!("{error}"));
        let paths = context_paths(directory.path());
        let (rules, receiver) = super::watch::channel(Vec::new());
        drop(receiver);

        let response = super::handle_alerts(
            &paths,
            &rules,
            super::AlertCommand::Enabled { enabled: false },
        );

        assert!(!response.ok);
        assert!(
            response
                .error
                .as_deref()
                .is_some_and(|error| error.contains("restart")),
            "{response:?}"
        );
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
    use super::{AppPaths, local_socket_candidates};

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
