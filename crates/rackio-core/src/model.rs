use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NodeState {
    Healthy,
    Warning,
    Critical,
    Stale,
    Offline,
    AuthError,
    Incompatible,
    Degraded,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConnectionPath {
    LanDirect,
    WanDirect,
    Relayed,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CapabilityState {
    Supported,
    Unsupported,
    PermissionDenied,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MetricCapability {
    pub name: String,
    pub state: CapabilityState,
    pub detail: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProtocolVersion {
    pub major: u32,
    pub minor: u32,
}

impl ProtocolVersion {
    pub const V1: Self = Self { major: 1, minor: 1 };

    #[must_use]
    pub const fn is_compatible_with(&self, other: &Self) -> bool {
        self.major == other.major
    }
}

#[cfg(test)]
mod protocol_version_tests {
    use super::ProtocolVersion;

    #[test]
    fn accepts_an_equal_major() {
        assert!(ProtocolVersion::V1.is_compatible_with(&ProtocolVersion::V1));
    }

    #[test]
    fn rejects_a_differing_major_in_both_directions() {
        // Fail closed: an unknown major is refused whether the peer is ahead or
        // behind, so an incompatible node reaches `NodeState::Incompatible`
        // instead of being admitted and misreporting metrics.
        let ahead = ProtocolVersion { major: 2, minor: 0 };
        let behind = ProtocolVersion { major: 0, minor: 9 };

        assert!(!ProtocolVersion::V1.is_compatible_with(&ahead));
        assert!(!ahead.is_compatible_with(&ProtocolVersion::V1));
        assert!(!ProtocolVersion::V1.is_compatible_with(&behind));
        assert!(!behind.is_compatible_with(&ProtocolVersion::V1));
    }

    #[test]
    fn ignores_a_differing_minor() {
        // Minor is additive, so a rolling upgrade must not partition the fleet.
        let newer_minor = ProtocolVersion {
            major: ProtocolVersion::V1.major,
            minor: ProtocolVersion::V1.minor + 7,
        };

        assert!(ProtocolVersion::V1.is_compatible_with(&newer_minor));
        assert!(newer_minor.is_compatible_with(&ProtocolVersion::V1));
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NodeInfo {
    pub node_id: Uuid,
    pub display_name: String,
    pub os: String,
    pub architecture: String,
    pub agent_version: String,
    pub protocol: ProtocolVersion,
    pub capabilities: Vec<MetricCapability>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DiskMetric {
    pub mount: String,
    pub total_bytes: u64,
    pub used_bytes: u64,
}

/// The hottest sensor on the machine, named so the reading is attributable.
///
/// A summary rather than the full sensor list: a laptop exposes forty-odd
/// sensors, and carrying every one of them in a two-second sample would exhaust
/// the 64 MiB history cap long before the retention window did.
///
/// `critical_celsius` is the threshold the hardware itself declares, carried
/// only when the OS exposes one: Rackio does not invent a "hot" threshold for a
/// machine whose sensor layout it knows nothing about.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TemperatureMetric {
    pub label: String,
    pub celsius: f32,
    pub critical_celsius: Option<f32>,
    /// How many sensors this is the maximum of, so "hottest" stays checkable.
    pub sensor_count: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NetworkMetric {
    pub received_bytes_per_second: u64,
    pub sent_bytes_per_second: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CollectorError {
    pub source: String,
    pub kind: CapabilityState,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MetricSample {
    pub timestamp_ms: i64,
    pub sequence: u64,
    pub cpu_percent: Option<f32>,
    pub memory_used_bytes: Option<u64>,
    pub memory_total_bytes: Option<u64>,
    pub swap_used_bytes: Option<u64>,
    pub swap_total_bytes: Option<u64>,
    pub disks: Vec<DiskMetric>,
    pub network: Option<NetworkMetric>,
    /// `None` on a machine with no readable sensor — see the `temperature`
    /// capability for whether that means "none exists" rather than "none was
    /// readable this time". Defaulted so history written before protocol 1.1
    /// still deserialises.
    #[serde(default)]
    pub temperature: Option<TemperatureMetric>,
    pub uptime_seconds: u64,
    pub errors: Vec<CollectorError>,
}

/// One point of the live trend a viewer draws without querying storage.
///
/// A projection of [`MetricSample`], not the sample itself: the trend window
/// holds up to [`TrendWindow::CAPACITY`] of these per machine in memory and in
/// the persisted registry, so it carries only the fields a trend can plot.
/// The timestamp is the sample's own — a viewer must label its time axis from
/// the data rather than assuming a sampling cadence.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TrendSample {
    pub timestamp_ms: i64,
    pub cpu_percent: Option<f32>,
    pub memory_used_bytes: Option<u64>,
    pub memory_total_bytes: Option<u64>,
}

impl From<&MetricSample> for TrendSample {
    fn from(sample: &MetricSample) -> Self {
        Self {
            timestamp_ms: sample.timestamp_ms,
            cpu_percent: sample.cpu_percent,
            memory_used_bytes: sample.memory_used_bytes,
            memory_total_bytes: sample.memory_total_bytes,
        }
    }
}

/// The most recent [`TrendSample`]s of one machine, oldest first.
///
/// The single owner of the live-trend retention rule: every surface (local
/// collector, remote metric stream, persisted registry) pushes through this
/// type so none of them can grow unbounded or disagree on the window size.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct TrendWindow {
    samples: Vec<TrendSample>,
}

impl TrendWindow {
    /// At the collector's two-second cadence this spans about four minutes:
    /// enough to see a spike develop, small enough to ship in every snapshot.
    pub const CAPACITY: usize = 120;

    pub fn push(&mut self, sample: TrendSample) {
        self.samples.push(sample);
        if self.samples.len() > Self::CAPACITY {
            let excess = self.samples.len() - Self::CAPACITY;
            self.samples.drain(..excess);
        }
    }

    #[must_use]
    pub fn samples(&self) -> &[TrendSample] {
        &self.samples
    }
}

#[cfg(test)]
mod trend_window_tests {
    use super::{TrendSample, TrendWindow};

    fn sample(timestamp_ms: i64) -> TrendSample {
        TrendSample {
            timestamp_ms,
            cpu_percent: Some(10.0),
            memory_used_bytes: Some(1_000),
            memory_total_bytes: Some(2_000),
        }
    }

    #[test]
    fn caps_the_window_by_dropping_the_oldest_samples() {
        let mut window = TrendWindow::default();
        for index in 0..(TrendWindow::CAPACITY + 5) {
            window.push(sample(
                i64::try_from(index).unwrap_or_else(|error| panic!("{error}")),
            ));
        }

        assert_eq!(window.samples().len(), TrendWindow::CAPACITY);
        assert_eq!(
            window.samples()[0].timestamp_ms,
            5,
            "oldest samples leave first"
        );
    }

    #[test]
    fn serialises_as_a_bare_sample_array() {
        // The transparent representation is what snapshots and the persisted
        // registry carry; a wrapping object would break both without a
        // migration.
        let mut window = TrendWindow::default();
        window.push(sample(7));

        let json = serde_json::to_value(&window).unwrap_or_else(|error| panic!("{error}"));
        assert_eq!(json[0]["timestamp_ms"], 7);
        let restored: TrendWindow =
            serde_json::from_value(json).unwrap_or_else(|error| panic!("{error}"));
        assert_eq!(restored, window);
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HealthSnapshot {
    pub state: NodeState,
    pub collector_degraded: bool,
    pub storage_degraded: bool,
    pub remote_listener_degraded: bool,
    pub details: Vec<String>,
}
