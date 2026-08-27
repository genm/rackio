use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex as StdMutex},
};

use chrono::Utc;
use iroh::{Endpoint, EndpointId, endpoint::Connection};
use rackio_core::{
    ConnectionPath, HealthSnapshot, HistoryResolution, MetricSample, MetricStore, NodeInfo,
    StoreError, TrendWindow,
};
use rackio_protocol::{
    FrameError, compatible, read_frame,
    v1::{
        ErrorResponse, Heartbeat, HistoryQuery, PairResponse, Request, Response, StreamComplete,
        history_query::Resolution, request, response,
    },
    write_frame,
};
use thiserror::Error;
use tokio::sync::{Mutex, RwLock, watch};

use crate::{
    PairingError, PairingManager, PairingMdnsState, PeerPermissions, PeerRegistry,
    classify_connection, protocol,
};

const HISTORY_PAGE_SIZE: usize = 256;

/// A viewer sends its request immediately after opening a stream. Bound the
/// wait so a peer cannot pin a task and its frame buffer by opening streams and
/// never sending a request. This does not bound a request already being served.
const REQUEST_HEADER_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);

#[derive(Debug, Error)]
pub enum ServerError {
    #[error("connection failed: {0}")]
    Connection(#[from] iroh::endpoint::ConnectionError),
    #[error("connection handshake failed: {0}")]
    Connecting(#[from] iroh::endpoint::ConnectingError),
    #[error("stream failed: {0}")]
    Stream(#[from] FrameError),
    #[error("stream was closed: {0}")]
    StreamClosed(#[from] iroh::endpoint::ClosedStream),
    #[error("peer authorization failed: {0}")]
    Pairing(#[from] PairingError),
    #[error("history storage failed: {0}")]
    Store(#[from] StoreError),
    #[error("pairing state lock is unavailable")]
    PairingStateUnavailable,
    #[error("active connection state lock is unavailable")]
    ActiveConnectionStateUnavailable,
    #[error("peer opened a stream without sending a request in time")]
    RequestHeaderTimeout,
}

pub struct NodeRuntime {
    pub info: NodeInfo,
    pub health: RwLock<HealthSnapshot>,
    pub latest: watch::Receiver<Option<MetricSample>>,
    /// The local machine's own live trend, fed by the collector loop: the
    /// viewer shows this machine next to its remotes, so it needs the same
    /// trend the remotes stream in.
    pub trend: RwLock<TrendWindow>,
    pub store: Mutex<MetricStore>,
    pub pairing: std::sync::Mutex<PairingManager>,
    pub pairing_mdns: Arc<PairingMdnsState>,
    pub peers: PeerRegistry,
    pub active_connections: StdMutex<BTreeMap<String, BTreeMap<usize, Connection>>>,
}

impl NodeRuntime {
    pub fn revoke_peer(&self, endpoint_id: &str) -> Result<bool, ServerError> {
        let removed = self.peers.revoke(endpoint_id)?;
        if removed {
            let connections = self
                .active_connections
                .lock()
                .map_err(|_| ServerError::ActiveConnectionStateUnavailable)?
                .remove(endpoint_id)
                .unwrap_or_default();
            tracing::info!(
                peer = %endpoint_id,
                torn_down = connections.len(),
                "peer authorization revoked"
            );
            for connection in connections.into_values() {
                connection.close(0_u32.into(), b"peer revoked");
            }
        }
        Ok(removed)
    }

    fn register_connection(&self, connection: &Connection) -> Result<(), ServerError> {
        self.active_connections
            .lock()
            .map_err(|_| ServerError::ActiveConnectionStateUnavailable)?
            .entry(connection.remote_id().to_string())
            .or_default()
            .insert(connection.stable_id(), connection.clone());
        Ok(())
    }

    fn unregister_connection(&self, connection: &Connection) {
        let Ok(mut active) = self.active_connections.lock() else {
            return;
        };
        let endpoint_id = connection.remote_id().to_string();
        if let Some(connections) = active.get_mut(&endpoint_id) {
            connections.remove(&connection.stable_id());
            if connections.is_empty() {
                active.remove(&endpoint_id);
            }
        }
    }
}

#[derive(Clone)]
pub struct RemoteServer {
    endpoint: Endpoint,
    runtime: Arc<NodeRuntime>,
}

impl RemoteServer {
    #[must_use]
    pub fn new(endpoint: Endpoint, runtime: Arc<NodeRuntime>) -> Self {
        Self { endpoint, runtime }
    }

    #[must_use]
    pub fn endpoint(&self) -> &Endpoint {
        &self.endpoint
    }

    pub async fn run(self) {
        while let Some(incoming) = self.endpoint.accept().await {
            let runtime = Arc::clone(&self.runtime);
            let local_id = self.endpoint.id();
            tokio::spawn(async move {
                let result = async {
                    let connection = incoming.await?;
                    runtime.register_connection(&connection)?;
                    let result =
                        handle_connection(connection.clone(), local_id, Arc::clone(&runtime)).await;
                    runtime.unregister_connection(&connection);
                    result
                }
                .await;
                if let Err(error) = result {
                    tracing::warn!(error = %error, "remote connection ended with an error");
                }
            });
        }
    }
}

async fn handle_connection(
    connection: Connection,
    local_id: EndpointId,
    runtime: Arc<NodeRuntime>,
) -> Result<(), ServerError> {
    let remote_id = connection.remote_id();
    loop {
        let stream = connection.accept_bi().await;
        let (mut send, mut receive) = match stream {
            Ok(stream) => stream,
            Err(iroh::endpoint::ConnectionError::ApplicationClosed(_)) => return Ok(()),
            Err(error) => return Err(error.into()),
        };
        let connection = connection.clone();
        let runtime = Arc::clone(&runtime);
        // A live metrics stream is intentionally long-lived. Handle each QUIC
        // stream independently so control and history requests are not blocked.
        tokio::spawn(async move {
            let result = async {
                let request: Request =
                    match tokio::time::timeout(REQUEST_HEADER_TIMEOUT, read_frame(&mut receive))
                        .await
                    {
                        Ok(request) => request?,
                        Err(_) => return Err(ServerError::RequestHeaderTimeout),
                    };
                handle_request(
                    &connection,
                    remote_id,
                    local_id,
                    request,
                    &mut send,
                    runtime,
                )
                .await?;
                send.finish()?;
                Ok::<(), ServerError>(())
            }
            .await;
            if let Err(error) = result {
                tracing::warn!(error = %error, "remote stream ended with an error");
            }
        });
    }
}

async fn handle_request(
    connection: &Connection,
    remote_id: EndpointId,
    local_id: EndpointId,
    request: Request,
    send: &mut iroh::endpoint::SendStream,
    runtime: Arc<NodeRuntime>,
) -> Result<(), ServerError> {
    let Some(body) = request.body else {
        write_error(send, "invalid_request", "request body is required").await?;
        return Ok(());
    };

    if let request::Body::Pair(pair) = body {
        return handle_pair(send, remote_id, local_id, &runtime, &pair).await;
    }

    let Some(permissions) = runtime.peers.permissions(remote_id)? else {
        tracing::warn!(peer = %remote_id, "rejected request from an unauthorized peer");
        write_error(send, "auth_error", "peer is not authorized").await?;
        return Ok(());
    };

    handle_authorized(connection, local_id, body, permissions, send, runtime).await
}

async fn handle_pair(
    send: &mut iroh::endpoint::SendStream,
    remote_id: EndpointId,
    local_id: EndpointId,
    runtime: &NodeRuntime,
    pair: &rackio_protocol::v1::PairRequest,
) -> Result<(), ServerError> {
    if pair.viewer_endpoint_id != remote_id.to_string() {
        write_error(
            send,
            "identity_mismatch",
            "claimed viewer identity does not match the QUIC peer",
        )
        .await?;
        return Ok(());
    }
    let (verified, window_closed) = {
        let mut pairing = runtime
            .pairing
            .lock()
            .map_err(|_| ServerError::PairingStateUnavailable)?;
        let verified = pairing.verify_and_consume(remote_id, &pair.one_time_secret);
        (verified, !pairing.is_open())
    };
    if window_closed {
        runtime.pairing_mdns.close().await;
    }
    if verified.is_err() {
        // Audit trail only: the endpoint ID is the authenticated QUIC identity.
        // The supplied secret is never logged.
        tracing::warn!(
            peer = %remote_id,
            window_closed,
            "pairing attempt rejected"
        );
        write_error(send, "pairing_rejected", "pairing request was rejected").await?;
        return Ok(());
    }
    let permissions = runtime.peers.authorize_preserving(remote_id)?;
    tracing::info!(
        peer = %remote_id,
        read_metrics = permissions.read_metrics,
        read_history = permissions.read_history,
        "peer paired and authorized"
    );
    write_response(
        send,
        response::Body::Pair(PairResponse {
            accepted: true,
            node_id: runtime.info.node_id.to_string(),
            endpoint_id: local_id.to_string(),
        }),
    )
    .await?;
    Ok(())
}

async fn handle_authorized(
    connection: &Connection,
    local_id: EndpointId,
    body: request::Body,
    permissions: PeerPermissions,
    send: &mut iroh::endpoint::SendStream,
    runtime: Arc<NodeRuntime>,
) -> Result<(), ServerError> {
    match body {
        request::Body::GetNodeInfo(version) => {
            if require_permission(send, permissions.read_metrics, "read_metrics").await?
                && version_is_compatible(send, &version).await?
            {
                write_response(
                    send,
                    response::Body::NodeInfo(protocol::node_info(&runtime.info, local_id)),
                )
                .await?;
            }
        }
        request::Body::GetHealth(version) => {
            if require_permission(send, permissions.read_metrics, "read_metrics").await?
                && version_is_compatible(send, &version).await?
            {
                // Snapshot before writing. Holding the health read guard across
                // the QUIC write would let a peer that stops reading block the
                // sampler's health writer, stalling local collection.
                let health = {
                    let guard = runtime.health.read().await;
                    protocol::health(&guard)
                };
                write_response(send, response::Body::Health(health)).await?;
            }
        }
        request::Body::GetConnectionPath(version) => {
            if require_permission(send, permissions.read_metrics, "read_metrics").await?
                && version_is_compatible(send, &version).await?
            {
                let details = settled_connection_details(connection).await;
                write_response(
                    send,
                    response::Body::ConnectionPath(protocol::connection_details(
                        details.path,
                        details.rtt_ms,
                    )),
                )
                .await?;
            }
        }
        request::Body::WatchMetrics(version) => {
            if require_permission(send, permissions.read_metrics, "read_metrics").await?
                && version_is_compatible(send, &version).await?
            {
                watch_metrics(send, runtime.latest.clone()).await?;
            }
        }
        request::Body::QueryHistory(query) => {
            handle_history(send, &runtime, permissions, &query).await?;
        }
        request::Body::Pair(_) => unreachable!("pair requests return before authorization"),
    }
    Ok(())
}

async fn settled_connection_details(
    connection: &Connection,
) -> crate::transport::ConnectionDetails {
    settle(|| classify_connection(connection)).await
}

/// Give path selection a bounded moment to converge.
///
/// A connection reports no selected path for the first instants of its life.
/// Answering from that snapshot would tell the viewer "unknown" for a link that
/// is about to be reported as direct, so retry while it is still unknown and
/// return whatever the last look found once the budget runs out.
async fn settle<F>(mut classify: F) -> crate::transport::ConnectionDetails
where
    F: FnMut() -> crate::transport::ConnectionDetails,
{
    let mut details = classify();
    for _ in 0..10 {
        if details.path != ConnectionPath::Unknown {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(25)).await;
        details = classify();
    }
    details
}

async fn handle_history(
    send: &mut iroh::endpoint::SendStream,
    runtime: &NodeRuntime,
    permissions: PeerPermissions,
    query: &HistoryQuery,
) -> Result<(), ServerError> {
    if !require_permission(send, permissions.read_history, "read_history").await? {
        return Ok(());
    }
    let Some(version) = query.protocol.as_ref() else {
        write_error(
            send,
            "invalid_request",
            "history protocol version is required",
        )
        .await?;
        return Ok(());
    };
    if !version_is_compatible(send, version).await? {
        return Ok(());
    }
    if query.from_ms > query.to_ms {
        write_error(send, "invalid_range", "history range is reversed").await?;
        return Ok(());
    }
    let resolution = match Resolution::try_from(query.resolution) {
        Ok(Resolution::Raw) => HistoryResolution::Raw,
        Ok(Resolution::Minute) => HistoryResolution::Minute,
        _ => {
            write_error(send, "invalid_resolution", "history resolution is required").await?;
            return Ok(());
        }
    };
    let mut cursor_ms = query.from_ms;
    loop {
        let samples = runtime.store.lock().await.query_page(
            cursor_ms,
            query.to_ms,
            resolution,
            HISTORY_PAGE_SIZE,
        )?;
        let Some(last_timestamp_ms) = samples.last().map(|sample| sample.timestamp_ms) else {
            break;
        };
        for sample in samples {
            write_response(
                send,
                response::Body::MetricSample(protocol::metric_sample(&sample)),
            )
            .await?;
        }
        let next_cursor_ms = last_timestamp_ms.saturating_add(1);
        if next_cursor_ms <= cursor_ms {
            break;
        }
        cursor_ms = next_cursor_ms;
    }
    write_response(send, response::Body::StreamComplete(StreamComplete {})).await?;
    Ok(())
}

async fn require_permission(
    send: &mut iroh::endpoint::SendStream,
    allowed: bool,
    permission: &str,
) -> Result<bool, FrameError> {
    if allowed {
        Ok(true)
    } else {
        write_error(
            send,
            "permission_denied",
            &format!("peer does not have {permission} permission"),
        )
        .await?;
        Ok(false)
    }
}

async fn version_is_compatible(
    send: &mut iroh::endpoint::SendStream,
    version: &rackio_protocol::v1::ProtocolVersion,
) -> Result<bool, FrameError> {
    if compatible(version) {
        Ok(true)
    } else {
        write_error(
            send,
            "incompatible",
            "protocol major version is incompatible",
        )
        .await?;
        Ok(false)
    }
}

async fn watch_metrics(
    send: &mut iroh::endpoint::SendStream,
    mut latest: watch::Receiver<Option<MetricSample>>,
) -> Result<(), ServerError> {
    let initial = latest.borrow().clone();
    if let Some(sample) = initial {
        write_response(
            send,
            response::Body::MetricSample(protocol::metric_sample(&sample)),
        )
        .await?;
    }
    let mut heartbeat = tokio::time::interval(std::time::Duration::from_secs(5));
    loop {
        tokio::select! {
            changed = latest.changed() => {
                if changed.is_err() {
                    return Ok(());
                }
                let sample = latest.borrow().clone();
                if let Some(sample) = sample {
                    write_response(send, response::Body::MetricSample(protocol::metric_sample(&sample))).await?;
                }
            }
            _ = heartbeat.tick() => {
                let sequence = latest.borrow().as_ref().map_or(0, |sample| sample.sequence);
                write_response(send, response::Body::Heartbeat(Heartbeat {
                    timestamp_ms: Utc::now().timestamp_millis(),
                    sequence,
                })).await?;
            }
        }
    }
}

async fn write_response(
    send: &mut iroh::endpoint::SendStream,
    body: response::Body,
) -> Result<(), FrameError> {
    write_frame(send, &Response { body: Some(body) }).await
}

async fn write_error(
    send: &mut iroh::endpoint::SendStream,
    code: &str,
    message: &str,
) -> Result<(), FrameError> {
    write_response(
        send,
        response::Body::Error(ErrorResponse {
            code: code.to_owned(),
            message: message.to_owned(),
        }),
    )
    .await
}

#[cfg(test)]
mod tests {
    use std::{
        net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr},
        sync::Arc,
        time::Duration,
    };

    use rackio_core::{
        ConnectionPath, HealthSnapshot, MetricSample, MetricStore, NodeInfo, NodeState,
        ProtocolVersion as CoreProtocolVersion, TrendWindow,
    };
    use rackio_protocol::{
        current_version,
        v1::{
            HistoryQuery, PairRequest, ProtocolVersion, Request, history_query, request, response,
        },
    };
    use tokio::sync::{RwLock, watch};
    use uuid::Uuid;

    use crate::{
        ClientConnection, EndpointConfig, PairingManager, PairingMdnsState, PeerPermissions,
        PeerRegistry, bind_endpoint, transport::ConnectionDetails,
    };

    use super::{HISTORY_PAGE_SIZE, NodeRuntime, RemoteServer, settle};

    /// Every exchange here is same-host QUIC and completes in milliseconds. The
    /// bound turns "the server never answered" into a failing assertion instead
    /// of a hung suite, so it has to expire well inside the harness timeouts
    /// that would otherwise kill the run first and report nothing.
    const REPLY_TIMEOUT: Duration = Duration::from_secs(10);

    struct TestRack {
        runtime: Arc<NodeRuntime>,
        endpoint: iroh::Endpoint,
        server_task: tokio::task::JoinHandle<()>,
        /// Closed when the accept loop returns. A server that stopped
        /// accepting is a failure the harness can report immediately, instead
        /// of every connecting test separately waiting out `REPLY_TIMEOUT`
        /// until the whole suite looks hung rather than broken.
        accepting: watch::Receiver<()>,
        latest_tx: watch::Sender<Option<MetricSample>>,
        /// Client endpoints outlive their connections. Dropping one as soon as
        /// its connection closes can discard the close frame before it is sent,
        /// which the daemon's long-lived endpoint never does.
        client_endpoints: std::sync::Mutex<Vec<iroh::Endpoint>>,
        _directory: tempfile::TempDir,
    }

    impl TestRack {
        async fn start() -> Self {
            let directory = tempfile::tempdir().unwrap_or_else(|error| panic!("{error}"));
            let endpoint = bind_endpoint(iroh::SecretKey::generate(), &EndpointConfig::default())
                .await
                .unwrap_or_else(|error| panic!("{error}"));
            let (latest_tx, latest) = watch::channel(None);
            let runtime = Arc::new(NodeRuntime {
                info: NodeInfo {
                    node_id: Uuid::new_v4(),
                    display_name: String::from("Test node"),
                    os: String::from("test"),
                    architecture: String::from("test"),
                    agent_version: String::from("0.1.0"),
                    protocol: CoreProtocolVersion::V1,
                    capabilities: Vec::new(),
                },
                health: RwLock::new(HealthSnapshot {
                    state: NodeState::Healthy,
                    collector_degraded: false,
                    storage_degraded: false,
                    remote_listener_degraded: false,
                    details: Vec::new(),
                }),
                latest,
                trend: RwLock::new(TrendWindow::default()),
                store: tokio::sync::Mutex::new(
                    MetricStore::in_memory().unwrap_or_else(|error| panic!("{error}")),
                ),
                pairing: std::sync::Mutex::new(PairingManager::default()),
                pairing_mdns: Arc::new(PairingMdnsState::default()),
                peers: PeerRegistry::load(directory.path().join("peers.json"))
                    .unwrap_or_else(|error| panic!("{error}")),
                active_connections: std::sync::Mutex::new(std::collections::BTreeMap::new()),
            });
            let server = RemoteServer::new(endpoint.clone(), Arc::clone(&runtime));
            let (accepting_tx, accepting) = watch::channel(());
            let server_task = tokio::spawn(async move {
                server.run().await;
                drop(accepting_tx);
            });
            Self {
                runtime,
                endpoint,
                server_task,
                accepting,
                latest_tx,
                client_endpoints: std::sync::Mutex::new(Vec::new()),
                _directory: directory,
            }
        }

        /// Restrict every test connection to loopback so unrelated host
        /// interfaces cannot turn a same-host contract test into a WAN-path
        /// selection race.
        async fn connect(&self) -> ClientConnection {
            let advertised = self.endpoint.addr();
            let advertised_ip = advertised
                .ip_addrs()
                .find(|address| address.is_ipv4())
                .or_else(|| advertised.ip_addrs().next())
                .copied()
                .unwrap_or_else(|| panic!("test endpoint did not advertise a direct address"));
            let loopback = SocketAddr::new(
                match advertised_ip {
                    SocketAddr::V4(_) => IpAddr::V4(Ipv4Addr::LOCALHOST),
                    SocketAddr::V6(_) => IpAddr::V6(Ipv6Addr::LOCALHOST),
                },
                advertised_ip.port(),
            );
            let address = iroh::EndpointAddr::new(self.endpoint.id()).with_ip_addr(loopback);
            let client_endpoint =
                bind_endpoint(iroh::SecretKey::generate(), &EndpointConfig::default())
                    .await
                    .unwrap_or_else(|error| panic!("{error}"));
            self.client_endpoints
                .lock()
                .unwrap_or_else(|error| panic!("{error}"))
                .push(client_endpoint.clone());
            let mut accepting = self.accepting.clone();
            tokio::select! {
                // `changed()` resolves with an error once the sender is
                // dropped, which happens only when `RemoteServer::run`
                // returned. Nothing can arrive on a connection after that, so
                // waiting for the timeout would only delay the same failure.
                _ = accepting.changed() => {
                    panic!("the accept loop exited instead of accepting the connection")
                }
                result = tokio::time::timeout(
                    REPLY_TIMEOUT,
                    ClientConnection::connect(client_endpoint, address),
                ) => {
                    result
                        .unwrap_or_else(|_| panic!("the server never accepted the connection"))
                        .unwrap_or_else(|error| panic!("{error}"))
                }
            }
        }

        /// Connect and authorize in one step, for the tests whose subject is
        /// what an already-paired viewer may read.
        async fn connect_authorized(&self, permissions: PeerPermissions) -> ClientConnection {
            let client = self.connect().await;
            self.runtime
                .peers
                .authorize(client.local_id(), permissions)
                .unwrap_or_else(|error| panic!("{error}"));
            client
        }

        async fn shutdown(self) {
            let clients = std::mem::take(
                &mut *self
                    .client_endpoints
                    .lock()
                    .unwrap_or_else(|error| panic!("{error}")),
            );
            for client in clients {
                client.close().await;
            }
            self.endpoint.close().await;
            self.server_task.abort();
        }
    }

    async fn ask(client: &ClientConnection, body: request::Body) -> response::Body {
        let response =
            tokio::time::timeout(REPLY_TIMEOUT, client.request(&Request { body: Some(body) }))
                .await
                .unwrap_or_else(|_| panic!("the server never answered"))
                .unwrap_or_else(|error| panic!("{error}"));
        response
            .body
            .unwrap_or_else(|| panic!("the server answered with an empty body"))
    }

    fn error_code(body: &response::Body) -> &str {
        match body {
            response::Body::Error(error) => &error.code,
            other => panic!("expected an error response, got {other:?}"),
        }
    }

    fn sample(timestamp_ms: i64) -> MetricSample {
        MetricSample {
            timestamp_ms,
            sequence: u64::try_from(timestamp_ms).unwrap_or_default(),
            cpu_percent: Some(10.0),
            memory_used_bytes: Some(100),
            memory_total_bytes: Some(200),
            swap_used_bytes: Some(0),
            swap_total_bytes: Some(0),
            disks: Vec::new(),
            network: None,
            temperature: None,
            uptime_seconds: 1,
            errors: Vec::new(),
        }
    }

    fn history_query(from_ms: i64, to_ms: i64, resolution: history_query::Resolution) -> Request {
        Request {
            body: Some(request::Body::QueryHistory(HistoryQuery {
                from_ms,
                to_ms,
                resolution: resolution as i32,
                protocol: Some(current_version()),
            })),
        }
    }

    /// Read a whole history stream, returning the samples that preceded its
    /// completion marker.
    async fn history(client: &ClientConnection, request: &Request) -> Vec<MetricSample> {
        let mut stream = tokio::time::timeout(REPLY_TIMEOUT, client.stream(request))
            .await
            .unwrap_or_else(|_| panic!("the server never opened the stream"))
            .unwrap_or_else(|error| panic!("{error}"));
        let mut samples = Vec::new();
        loop {
            let response = tokio::time::timeout(REPLY_TIMEOUT, stream.next())
                .await
                .unwrap_or_else(|_| panic!("the history stream never completed"))
                .unwrap_or_else(|error| panic!("{error}"));
            match response.body {
                Some(response::Body::MetricSample(wire)) => samples.push(sample(wire.timestamp_ms)),
                Some(response::Body::StreamComplete(_)) => return samples,
                other => panic!("unexpected history response: {other:?}"),
            }
        }
    }

    #[tokio::test]
    async fn an_unsettled_path_is_retried_before_it_is_reported() {
        // The first look at a fresh connection reports no selected path.
        // Answering from it would tell the viewer "unknown" about a link that
        // is about to be reported as direct.
        let mut looks = 0;
        let details = settle(|| {
            looks += 1;
            ConnectionDetails {
                path: if looks < 3 {
                    ConnectionPath::Unknown
                } else {
                    ConnectionPath::LanDirect
                },
                rtt_ms: 7,
            }
        })
        .await;

        assert_eq!(details.path, ConnectionPath::LanDirect);
        assert_eq!(details.rtt_ms, 7);
    }

    #[tokio::test]
    async fn a_settled_path_is_reported_without_waiting() {
        let mut looks = 0;
        let details = settle(|| {
            looks += 1;
            ConnectionDetails {
                path: ConnectionPath::Relayed,
                rtt_ms: 3,
            }
        })
        .await;

        assert_eq!(details.path, ConnectionPath::Relayed);
        assert_eq!(looks, 1, "a known path must not be re-examined");
    }

    #[tokio::test]
    async fn an_incompatible_viewer_is_refused_every_reading() {
        // A viewer that speaks another protocol major must be told so for each
        // request, not served a payload it cannot interpret.
        let incompatible = ProtocolVersion { major: 2, minor: 0 };
        let rack = TestRack::start().await;
        let client = rack.connect_authorized(PeerPermissions::default()).await;

        for body in [
            request::Body::GetNodeInfo(incompatible),
            request::Body::GetHealth(incompatible),
            request::Body::GetConnectionPath(incompatible),
            request::Body::WatchMetrics(incompatible),
        ] {
            let response = ask(&client, body).await;
            assert_eq!(error_code(&response), "incompatible");
        }

        client.close();
        rack.shutdown().await;
    }

    #[tokio::test]
    async fn a_viewer_without_read_metrics_is_refused_every_reading() {
        let rack = TestRack::start().await;
        let client = rack
            .connect_authorized(PeerPermissions {
                read_metrics: false,
                read_history: true,
            })
            .await;

        for body in [
            request::Body::GetNodeInfo(current_version()),
            request::Body::GetHealth(current_version()),
            request::Body::GetConnectionPath(current_version()),
            request::Body::WatchMetrics(current_version()),
        ] {
            let response = ask(&client, body).await;
            assert_eq!(error_code(&response), "permission_denied");
        }

        client.close();
        rack.shutdown().await;
    }

    #[tokio::test]
    async fn a_watching_viewer_receives_the_latest_sample() {
        let rack = TestRack::start().await;
        rack.latest_tx.send_replace(Some(sample(1_000)));
        let client = rack.connect_authorized(PeerPermissions::default()).await;

        let mut stream = client
            .stream(&Request {
                body: Some(request::Body::WatchMetrics(current_version())),
            })
            .await
            .unwrap_or_else(|error| panic!("{error}"));
        let response = tokio::time::timeout(REPLY_TIMEOUT, stream.next())
            .await
            .unwrap_or_else(|_| panic!("a watching viewer was never sent anything"))
            .unwrap_or_else(|error| panic!("{error}"));

        assert!(matches!(
            response.body,
            Some(response::Body::MetricSample(ref wire)) if wire.timestamp_ms == 1_000
        ));

        client.close();
        rack.shutdown().await;
    }

    #[tokio::test]
    async fn history_is_served_at_both_resolutions_and_rejects_a_reversed_range() {
        let rack = TestRack::start().await;
        rack.runtime
            .store
            .lock()
            .await
            .insert_batch(&[sample(1_000), sample(2_000), sample(61_000)])
            .unwrap_or_else(|error| panic!("{error}"));
        let client = rack.connect_authorized(PeerPermissions::default()).await;

        let raw = history(
            &client,
            &history_query(0, 120_000, history_query::Resolution::Raw),
        )
        .await;
        assert_eq!(raw.len(), 3, "raw history returns every stored sample");

        let minute = history(
            &client,
            &history_query(0, 120_000, history_query::Resolution::Minute),
        )
        .await;
        assert_eq!(
            minute
                .iter()
                .map(|entry| entry.timestamp_ms)
                .collect::<Vec<_>>(),
            vec![0, 60_000],
            "minute history returns one entry per bucket, not the raw samples"
        );

        // A single-millisecond range is a legitimate query, so the reversed
        // check has to reject only a range that really runs backwards.
        let single = history(
            &client,
            &history_query(1_000, 1_000, history_query::Resolution::Raw),
        )
        .await;
        assert_eq!(single.len(), 1, "from == to is a valid one-sample range");

        let reversed = ask(
            &client,
            request::Body::QueryHistory(HistoryQuery {
                from_ms: 2,
                to_ms: 1,
                resolution: history_query::Resolution::Raw as i32,
                protocol: Some(current_version()),
            }),
        )
        .await;
        assert_eq!(error_code(&reversed), "invalid_range");

        client.close();
        rack.shutdown().await;
    }

    #[tokio::test]
    async fn history_longer_than_one_page_is_streamed_to_the_end() {
        // Paging advances on the last timestamp of each page. A cursor that
        // stops advancing silently truncates the answer to its first page,
        // which reads as a machine with far less history than it has.
        let rack = TestRack::start().await;
        let samples: Vec<_> = (1..=(HISTORY_PAGE_SIZE + 44))
            .map(|index| sample(i64::try_from(index).unwrap_or_default() * 1_000))
            .collect();
        rack.runtime
            .store
            .lock()
            .await
            .insert_batch(&samples)
            .unwrap_or_else(|error| panic!("{error}"));
        let client = rack.connect_authorized(PeerPermissions::default()).await;

        let served = history(
            &client,
            &history_query(0, i64::MAX, history_query::Resolution::Raw),
        )
        .await;

        assert_eq!(served.len(), samples.len());

        client.close();
        rack.shutdown().await;
    }

    #[tokio::test]
    async fn a_completed_pairing_retires_the_lan_advertisement() {
        // The advertisement exists only while a window is open. Leaving it
        // running after the window is consumed keeps announcing a machine that
        // no longer accepts pairing.
        let rack = TestRack::start().await;
        let client = rack.connect().await;
        let bundle = rack
            .runtime
            .pairing
            .lock()
            .unwrap_or_else(|error| panic!("{error}"))
            .open(
                rack.runtime.info.node_id,
                rack.endpoint.id(),
                Vec::new(),
                Vec::new(),
            );

        let paired = ask(
            &client,
            request::Body::Pair(PairRequest {
                one_time_secret: bundle.one_time_secret,
                viewer_endpoint_id: client.local_id().to_string(),
            }),
        )
        .await;
        assert!(matches!(paired, response::Body::Pair(ref pair) if pair.accepted));

        // `close` moves the generation on, so the generation that was current
        // before pairing can no longer be closed.
        assert!(
            !rack.runtime.pairing_mdns.close_if_generation(0).await,
            "consuming the window must have closed the advertisement"
        );

        client.close();
        rack.shutdown().await;
    }

    #[tokio::test]
    async fn a_departed_viewer_is_forgotten() {
        // The connection table is what `revoke_peer` tears down. An entry that
        // outlives its connection makes revoke report a teardown that never
        // happened and holds the connection alive.
        let rack = TestRack::start().await;
        let client = rack.connect_authorized(PeerPermissions::default()).await;
        let peer = client.local_id().to_string();
        ask(&client, request::Body::GetNodeInfo(current_version())).await;
        assert!(
            rack.runtime
                .active_connections
                .lock()
                .unwrap_or_else(|error| panic!("{error}"))
                .contains_key(&peer),
            "a live viewer must be tracked"
        );

        client.close();

        let deadline = std::time::Instant::now() + REPLY_TIMEOUT;
        while std::time::Instant::now() < deadline {
            if !rack
                .runtime
                .active_connections
                .lock()
                .unwrap_or_else(|error| panic!("{error}"))
                .contains_key(&peer)
            {
                rack.shutdown().await;
                return;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
        panic!("a departed viewer stayed in the connection table");
    }

    // Keep the complete authorization transition on one connection so this
    // remains an integration test of stream-level identity and permissions.
    #[tokio::test]
    async fn unknown_peer_fails_closed_then_single_use_pairing_authorizes_it() {
        let rack = TestRack::start().await;
        let client = rack.connect().await;

        let unauthorized = ask(&client, request::Body::GetNodeInfo(current_version())).await;
        assert_eq!(error_code(&unauthorized), "auth_error");

        let bundle = rack
            .runtime
            .pairing
            .lock()
            .unwrap_or_else(|error| panic!("{error}"))
            .open(
                rack.runtime.info.node_id,
                rack.endpoint.id(),
                rack.endpoint.addr().ip_addrs().copied().collect(),
                Vec::new(),
            );
        let paired = ask(
            &client,
            request::Body::Pair(PairRequest {
                one_time_secret: bundle.one_time_secret,
                viewer_endpoint_id: client.local_id().to_string(),
            }),
        )
        .await;
        assert!(matches!(paired, response::Body::Pair(ref pair) if pair.accepted));

        let authorized = ask(&client, request::Body::GetNodeInfo(current_version())).await;
        assert!(matches!(
            authorized,
            response::Body::NodeInfo(ref info) if info.display_name == "Test node"
        ));

        // iroh can briefly select a different direct candidate while its path
        // state converges under concurrent test load. Require the same-host LAN
        // result eventually without treating the first truthful snapshot as
        // the final path.
        let lan_direct = rackio_protocol::v1::ConnectionPath::LanDirect as i32;
        let mut connection_path =
            ask(&client, request::Body::GetConnectionPath(current_version())).await;
        for _ in 0..40 {
            if matches!(
                connection_path,
                response::Body::ConnectionPath(ref details) if details.path == lan_direct
            ) {
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
            connection_path =
                ask(&client, request::Body::GetConnectionPath(current_version())).await;
        }
        assert!(matches!(
            connection_path,
            response::Body::ConnectionPath(ref details) if details.path == lan_direct
        ));

        let incompatible_history = ask(
            &client,
            request::Body::QueryHistory(HistoryQuery {
                from_ms: 0,
                to_ms: 1,
                resolution: history_query::Resolution::Raw as i32,
                protocol: Some(ProtocolVersion { major: 2, minor: 0 }),
            }),
        )
        .await;
        assert_eq!(error_code(&incompatible_history), "incompatible");

        rack.runtime
            .peers
            .authorize(
                client.local_id(),
                PeerPermissions {
                    read_metrics: true,
                    read_history: false,
                },
            )
            .unwrap_or_else(|error| panic!("{error}"));
        let denied_history = ask(
            &client,
            request::Body::QueryHistory(HistoryQuery {
                from_ms: 0,
                to_ms: 1,
                resolution: history_query::Resolution::Raw as i32,
                protocol: Some(current_version()),
            }),
        )
        .await;
        assert_eq!(error_code(&denied_history), "permission_denied");

        assert!(
            rack.runtime
                .revoke_peer(&client.local_id().to_string())
                .unwrap_or_else(|error| panic!("{error}"))
        );
        let after_revoke = tokio::time::timeout(
            REPLY_TIMEOUT,
            client.request(&Request {
                body: Some(request::Body::GetNodeInfo(current_version())),
            }),
        )
        .await
        .unwrap_or_else(|_| panic!("a revoked viewer must be torn down, not left waiting"));
        assert!(
            after_revoke.is_err(),
            "revoking a peer must tear its connection down"
        );

        rack.shutdown().await;
    }
}
