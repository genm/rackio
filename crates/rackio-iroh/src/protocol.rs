use rackio_core as core;
use rackio_protocol::v1 as wire;

pub(crate) fn metric_sample(sample: &core::MetricSample) -> wire::MetricSample {
    wire::MetricSample {
        timestamp_ms: sample.timestamp_ms,
        sequence: sample.sequence,
        cpu_percent: sample.cpu_percent,
        memory_used_bytes: sample.memory_used_bytes,
        memory_total_bytes: sample.memory_total_bytes,
        swap_used_bytes: sample.swap_used_bytes,
        swap_total_bytes: sample.swap_total_bytes,
        disks: sample
            .disks
            .iter()
            .map(|disk| wire::DiskMetric {
                mount: disk.mount.clone(),
                total_bytes: disk.total_bytes,
                used_bytes: disk.used_bytes,
            })
            .collect(),
        network: sample.network.as_ref().map(|network| wire::NetworkMetric {
            received_bytes_per_second: network.received_bytes_per_second,
            sent_bytes_per_second: network.sent_bytes_per_second,
        }),
        uptime_seconds: sample.uptime_seconds,
        errors: sample
            .errors
            .iter()
            .map(|error| wire::CollectorError {
                source: error.source.clone(),
                kind: capability_state(error.kind),
                message: error.message.clone(),
            })
            .collect(),
    }
}

pub(crate) fn node_info(info: &core::NodeInfo, endpoint_id: iroh::EndpointId) -> wire::NodeInfo {
    wire::NodeInfo {
        node_id: info.node_id.to_string(),
        endpoint_id: endpoint_id.to_string(),
        display_name: info.display_name.clone(),
        os: info.os.clone(),
        architecture: info.architecture.clone(),
        agent_version: info.agent_version.clone(),
        protocol: Some(wire::ProtocolVersion {
            major: info.protocol.major,
            minor: info.protocol.minor,
        }),
        capabilities: info
            .capabilities
            .iter()
            .map(|capability| wire::MetricCapability {
                name: capability.name.clone(),
                state: capability_state(capability.state),
                detail: capability.detail.clone(),
            })
            .collect(),
    }
}

pub(crate) fn health(health: &core::HealthSnapshot) -> wire::HealthSnapshot {
    wire::HealthSnapshot {
        state: node_state(health.state),
        collector_degraded: health.collector_degraded,
        storage_degraded: health.storage_degraded,
        remote_listener_degraded: health.remote_listener_degraded,
        details: health.details.clone(),
    }
}

pub(crate) const fn connection_details(
    path: core::ConnectionPath,
    rtt_ms: u64,
) -> wire::ConnectionDetails {
    wire::ConnectionDetails {
        path: match path {
            core::ConnectionPath::LanDirect => wire::ConnectionPath::LanDirect as i32,
            core::ConnectionPath::WanDirect => wire::ConnectionPath::WanDirect as i32,
            core::ConnectionPath::Relayed => wire::ConnectionPath::Relayed as i32,
            core::ConnectionPath::Unknown => wire::ConnectionPath::Unknown as i32,
        },
        rtt_ms,
    }
}

const fn capability_state(state: core::CapabilityState) -> i32 {
    match state {
        core::CapabilityState::Supported => wire::CapabilityState::Supported as i32,
        core::CapabilityState::Unsupported => wire::CapabilityState::Unsupported as i32,
        core::CapabilityState::PermissionDenied => wire::CapabilityState::PermissionDenied as i32,
    }
}

const fn node_state(state: core::NodeState) -> i32 {
    match state {
        core::NodeState::Healthy => wire::NodeState::Healthy as i32,
        core::NodeState::Warning => wire::NodeState::Warning as i32,
        core::NodeState::Critical => wire::NodeState::Critical as i32,
        core::NodeState::Stale => wire::NodeState::Stale as i32,
        core::NodeState::Offline => wire::NodeState::Offline as i32,
        core::NodeState::AuthError => wire::NodeState::AuthError as i32,
        core::NodeState::Incompatible => wire::NodeState::Incompatible as i32,
        core::NodeState::Degraded => wire::NodeState::Degraded as i32,
    }
}
