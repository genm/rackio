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
        temperature: sample
            .temperature
            .as_ref()
            .map(|temperature| wire::TemperatureMetric {
                label: temperature.label.clone(),
                celsius: temperature.celsius,
                critical_celsius: temperature.critical_celsius,
                sensor_count: temperature.sensor_count,
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

#[cfg(test)]
mod tests {
    use rackio_core as core;
    use rackio_protocol::v1 as wire;

    use super::{capability_state, connection_details, health, metric_sample, node_state};

    // The numbers are asserted as literals against `proto/rackio.proto`, not
    // against the generated enum: a viewer on one release reads these bytes
    // from an agent on another, so renumbering is a wire break rather than a
    // rename. Zero is deliberately absent — it is the protobuf "unspecified"
    // value, and no domain state may translate into it.
    #[test]
    fn capability_states_translate_to_their_wire_numbers() {
        for (state, expected) in [
            (core::CapabilityState::Supported, 1),
            (core::CapabilityState::Unsupported, 2),
            (core::CapabilityState::PermissionDenied, 3),
        ] {
            assert_eq!(capability_state(state), expected, "{state:?}");
        }
    }

    #[test]
    fn node_states_translate_to_their_wire_numbers() {
        for (state, expected) in [
            (core::NodeState::Healthy, 1),
            (core::NodeState::Warning, 2),
            (core::NodeState::Critical, 3),
            (core::NodeState::Stale, 4),
            (core::NodeState::Offline, 5),
            (core::NodeState::AuthError, 6),
            (core::NodeState::Incompatible, 7),
            (core::NodeState::Degraded, 8),
        ] {
            assert_eq!(node_state(state), expected, "{state:?}");
        }
    }

    #[test]
    fn connection_paths_translate_to_their_wire_numbers() {
        for (path, expected) in [
            (core::ConnectionPath::LanDirect, 1),
            (core::ConnectionPath::WanDirect, 2),
            (core::ConnectionPath::Relayed, 3),
            (core::ConnectionPath::Unknown, 4),
        ] {
            let details = connection_details(path, 12);
            assert_eq!(details.path, expected, "{path:?}");
            assert_eq!(details.rtt_ms, 12);
        }
    }

    #[test]
    fn a_metric_sample_keeps_every_reading_it_carried() {
        let sample = core::MetricSample {
            timestamp_ms: 1_700_000_000_000,
            sequence: 42,
            cpu_percent: Some(37.5),
            memory_used_bytes: Some(1_024),
            memory_total_bytes: Some(4_096),
            swap_used_bytes: Some(8),
            swap_total_bytes: Some(16),
            disks: vec![core::DiskMetric {
                mount: String::from("/"),
                total_bytes: 500,
                used_bytes: 200,
            }],
            network: Some(core::NetworkMetric {
                received_bytes_per_second: 10,
                sent_bytes_per_second: 20,
            }),
            temperature: Some(core::TemperatureMetric {
                label: String::from("CPU die"),
                celsius: 48.5,
                critical_celsius: Some(100.0),
                sensor_count: 41,
            }),
            uptime_seconds: 99,
            errors: vec![core::CollectorError {
                source: String::from("disk"),
                kind: core::CapabilityState::PermissionDenied,
                message: String::from("denied"),
            }],
        };

        let wire = metric_sample(&sample);

        assert_eq!(wire.timestamp_ms, 1_700_000_000_000);
        assert_eq!(wire.sequence, 42);
        assert_eq!(wire.cpu_percent, Some(37.5));
        assert_eq!(wire.memory_used_bytes, Some(1_024));
        assert_eq!(wire.memory_total_bytes, Some(4_096));
        assert_eq!(wire.swap_used_bytes, Some(8));
        assert_eq!(wire.swap_total_bytes, Some(16));
        assert_eq!(wire.uptime_seconds, 99);
        assert_eq!(
            wire.disks,
            vec![wire::DiskMetric {
                mount: String::from("/"),
                total_bytes: 500,
                used_bytes: 200,
            }]
        );
        assert_eq!(
            wire.network,
            Some(wire::NetworkMetric {
                received_bytes_per_second: 10,
                sent_bytes_per_second: 20,
            })
        );
        assert_eq!(
            wire.errors,
            vec![wire::CollectorError {
                source: String::from("disk"),
                kind: 3,
                message: String::from("denied"),
            }],
            "a collector error must keep its capability state"
        );
        assert_eq!(
            wire.temperature,
            Some(wire::TemperatureMetric {
                label: String::from("CPU die"),
                celsius: 48.5,
                critical_celsius: Some(100.0),
                sensor_count: 41,
            }),
            "a reading keeps its label, value, hardware threshold and sensor count"
        );
    }

    #[test]
    fn an_unreadable_metric_stays_absent_on_the_wire() {
        // Sending 0 for a reading the collector could not take would present an
        // unreadable source as an idle one on the viewer.
        let sample = core::MetricSample {
            timestamp_ms: 1,
            sequence: 1,
            cpu_percent: None,
            memory_used_bytes: None,
            memory_total_bytes: None,
            swap_used_bytes: None,
            swap_total_bytes: None,
            disks: Vec::new(),
            network: None,
            temperature: None,
            uptime_seconds: 0,
            errors: Vec::new(),
        };

        let wire = metric_sample(&sample);

        assert_eq!(wire.cpu_percent, None);
        assert_eq!(wire.memory_used_bytes, None);
        assert_eq!(wire.network, None);
        assert_eq!(
            wire.temperature, None,
            "a host with no readable sensor sends no reading at all"
        );
    }

    #[test]
    fn a_health_snapshot_keeps_every_degraded_flag() {
        let snapshot = core::HealthSnapshot {
            state: core::NodeState::Degraded,
            collector_degraded: true,
            storage_degraded: false,
            remote_listener_degraded: true,
            details: vec![String::from("storage is read-only")],
        };

        let wire = health(&snapshot);

        assert_eq!(wire.state, 8);
        assert!(wire.collector_degraded);
        assert!(!wire.storage_degraded);
        assert!(wire.remote_listener_degraded);
        assert_eq!(wire.details, vec![String::from("storage is read-only")]);
    }
}
