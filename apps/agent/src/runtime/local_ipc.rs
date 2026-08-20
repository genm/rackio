//! Authorized OS-local IPC transport, command dispatch, and CLI client.

#[cfg(unix)]
use std::{fs, path::PathBuf, time::Instant};
use std::{sync::Arc, time::Duration};

#[cfg(windows)]
use anyhow::Context as _;
use anyhow::anyhow;
use rackio_core::{HealthSnapshot, HistoryResolution, NodeInfo};
use rackio_iroh::NodeRuntime;
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncBufReadExt as _, AsyncReadExt as _, AsyncWriteExt as _, BufReader};

use crate::remote::{RemoteFleet, RemoteHistoryResolution, RemoteMachineSnapshot};

use super::config::{AppPaths, load_config, save_config, validate_relay_url};

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

#[cfg(unix)]
pub(super) async fn run_local_server(
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
pub(super) async fn run_local_server(
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
pub(super) async fn run_local_server(
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

#[cfg(test)]
mod tests {
    #[cfg(unix)]
    use std::path::PathBuf;

    use super::{MAX_LOCAL_REQUEST_BYTES, read_local_request};

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
