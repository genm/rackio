//! The read-only protocol client: connection establishment with identity
//! pinning, single-request helpers, and decoding wire responses into domain
//! types. The inverse direction (domain to wire) lives with the server in
//! `rackio-iroh`.

use std::time::Duration;

use rackio_core::{
    CapabilityState, CollectorError, ConnectionPath, DiskMetric, HealthSnapshot, MetricCapability,
    MetricSample, NetworkMetric, NodeInfo, NodeState, ProtocolVersion, TemperatureMetric,
};
use rackio_iroh::{ClientConnection, TransportError};
use rackio_protocol::{
    current_version,
    v1::{Request, request, response},
};
use uuid::Uuid;

use super::{RemoteFleetError, registry::RemoteMachineRecord};

pub(super) const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
pub(super) const REQUEST_TIMEOUT: Duration = Duration::from_secs(8);

pub(super) async fn connect_record(
    endpoint: iroh::Endpoint,
    record: &RemoteMachineRecord,
) -> Result<ClientConnection, RemoteFleetError> {
    let address = record.endpoint_addr()?;
    let client = tokio::time::timeout(
        CONNECT_TIMEOUT,
        ClientConnection::connect(endpoint, address),
    )
    .await
    .map_err(|_| RemoteFleetError::Timeout("connect"))??;
    if client.remote_id().to_string() != record.endpoint_id {
        client.close();
        return Err(RemoteFleetError::IdentityMismatch);
    }
    let node = get_node_info(&client).await?;
    if node.node_id != record.node.node_id {
        client.close();
        return Err(RemoteFleetError::IdentityMismatch);
    }
    Ok(client)
}

pub(super) async fn request(
    client: &ClientConnection,
    request: Request,
    operation: &'static str,
) -> Result<rackio_protocol::v1::Response, RemoteFleetError> {
    let response = tokio::time::timeout(REQUEST_TIMEOUT, client.request(&request))
        .await
        .map_err(|_| RemoteFleetError::Timeout(operation))?
        .map_err(RemoteFleetError::from)?;
    if let Some(response::Body::Error(error)) = &response.body {
        return Err(RemoteFleetError::Transport(TransportError::Remote {
            code: error.code.clone(),
            message: error.message.clone(),
        }));
    }
    Ok(response)
}

pub(super) async fn get_node_info(client: &ClientConnection) -> Result<NodeInfo, RemoteFleetError> {
    let response = request(
        client,
        Request {
            body: Some(request::Body::GetNodeInfo(current_version())),
        },
        "node info",
    )
    .await?;
    let Some(response::Body::NodeInfo(info)) = response.body else {
        return Err(RemoteFleetError::UnexpectedResponse("node info"));
    };
    node_info(info)
}

pub(super) async fn get_health(
    client: &ClientConnection,
) -> Result<HealthSnapshot, RemoteFleetError> {
    let response = request(
        client,
        Request {
            body: Some(request::Body::GetHealth(current_version())),
        },
        "health",
    )
    .await?;
    let Some(response::Body::Health(health)) = response.body else {
        return Err(RemoteFleetError::UnexpectedResponse("health"));
    };
    Ok(HealthSnapshot {
        state: node_state(health.state),
        collector_degraded: health.collector_degraded,
        storage_degraded: health.storage_degraded,
        remote_listener_degraded: health.remote_listener_degraded,
        details: health.details,
    })
}

pub(super) async fn get_connection_path(
    client: &ClientConnection,
) -> Result<(ConnectionPath, u64), RemoteFleetError> {
    let response = request(
        client,
        Request {
            body: Some(request::Body::GetConnectionPath(current_version())),
        },
        "connection path",
    )
    .await?;
    let Some(response::Body::ConnectionPath(details)) = response.body else {
        return Err(RemoteFleetError::UnexpectedResponse("connection path"));
    };
    Ok((connection_path(details.path), details.rtt_ms))
}

fn node_info(info: rackio_protocol::v1::NodeInfo) -> Result<NodeInfo, RemoteFleetError> {
    let protocol = info
        .protocol
        .ok_or(RemoteFleetError::UnexpectedResponse("protocol version"))?;
    Ok(NodeInfo {
        node_id: Uuid::parse_str(&info.node_id).map_err(|_| RemoteFleetError::IdentityMismatch)?,
        display_name: info.display_name,
        os: info.os,
        architecture: info.architecture,
        agent_version: info.agent_version,
        protocol: ProtocolVersion {
            major: protocol.major,
            minor: protocol.minor,
        },
        capabilities: info
            .capabilities
            .into_iter()
            .map(|capability| MetricCapability {
                name: capability.name,
                state: capability_state(capability.state),
                detail: capability.detail,
            })
            .collect(),
    })
}

pub(super) fn metric_sample(sample: rackio_protocol::v1::MetricSample) -> MetricSample {
    MetricSample {
        timestamp_ms: sample.timestamp_ms,
        sequence: sample.sequence,
        cpu_percent: sample.cpu_percent,
        memory_used_bytes: sample.memory_used_bytes,
        memory_total_bytes: sample.memory_total_bytes,
        swap_used_bytes: sample.swap_used_bytes,
        swap_total_bytes: sample.swap_total_bytes,
        disks: sample
            .disks
            .into_iter()
            .map(|disk| DiskMetric {
                mount: disk.mount,
                total_bytes: disk.total_bytes,
                used_bytes: disk.used_bytes,
            })
            .collect(),
        network: sample.network.map(|network| NetworkMetric {
            received_bytes_per_second: network.received_bytes_per_second,
            sent_bytes_per_second: network.sent_bytes_per_second,
        }),
        temperature: sample.temperature.map(|temperature| TemperatureMetric {
            label: temperature.label,
            celsius: temperature.celsius,
            critical_celsius: temperature.critical_celsius,
            sensor_count: temperature.sensor_count,
        }),
        uptime_seconds: sample.uptime_seconds,
        errors: sample
            .errors
            .into_iter()
            .map(|error| CollectorError {
                source: error.source,
                kind: capability_state(error.kind),
                message: error.message,
            })
            .collect(),
    }
}

fn capability_state(state: i32) -> CapabilityState {
    match rackio_protocol::v1::CapabilityState::try_from(state) {
        Ok(rackio_protocol::v1::CapabilityState::Supported) => CapabilityState::Supported,
        Ok(rackio_protocol::v1::CapabilityState::PermissionDenied) => {
            CapabilityState::PermissionDenied
        }
        _ => CapabilityState::Unsupported,
    }
}

fn node_state(state: i32) -> NodeState {
    match rackio_protocol::v1::NodeState::try_from(state) {
        Ok(rackio_protocol::v1::NodeState::Healthy) => NodeState::Healthy,
        Ok(rackio_protocol::v1::NodeState::Warning) => NodeState::Warning,
        Ok(rackio_protocol::v1::NodeState::Critical) => NodeState::Critical,
        Ok(rackio_protocol::v1::NodeState::Stale) => NodeState::Stale,
        Ok(rackio_protocol::v1::NodeState::Offline) => NodeState::Offline,
        Ok(rackio_protocol::v1::NodeState::AuthError) => NodeState::AuthError,
        Ok(rackio_protocol::v1::NodeState::Incompatible) => NodeState::Incompatible,
        _ => NodeState::Degraded,
    }
}

fn connection_path(path: i32) -> ConnectionPath {
    match rackio_protocol::v1::ConnectionPath::try_from(path) {
        Ok(rackio_protocol::v1::ConnectionPath::LanDirect) => ConnectionPath::LanDirect,
        Ok(rackio_protocol::v1::ConnectionPath::WanDirect) => ConnectionPath::WanDirect,
        Ok(rackio_protocol::v1::ConnectionPath::Relayed) => ConnectionPath::Relayed,
        _ => ConnectionPath::Unknown,
    }
}
