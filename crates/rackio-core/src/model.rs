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
/// Every periodically displayed metric is carried here — a number the viewer
/// shows on a cadence must also be plottable, without exception. The timestamp
/// is the sample's own — a viewer must label its time axis from the data
/// rather than assuming a sampling cadence.
///
/// `rtt_ms` is not part of [`MetricSample`]: the viewer's agent measures it
/// against the connection, so the stream loop stamps it after projection.
/// Later fields default so trend windows persisted before them deserialise.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TrendSample {
    pub timestamp_ms: i64,
    pub cpu_percent: Option<f32>,
    pub memory_used_bytes: Option<u64>,
    pub memory_total_bytes: Option<u64>,
    #[serde(default)]
    pub disk_used_bytes: Option<u64>,
    #[serde(default)]
    pub disk_total_bytes: Option<u64>,
    #[serde(default)]
    pub temperature_celsius: Option<f32>,
    #[serde(default)]
    pub network_received_bytes_per_second: Option<u64>,
    #[serde(default)]
    pub network_sent_bytes_per_second: Option<u64>,
    #[serde(default)]
    pub rtt_ms: Option<u64>,
}

impl MetricSample {
    /// The fullest disk, the one that runs out first. Owned here so the trend
    /// and any snapshot view agree on which disk "the machine's disk" is.
    #[must_use]
    pub fn worst_disk(&self) -> Option<&DiskMetric> {
        self.disks
            .iter()
            .filter(|disk| disk.total_bytes > 0)
            .max_by(|left, right| {
                u128::from(left.used_bytes)
                    .saturating_mul(u128::from(right.total_bytes))
                    .cmp(&u128::from(right.used_bytes).saturating_mul(u128::from(left.total_bytes)))
            })
    }
}

impl From<&MetricSample> for TrendSample {
    fn from(sample: &MetricSample) -> Self {
        let worst_disk = sample.worst_disk();
        Self {
            timestamp_ms: sample.timestamp_ms,
            cpu_percent: sample.cpu_percent,
            memory_used_bytes: sample.memory_used_bytes,
            memory_total_bytes: sample.memory_total_bytes,
            disk_used_bytes: worst_disk.map(|disk| disk.used_bytes),
            disk_total_bytes: worst_disk.map(|disk| disk.total_bytes),
            temperature_celsius: sample
                .temperature
                .as_ref()
                .map(|temperature| temperature.celsius),
            network_received_bytes_per_second: sample
                .network
                .as_ref()
                .map(|network| network.received_bytes_per_second),
            network_sent_bytes_per_second: sample
                .network
                .as_ref()
                .map(|network| network.sent_bytes_per_second),
            rtt_ms: None,
        }
    }
}

impl TrendWindow {
    /// Rebuild a window from stored samples, oldest first, keeping only the
    /// most recent [`TrendWindow::CAPACITY`]. The local machine's window lives
    /// in memory, so without this a daemon restart would blank its own trend
    /// while every remote kept the one its registry persisted.
    #[must_use]
    pub fn from_samples(samples: &[MetricSample]) -> Self {
        let mut window = Self::default();
        for sample in samples
            .iter()
            .skip(samples.len().saturating_sub(Self::CAPACITY))
        {
            window.push(TrendSample::from(sample));
        }
        window
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

    /// Append one sample and restore the capacity bound, however far the
    /// window started beyond it.
    ///
    /// The trim is unconditional rather than guarded by a length comparison: a
    /// guard that only differs from this at exactly `CAPACITY`, where the trim
    /// is already a no-op, is a branch no test can distinguish. Draining a
    /// saturating overshoot says the same thing with one behaviour instead of
    /// two.
    pub fn push(&mut self, sample: TrendSample) {
        self.samples.push(sample);
        let excess = self.samples.len().saturating_sub(Self::CAPACITY);
        self.samples.drain(..excess);
    }

    #[must_use]
    pub fn samples(&self) -> &[TrendSample] {
        &self.samples
    }
}

#[cfg(test)]
mod trend_sample_tests {
    use super::{DiskMetric, MetricSample, NetworkMetric, TemperatureMetric, TrendSample};

    #[test]
    fn projects_the_fullest_disk_and_the_hottest_sensor() {
        let sample = MetricSample {
            timestamp_ms: 1,
            sequence: 0,
            cpu_percent: Some(10.0),
            memory_used_bytes: Some(1),
            memory_total_bytes: Some(2),
            swap_used_bytes: None,
            swap_total_bytes: None,
            disks: vec![
                DiskMetric {
                    mount: String::from("/"),
                    total_bytes: 100,
                    used_bytes: 10,
                },
                DiskMetric {
                    mount: String::from("/data"),
                    total_bytes: 100,
                    used_bytes: 90,
                },
                // A zero-total pseudo filesystem must never win, or the card
                // would divide by zero for a machine with a real disk.
                DiskMetric {
                    mount: String::from("/proc"),
                    total_bytes: 0,
                    used_bytes: 0,
                },
            ],
            network: Some(NetworkMetric {
                received_bytes_per_second: 2_048,
                sent_bytes_per_second: 512,
            }),
            temperature: Some(TemperatureMetric {
                label: String::from("Package id 0"),
                celsius: 61.5,
                critical_celsius: None,
                sensor_count: 7,
            }),
            uptime_seconds: 0,
            errors: Vec::new(),
        };

        let point = TrendSample::from(&sample);
        assert_eq!(point.disk_used_bytes, Some(90));
        assert_eq!(point.disk_total_bytes, Some(100));
        assert_eq!(point.temperature_celsius, Some(61.5));
        assert_eq!(point.network_received_bytes_per_second, Some(2_048));
        assert_eq!(point.network_sent_bytes_per_second, Some(512));
        assert_eq!(
            point.rtt_ms, None,
            "RTT is stamped by the stream loop, not the sample"
        );
    }

    #[test]
    fn deserialises_a_pre_disk_trend_sample() {
        // Windows persisted before the later fields existed must still load.
        let point: TrendSample = serde_json::from_value(serde_json::json!({
            "timestamp_ms": 5,
            "cpu_percent": 1.0,
            "memory_used_bytes": 1,
            "memory_total_bytes": 2,
        }))
        .unwrap_or_else(|error| panic!("{error}"));
        assert_eq!(point.disk_used_bytes, None);
        assert_eq!(point.rtt_ms, None);
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
            disk_used_bytes: Some(90),
            disk_total_bytes: Some(100),
            temperature_celsius: Some(61.5),
            network_received_bytes_per_second: Some(2_048),
            network_sent_bytes_per_second: Some(512),
            rtt_ms: Some(8),
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
    fn rebuilds_from_stored_samples_keeping_only_the_newest() {
        let stored: Vec<super::MetricSample> = (0..(TrendWindow::CAPACITY + 3))
            .map(|index| super::MetricSample {
                timestamp_ms: i64::try_from(index).unwrap_or_else(|error| panic!("{error}")),
                sequence: 0,
                cpu_percent: Some(5.0),
                memory_used_bytes: None,
                memory_total_bytes: None,
                swap_used_bytes: None,
                swap_total_bytes: None,
                disks: Vec::new(),
                network: None,
                temperature: None,
                uptime_seconds: 0,
                errors: Vec::new(),
            })
            .collect();

        let window = TrendWindow::from_samples(&stored);
        assert_eq!(window.samples().len(), TrendWindow::CAPACITY);
        assert_eq!(
            window.samples()[0].timestamp_ms,
            3,
            "a restart resumes at the newest stored samples, not the oldest"
        );
    }

    #[test]
    fn restores_the_capacity_bound_from_an_over_capacity_window() {
        // Pushing one at a time never leaves the window more than a single
        // sample over capacity, so that path alone cannot show that the trim
        // is proportional to the overshoot. A window deserialised from a
        // persisted snapshot can start arbitrarily far over the bound, and the
        // next push has to bring it back regardless of how far.
        let oversized: Vec<TrendSample> = (0..(TrendWindow::CAPACITY * 4))
            .map(|index| sample(i64::try_from(index).unwrap_or_else(|error| panic!("{error}"))))
            .collect();
        let mut window: TrendWindow = serde_json::from_value(
            serde_json::to_value(&oversized).unwrap_or_else(|error| panic!("{error}")),
        )
        .unwrap_or_else(|error| panic!("{error}"));

        window.push(sample(i64::MAX));

        assert_eq!(window.samples().len(), TrendWindow::CAPACITY);
        assert_eq!(
            window.samples()[TrendWindow::CAPACITY - 1].timestamp_ms,
            i64::MAX,
            "the newest sample survives the trim"
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
