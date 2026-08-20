//! Projection from daemon-owned JSON into the frontend's stable view models.

/// Project one daemon-reported `MetricSample` (`snake_case`) into the
/// camelCase `HistoryPoint` shape the frontend expects.
///
/// The peer's minute buckets report the fullest disk and the hottest sensor
/// as a single averaged reading, carried as a one-element `disks` array so
/// the minute resolution can reuse the raw resolution's wire shape — see
/// `MetricStore::query_minute`.
pub(crate) fn history_point_from_sample(sample: &serde_json::Value) -> serde_json::Value {
    let worst_disk = sample
        .get("disks")
        .and_then(serde_json::Value::as_array)
        .and_then(|disks| disks.first());
    serde_json::json!({
        "timestampMs": sample.get("timestamp_ms"),
        "cpuPercent": sample.get("cpu_percent"),
        "memoryUsedBytes": sample.get("memory_used_bytes"),
        "memoryTotalBytes": sample.get("memory_total_bytes"),
        "diskUsedBytes": worst_disk.and_then(|disk| disk.get("used_bytes")),
        "diskTotalBytes": worst_disk.and_then(|disk| disk.get("total_bytes")),
        "temperatureCelsius": sample
            .get("temperature")
            .and_then(|temperature| temperature.get("celsius")),
        "networkReceivedBytesPerSecond": sample
            .get("network")
            .and_then(|network| network.get("received_bytes_per_second")),
        "networkSentBytesPerSecond": sample
            .get("network")
            .and_then(|network| network.get("sent_bytes_per_second")),
    })
}

/// Convert a daemon-side trend sample to the camelCase point shape the
/// dashboard shares with its 24-hour history query.
fn trend_point_json(sample: &serde_json::Value) -> serde_json::Value {
    serde_json::json!({
        "timestampMs": sample.get("timestamp_ms"),
        "cpuPercent": sample.get("cpu_percent"),
        "memoryUsedBytes": sample.get("memory_used_bytes"),
        "memoryTotalBytes": sample.get("memory_total_bytes"),
        "diskUsedBytes": sample.get("disk_used_bytes"),
        "diskTotalBytes": sample.get("disk_total_bytes"),
        "temperatureCelsius": sample.get("temperature_celsius"),
        "networkReceivedBytesPerSecond": sample.get("network_received_bytes_per_second"),
        "networkSentBytesPerSecond": sample.get("network_sent_bytes_per_second"),
        "rttMs": sample.get("rtt_ms"),
    })
}

pub(crate) fn machine_json(
    source: &serde_json::Value,
    node_state: &str,
    path: &str,
    rtt_ms: Option<u64>,
    last_seen_ms: Option<i64>,
    trend: &[serde_json::Value],
    detail: Option<&str>,
) -> Result<serde_json::Value, String> {
    let node = source
        .get("node")
        .ok_or_else(|| String::from("machine response did not contain node info"))?;
    let latest = source.get("latest").unwrap_or(&serde_json::Value::Null);
    let node_id = node
        .get("node_id")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| String::from("machine node ID is missing"))?;
    let node_name = node
        .get("display_name")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| String::from("machine node name is missing"))?;
    let disks = latest
        .get("disks")
        .and_then(serde_json::Value::as_array)
        .cloned()
        .unwrap_or_default();
    let worst_disk = disks
        .iter()
        .filter_map(|disk| {
            let used = disk.get("used_bytes").and_then(serde_json::Value::as_u64)?;
            let total = disk
                .get("total_bytes")
                .and_then(serde_json::Value::as_u64)?;
            (total > 0).then_some((used, total))
        })
        .max_by(|(left_used, left_total), (right_used, right_total)| {
            u128::from(*left_used)
                .saturating_mul(u128::from(*right_total))
                .cmp(&u128::from(*right_used).saturating_mul(u128::from(*left_total)))
        });
    let (disk_used, disk_total) = worst_disk.unzip();
    let cpu = latest
        .get("cpu_percent")
        .and_then(serde_json::Value::as_f64);
    // Absent on a host with no readable sensor. Carried through as an absent
    // field rather than as a zero so the viewer renders "—" instead of a
    // frozen machine.
    let temperature = latest
        .get("temperature")
        .filter(|value| !value.is_null())
        .map(|temperature| {
            serde_json::json!({
                "label": temperature.get("label"),
                "celsius": temperature.get("celsius"),
                "criticalCelsius": temperature.get("critical_celsius"),
                "sensorCount": temperature.get("sensor_count"),
            })
        });
    Ok(serde_json::json!({
        "id": node_id,
        "endpointId": source.get("endpoint_id"),
        "name": node_name,
        "os": format!(
            "{} · {}",
            node.get("os").and_then(serde_json::Value::as_str).unwrap_or("unknown"),
            node.get("architecture").and_then(serde_json::Value::as_str).unwrap_or("unknown")
        ),
        "state": node_state,
        "path": path,
        "cpuPercent": cpu,
        "memoryUsedBytes": latest.get("memory_used_bytes"),
        "memoryTotalBytes": latest.get("memory_total_bytes"),
        "diskUsedBytes": disk_used,
        "diskTotalBytes": disk_total,
        "temperature": temperature,
        "networkReceivedBytesPerSecond": latest
            .get("network")
            .and_then(|network| network.get("received_bytes_per_second")),
        "networkSentBytesPerSecond": latest
            .get("network")
            .and_then(|network| network.get("sent_bytes_per_second")),
        "rttMs": rtt_ms,
        "lastSeenMs": last_seen_ms,
        "trend": trend.iter().map(trend_point_json).collect::<Vec<_>>(),
        "detail": detail,
    }))
}

#[cfg(test)]
mod tests {
    use super::{history_point_from_sample, machine_json};

    /// The daemon speaks `snake_case` and the viewer `camelCase`, so this boundary
    /// is where a temperature would silently disappear — or, worse, arrive as a
    /// zero on a machine that never reported one.
    #[test]
    fn a_machine_carries_its_hottest_sensor_to_the_viewer_or_nothing_at_all() {
        let source = serde_json::json!({
            "node": {
                "node_id": "id",
                "display_name": "Server",
                "os": "linux",
                "architecture": "x86_64",
            },
            "latest": {
                "cpu_percent": 12.0,
                "temperature": {
                    "label": "Package id 0",
                    "celsius": 61.5,
                    "critical_celsius": 100.0,
                    "sensor_count": 7,
                },
            },
        });

        let machine = machine_json(&source, "healthy", "lan_direct", None, None, &[], None)
            .unwrap_or_else(|error| panic!("{error}"));
        assert_eq!(
            machine.get("temperature"),
            Some(&serde_json::json!({
                "label": "Package id 0",
                "celsius": 61.5,
                "criticalCelsius": 100.0,
                "sensorCount": 7,
            }))
        );

        // A host with no readable sensor must not acquire one here.
        let sensorless = serde_json::json!({
            "node": { "node_id": "id", "display_name": "Server" },
            "latest": { "cpu_percent": 12.0 },
        });
        let machine = machine_json(&sensorless, "healthy", "lan_direct", None, None, &[], None)
            .unwrap_or_else(|error| panic!("{error}"));
        assert_eq!(machine.get("temperature"), Some(&serde_json::Value::Null));
    }

    #[test]
    fn trend_samples_reach_the_viewer_as_timestamped_points() {
        let source = serde_json::json!({
            "node": { "node_id": "id", "display_name": "Server" },
            "latest": { "cpu_percent": 12.0 },
        });
        let trend = [serde_json::json!({
            "timestamp_ms": 1_750_000_000_000_i64,
            "cpu_percent": 12.5,
            "memory_used_bytes": 3_000,
            "memory_total_bytes": 4_000,
            "disk_used_bytes": 90,
            "disk_total_bytes": 100,
            "temperature_celsius": 61.5,
            "network_received_bytes_per_second": 2_048,
            "network_sent_bytes_per_second": 512,
            "rtt_ms": 8,
        })];

        let machine = machine_json(&source, "healthy", "lan_direct", None, None, &trend, None)
            .unwrap_or_else(|error| panic!("{error}"));
        assert_eq!(
            machine.get("trend"),
            Some(&serde_json::json!([{
                "timestampMs": 1_750_000_000_000_i64,
                "cpuPercent": 12.5,
                "memoryUsedBytes": 3_000,
                "memoryTotalBytes": 4_000,
                "diskUsedBytes": 90,
                "diskTotalBytes": 100,
                "temperatureCelsius": 61.5,
                "networkReceivedBytesPerSecond": 2_048,
                "networkSentBytesPerSecond": 512,
                "rttMs": 8,
            }]))
        );
    }

    #[test]
    fn history_point_carries_the_minute_averaged_disk_and_temperature() {
        let sample = serde_json::json!({
            "timestamp_ms": 60_000,
            "cpu_percent": 20.0,
            "memory_used_bytes": 100,
            "memory_total_bytes": 200,
            "disks": [{"mount": "(minute average)", "used_bytes": 25, "total_bytes": 100}],
            "temperature": {"label": "(minute average)", "celsius": 50.0, "sensor_count": 0},
            "network": {"received_bytes_per_second": 10, "sent_bytes_per_second": 20},
        });

        let point = history_point_from_sample(&sample);
        assert_eq!(point["timestampMs"], 60_000);
        assert_eq!(point["diskUsedBytes"], 25);
        assert_eq!(point["diskTotalBytes"], 100);
        assert_eq!(point["temperatureCelsius"], 50.0);
        assert_eq!(point["networkReceivedBytesPerSecond"], 10);
    }

    #[test]
    fn history_point_reports_no_disk_or_temperature_when_the_minute_had_neither() {
        let sample = serde_json::json!({
            "timestamp_ms": 60_000,
            "cpu_percent": 20.0,
            "memory_used_bytes": 100,
            "memory_total_bytes": 200,
            "disks": [],
        });

        let point = history_point_from_sample(&sample);
        assert!(point["diskUsedBytes"].is_null());
        assert!(point["diskTotalBytes"].is_null());
        assert!(point["temperatureCelsius"].is_null());
    }
}
