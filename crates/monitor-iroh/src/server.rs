use std::sync::Arc;

use chrono::Utc;
use iroh::{Endpoint, EndpointId, endpoint::Connection};
use thiserror::Error;
use tokio::sync::{Mutex, RwLock, watch};
use tray_monitor_core::{
    HealthSnapshot, HistoryResolution, MetricSample, MetricStore, NodeInfo, StoreError,
};
use tray_monitor_protocol::{
    FrameError, compatible, read_frame,
    v1::{
        ErrorResponse, Heartbeat, HistoryQuery, PairResponse, Request, Response, StreamComplete,
        history_query::Resolution, request, response,
    },
    write_frame,
};

use crate::{
    PairingError, PairingManager, PeerPermissions, PeerRegistry, classify_connection, protocol,
};

const HISTORY_PAGE_SIZE: usize = 256;

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
}

pub struct NodeRuntime {
    pub info: NodeInfo,
    pub health: RwLock<HealthSnapshot>,
    pub latest: watch::Receiver<Option<MetricSample>>,
    pub store: Mutex<MetricStore>,
    pub pairing: std::sync::Mutex<PairingManager>,
    pub peers: PeerRegistry,
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
                    handle_connection(connection, local_id, runtime).await
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
                let request: Request = read_frame(&mut receive).await?;
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
    pair: &tray_monitor_protocol::v1::PairRequest,
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
    let verified = runtime
        .pairing
        .lock()
        .map_err(|_| ServerError::PairingStateUnavailable)?
        .verify_and_consume(&pair.one_time_secret);
    if verified.is_err() {
        write_error(send, "pairing_rejected", "pairing request was rejected").await?;
        return Ok(());
    }
    runtime
        .peers
        .authorize(remote_id, PeerPermissions::default())?;
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
                let health = runtime.health.read().await;
                write_response(send, response::Body::Health(protocol::health(&health))).await?;
            }
        }
        request::Body::GetConnectionPath(version) => {
            if require_permission(send, permissions.read_metrics, "read_metrics").await?
                && version_is_compatible(send, &version).await?
            {
                let details = classify_connection(connection);
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
    version: &tray_monitor_protocol::v1::ProtocolVersion,
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
    use std::sync::Arc;

    use tokio::sync::{RwLock, watch};
    use tray_monitor_core::{
        HealthSnapshot, MetricStore, NodeInfo, NodeState, ProtocolVersion as CoreProtocolVersion,
    };
    use tray_monitor_protocol::{
        current_version,
        v1::{
            HistoryQuery, PairRequest, ProtocolVersion, Request, history_query, request, response,
        },
    };
    use uuid::Uuid;

    use crate::{ClientConnection, EndpointConfig, PairingManager, PeerRegistry, bind_endpoint};

    use super::{NodeRuntime, RemoteServer};

    // Keep the complete authorization transition on one connection so this
    // remains an integration test of stream-level identity and permissions.
    #[allow(clippy::too_many_lines)]
    #[tokio::test]
    async fn unknown_peer_fails_closed_then_single_use_pairing_authorizes_it() {
        let directory = tempfile::tempdir().unwrap_or_else(|error| panic!("{error}"));
        let server_endpoint =
            bind_endpoint(iroh::SecretKey::generate(), &EndpointConfig::default())
                .await
                .unwrap_or_else(|error| panic!("{error}"));
        let client_endpoint =
            bind_endpoint(iroh::SecretKey::generate(), &EndpointConfig::default())
                .await
                .unwrap_or_else(|error| panic!("{error}"));
        let (_, latest) = watch::channel(None);
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
            store: tokio::sync::Mutex::new(
                MetricStore::in_memory().unwrap_or_else(|error| panic!("{error}")),
            ),
            pairing: std::sync::Mutex::new(PairingManager::default()),
            peers: PeerRegistry::load(directory.path().join("peers.json"))
                .unwrap_or_else(|error| panic!("{error}")),
        });
        let server = RemoteServer::new(server_endpoint.clone(), Arc::clone(&runtime));
        let server_task = tokio::spawn(server.run());
        let client = ClientConnection::connect(client_endpoint, server_endpoint.addr())
            .await
            .unwrap_or_else(|error| panic!("{error}"));

        let unauthorized = client
            .request(&Request {
                body: Some(request::Body::GetNodeInfo(current_version())),
            })
            .await
            .unwrap_or_else(|error| panic!("{error}"));
        assert!(matches!(
            unauthorized.body,
            Some(response::Body::Error(ref error)) if error.code == "auth_error"
        ));

        let bundle = runtime
            .pairing
            .lock()
            .unwrap_or_else(|error| panic!("{error}"))
            .open(
                runtime.info.node_id,
                server_endpoint.id(),
                server_endpoint.addr().ip_addrs().copied().collect(),
                Vec::new(),
            );
        let paired = client
            .request(&Request {
                body: Some(request::Body::Pair(PairRequest {
                    one_time_secret: bundle.one_time_secret,
                    viewer_endpoint_id: client.local_id().to_string(),
                })),
            })
            .await
            .unwrap_or_else(|error| panic!("{error}"));
        assert!(matches!(
            paired.body,
            Some(response::Body::Pair(ref response)) if response.accepted
        ));

        let authorized = client
            .request(&Request {
                body: Some(request::Body::GetNodeInfo(current_version())),
            })
            .await
            .unwrap_or_else(|error| panic!("{error}"));
        assert!(matches!(
            authorized.body,
            Some(response::Body::NodeInfo(ref info)) if info.display_name == "Test node"
        ));

        let connection_path = client
            .request(&Request {
                body: Some(request::Body::GetConnectionPath(current_version())),
            })
            .await
            .unwrap_or_else(|error| panic!("{error}"));
        assert!(matches!(
            connection_path.body,
            Some(response::Body::ConnectionPath(ref details))
                if details.path == tray_monitor_protocol::v1::ConnectionPath::LanDirect as i32
        ));

        let incompatible_history = client
            .request(&Request {
                body: Some(request::Body::QueryHistory(HistoryQuery {
                    from_ms: 0,
                    to_ms: 1,
                    resolution: history_query::Resolution::Raw as i32,
                    protocol: Some(ProtocolVersion { major: 2, minor: 0 }),
                })),
            })
            .await
            .unwrap_or_else(|error| panic!("{error}"));
        assert!(matches!(
            incompatible_history.body,
            Some(response::Body::Error(ref error)) if error.code == "incompatible"
        ));

        runtime
            .peers
            .authorize(
                client.local_id(),
                crate::PeerPermissions {
                    read_metrics: true,
                    read_history: false,
                },
            )
            .unwrap_or_else(|error| panic!("{error}"));
        let denied_history = client
            .request(&Request {
                body: Some(request::Body::QueryHistory(HistoryQuery {
                    from_ms: 0,
                    to_ms: 1,
                    resolution: history_query::Resolution::Raw as i32,
                    protocol: Some(current_version()),
                })),
            })
            .await
            .unwrap_or_else(|error| panic!("{error}"));
        assert!(matches!(
            denied_history.body,
            Some(response::Body::Error(ref error)) if error.code == "permission_denied"
        ));

        client.close().await;
        server_endpoint.close().await;
        server_task.abort();
    }
}
