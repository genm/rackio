use std::time::{Duration, Instant};

use chrono::Utc;
use sysinfo::{Components, Disks, Networks, System};

use crate::{
    CapabilityState, CollectorError, DiskMetric, MetricCapability, MetricSample, NetworkMetric,
    TemperatureMetric,
};

pub struct SystemCollector {
    system: System,
    disks: Disks,
    networks: Networks,
    components: Components,
    sequence: u64,
    last_network_refresh: Instant,
}

impl SystemCollector {
    #[must_use]
    pub fn new() -> Self {
        let mut system = System::new();
        system.refresh_memory();
        system.refresh_cpu_all();

        Self {
            system,
            disks: Disks::new_with_refreshed_list(),
            networks: Networks::new_with_refreshed_list(),
            components: Components::new_with_refreshed_list(),
            sequence: 0,
            last_network_refresh: Instant::now(),
        }
    }

    /// Report what this host can actually collect.
    ///
    /// Derived from the current readings rather than declared from a literal
    /// list, so a sandbox or container where a source is unreadable is
    /// reported as `Unsupported` instead of claiming support and then
    /// publishing zeros.
    #[must_use]
    pub fn capabilities(&self) -> Vec<MetricCapability> {
        // Read here, decide in `capabilities_from`. Every rule below this line
        // would otherwise only be reachable on a host that genuinely lacks the
        // source it describes.
        capabilities_from(
            readable_cpu_percent(self.system.cpus().len(), self.system.global_cpu_usage())
                .is_some(),
            readable_memory_total(self.system.total_memory()).is_some(),
            self.disks.list().len(),
            self.networks.list().len(),
            self.hottest_temperature().is_some(),
        )
    }

    /// The hottest sensor that reports a usable reading right now.
    ///
    /// A component whose temperature is absent or non-finite is skipped rather
    /// than ranked as 0 °C, which would read as a frozen machine.
    ///
    /// This method is only the sysinfo adapter; the ranking it delegates to is
    /// tested directly on the free function below. Mutating it away is
    /// indistinguishable on a host with no sensor, so it is excluded in
    /// `.cargo/mutants.toml` rather than left as a recurring finding.
    fn hottest_temperature(&self) -> Option<TemperatureMetric> {
        hottest_temperature(self.components.list().iter().map(|component| {
            (
                component.label(),
                component.temperature(),
                component.critical(),
            )
        }))
    }

    #[must_use]
    pub fn sample(&mut self) -> MetricSample {
        self.system.refresh_cpu_usage();
        self.system.refresh_memory();
        self.disks.refresh(true);
        self.components.refresh(true);

        let elapsed = self.last_network_refresh.elapsed();
        self.networks.refresh(true);
        self.last_network_refresh = Instant::now();

        self.sequence = self.sequence.saturating_add(1);

        let mut errors = Vec::new();

        let cpu_percent =
            readable_cpu_percent(self.system.cpus().len(), self.system.global_cpu_usage());
        if cpu_percent.is_none() {
            errors.push(unavailable("cpu", "no CPU usage is readable on this host"));
        }

        let memory_total_bytes = readable_memory_total(self.system.total_memory());
        let memory_used_bytes = memory_total_bytes.map(|_| self.system.used_memory());
        if memory_total_bytes.is_none() {
            errors.push(unavailable("memory", "total memory is not readable"));
        }

        let disks: Vec<DiskMetric> = self
            .disks
            .list()
            .iter()
            .filter_map(|disk| {
                disk_metric(
                    &disk.mount_point().to_string_lossy(),
                    disk.total_space(),
                    disk.available_space(),
                )
            })
            .collect();
        if disks.is_empty() {
            errors.push(unavailable("disk", "no filesystem could be enumerated"));
        }

        let (interface_count, received, sent) = self.networks.list().values().fold(
            (0_usize, 0_u64, 0_u64),
            |(count, received, sent), network| {
                (
                    count.saturating_add(1),
                    received.saturating_add(network.received()),
                    sent.saturating_add(network.transmitted()),
                )
            },
        );
        if interface_count == 0 {
            errors.push(unavailable(
                "network",
                "no network interface could be enumerated",
            ));
        }
        let network = network_metric(readable_totals(interface_count, received, sent), elapsed);

        // No collector error when nothing is readable: a cloud VM, a container
        // and an unprivileged macOS host genuinely expose no sensor, and
        // reporting that as a degraded collector would pin those machines to
        // `Degraded` forever. The absence is declared once, in `capabilities`.
        let temperature = self.hottest_temperature();

        MetricSample {
            timestamp_ms: Utc::now().timestamp_millis(),
            sequence: self.sequence,
            cpu_percent,
            memory_used_bytes,
            memory_total_bytes,
            swap_used_bytes: Some(self.system.used_swap()),
            swap_total_bytes: Some(self.system.total_swap()),
            disks,
            network,
            temperature,
            uptime_seconds: System::uptime(),
            errors,
        }
    }
}

/// Report what a host with these readings can collect.
///
/// Kept separate from the readings themselves so every branch is reachable: a
/// machine that can enumerate its disks cannot exercise the rule for one that
/// cannot.
fn capabilities_from(
    cpu: bool,
    memory: bool,
    disks: usize,
    interfaces: usize,
    temperature: bool,
) -> Vec<MetricCapability> {
    vec![
        capability("cpu", cpu, "no CPU is readable"),
        capability("memory", memory, "total memory is not readable"),
        // A machine with swap disabled genuinely reports zero total swap, so
        // absence of swap is not absence of the capability.
        capability("swap", true, ""),
        capability("disk", disks > 0, "no filesystem is enumerable"),
        capability(
            "network",
            interfaces > 0,
            "no network interface is enumerable",
        ),
        // Unlike swap, a machine that exposes no sensor cannot be said to be at
        // zero degrees, so absence is reported as an unsupported source with a
        // reason instead of a reading.
        capability(
            "temperature",
            temperature,
            "no temperature sensor is readable on this host",
        ),
    ]
}

/// An idle machine legitimately reports `Some(0.0)`, so zero is never treated
/// as absence. A host with no enumerable CPU has nothing to average, and a
/// non-finite reading is not a percentage; both are reported absent.
fn readable_cpu_percent(cpu_count: usize, usage: f32) -> Option<f32> {
    if cpu_count == 0 {
        return None;
    }
    usage.is_finite().then_some(usage)
}

/// `None` when no interface could be enumerated. A fold over nothing produces
/// zero, and publishing that would present an unreadable source as an idle one.
fn readable_totals(interfaces: usize, received: u64, sent: u64) -> Option<(u64, u64)> {
    (interfaces > 0).then_some((received, sent))
}

/// Every machine has memory, so a zero total means the source could not be read
/// rather than that the machine has none.
fn readable_memory_total(total: u64) -> Option<u64> {
    (total > 0).then_some(total)
}

/// A filesystem with no total is a pseudo-filesystem, not a full one. Including
/// it would put a meaningless 0-byte entry into the fleet's disk view.
fn disk_metric(mount: &str, total: u64, available: u64) -> Option<DiskMetric> {
    (total > 0).then(|| DiskMetric {
        mount: mount.to_owned(),
        total_bytes: total,
        used_bytes: total.saturating_sub(available),
    })
}

/// `totals` is `None` when no interface could be enumerated. With no readable
/// interface there is no traffic to divide, and with no elapsed time there is
/// nothing to divide by — reporting 0 B/s in either case would present an
/// unreadable source as an idle one.
fn network_metric(totals: Option<(u64, u64)>, elapsed: Duration) -> Option<NetworkMetric> {
    let (received, sent) = totals?;
    if elapsed.is_zero() {
        return None;
    }
    Some(NetworkMetric {
        received_bytes_per_second: rate_per_second(received, elapsed),
        sent_bytes_per_second: rate_per_second(sent, elapsed),
    })
}

/// Reduce every sensor the host lists to the hottest usable reading.
///
/// A sensor the host lists but cannot read has no temperature; ranking it as
/// 0 °C would let an unreadable sensor stand in for a frozen one, and letting a
/// NaN into the comparison would make the maximum depend on iteration order.
/// Unreadable sensors are excluded from `sensor_count` too, so the published
/// count matches what the reading was actually chosen from. The hardware's own
/// critical threshold is carried only when it is a real number, so the viewer
/// never compares against a NaN.
fn hottest_temperature<'a>(
    sensors: impl Iterator<Item = (&'a str, Option<f32>, Option<f32>)>,
) -> Option<TemperatureMetric> {
    let readable: Vec<(&str, f32, Option<f32>)> = sensors
        .filter_map(|(label, celsius, critical)| {
            let celsius = celsius.filter(|value| value.is_finite())?;
            Some((label, celsius, critical.filter(|value| value.is_finite())))
        })
        .collect();
    let sensor_count = u32::try_from(readable.len()).unwrap_or(u32::MAX);
    let (label, celsius, critical_celsius) = readable
        .into_iter()
        .max_by(|(_, left, _), (_, right, _)| left.total_cmp(right))?;
    Some(TemperatureMetric {
        label: label.to_owned(),
        celsius,
        critical_celsius,
        sensor_count,
    })
}

fn capability(name: &str, supported: bool, detail: &str) -> MetricCapability {
    MetricCapability {
        name: name.to_owned(),
        state: if supported {
            CapabilityState::Supported
        } else {
            CapabilityState::Unsupported
        },
        detail: (!supported && !detail.is_empty()).then(|| detail.to_owned()),
    }
}

fn unavailable(source: &str, message: &str) -> CollectorError {
    CollectorError {
        source: source.to_owned(),
        kind: CapabilityState::Unsupported,
        message: message.to_owned(),
    }
}

impl Default for SystemCollector {
    fn default() -> Self {
        Self::new()
    }
}

fn rate_per_second(bytes: u64, elapsed: Duration) -> u64 {
    let rate = u128::from(bytes)
        .saturating_mul(1_000_000_000)
        .checked_div(elapsed.as_nanos())
        .unwrap_or_default();
    u64::try_from(rate).unwrap_or(u64::MAX)
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::{
        SystemCollector, capabilities_from, capability, disk_metric, hottest_temperature,
        network_metric, rate_per_second, readable_cpu_percent, readable_memory_total,
        readable_totals,
    };
    use crate::CapabilityState;

    #[test]
    fn an_unreadable_cpu_is_absent_but_an_idle_one_is_zero() {
        assert_eq!(
            readable_cpu_percent(1, 0.0),
            Some(0.0),
            "idle is not absent"
        );
        assert_eq!(readable_cpu_percent(8, 42.5), Some(42.5));
        assert_eq!(readable_cpu_percent(1, f32::NAN), None);
        assert_eq!(readable_cpu_percent(1, f32::INFINITY), None);
        assert_eq!(
            readable_cpu_percent(0, 0.0),
            None,
            "a host with no enumerable CPU has nothing to average"
        );
    }

    #[test]
    fn a_host_with_no_interface_reports_no_traffic_at_all() {
        assert_eq!(readable_totals(0, 0, 0), None);
        assert_eq!(
            readable_totals(0, 10, 20),
            None,
            "a stale total without an interface is not a reading"
        );
        assert_eq!(readable_totals(1, 10, 20), Some((10, 20)));
        assert_eq!(
            readable_totals(1, 0, 0),
            Some((0, 0)),
            "an enumerated interface with no traffic is genuinely idle"
        );
    }

    #[test]
    fn a_source_that_cannot_be_read_is_declared_unsupported_with_a_reason() {
        // Each rule is exercised in both directions here, which no single host
        // can do: this machine either has a disk or it does not.
        let all = capabilities_from(true, true, 1, 1, true);
        for name in ["cpu", "memory", "swap", "disk", "network", "temperature"] {
            let capability = all
                .iter()
                .find(|capability| capability.name == name)
                .unwrap_or_else(|| panic!("{name} capability is missing"));
            assert_eq!(
                capability.state,
                CapabilityState::Supported,
                "{name} is readable on this reading"
            );
        }

        let none = capabilities_from(false, false, 0, 0, false);
        for name in ["cpu", "memory", "disk", "network", "temperature"] {
            let capability = none
                .iter()
                .find(|capability| capability.name == name)
                .unwrap_or_else(|| panic!("{name} capability is missing"));
            assert_eq!(capability.state, CapabilityState::Unsupported, "{name}");
            assert!(capability.detail.is_some(), "{name} must carry a reason");
        }

        let swap = none
            .iter()
            .find(|capability| capability.name == "swap")
            .unwrap_or_else(|| panic!("swap capability is missing"));
        assert_eq!(
            swap.state,
            CapabilityState::Supported,
            "swap stays supported: a machine with it disabled truly has zero"
        );
    }

    #[test]
    fn a_zero_memory_total_means_unreadable() {
        assert_eq!(readable_memory_total(0), None);
        assert_eq!(readable_memory_total(1), Some(1));
        assert_eq!(readable_memory_total(u64::MAX), Some(u64::MAX));
    }

    #[test]
    fn a_filesystem_without_a_total_is_not_reported() {
        assert_eq!(disk_metric("/proc", 0, 0), None);

        let root = disk_metric("/", 100, 30)
            .unwrap_or_else(|| panic!("a filesystem with a total is reported"));
        assert_eq!(root.mount, "/");
        assert_eq!(root.total_bytes, 100);
        assert_eq!(root.used_bytes, 70, "used is total minus available");

        // More available than total cannot underflow into a huge used value.
        let odd = disk_metric("/odd", 100, 500)
            .unwrap_or_else(|| panic!("a filesystem with a total is reported"));
        assert_eq!(odd.used_bytes, 0);
    }

    #[test]
    fn an_unreadable_network_is_absent_rather_than_idle() {
        // No interface enumerated, and no elapsed interval, are both "cannot
        // tell" rather than "no traffic".
        assert!(network_metric(None, Duration::from_secs(1)).is_none());
        assert!(network_metric(Some((1_024, 2_048)), Duration::ZERO).is_none());

        let metric = network_metric(Some((2_048, 4_096)), Duration::from_secs(2))
            .unwrap_or_else(|| panic!("a readable interval produces a rate"));
        assert_eq!(metric.received_bytes_per_second, 1_024);
        assert_eq!(metric.sent_bytes_per_second, 2_048);

        // A genuinely idle interface does report zero.
        let idle = network_metric(Some((0, 0)), Duration::from_secs(1))
            .unwrap_or_else(|| panic!("an enumerated idle interface still reports"));
        assert_eq!(idle.received_bytes_per_second, 0);
    }

    #[test]
    fn the_hottest_readable_sensor_wins_and_unreadable_ones_are_not_zero_degrees() {
        // A listed-but-unreadable sensor is the common case on macOS without
        // privileges and inside containers. Zero degrees would both render as a
        // frozen machine and inflate the count the reading is drawn from.
        let hottest = hottest_temperature(
            [
                ("SSD", Some(41.0), None),
                ("CPU die", Some(72.5), Some(100.0)),
                ("Fan intake", None, None),
                ("Broken", Some(f32::NAN), None),
                ("Also broken", Some(f32::INFINITY), None),
            ]
            .into_iter(),
        )
        .unwrap_or_else(|| panic!("two sensors are readable"));
        assert_eq!(hottest.label, "CPU die");
        assert!((hottest.celsius - 72.5).abs() < f32::EPSILON);
        assert_eq!(hottest.critical_celsius, Some(100.0));
        assert_eq!(
            hottest.sensor_count, 2,
            "only the sensors the reading could be chosen from are counted"
        );

        // Sub-zero is a real reading on cold hardware, so it must survive
        // rather than lose to an absent sensor treated as zero.
        let cold =
            hottest_temperature([("Intake", Some(-5.0), None), ("Dead", None, None)].into_iter())
                .unwrap_or_else(|| panic!("a sub-zero reading is still a reading"));
        assert!((cold.celsius + 5.0).abs() < f32::EPSILON);
        assert_eq!(cold.sensor_count, 1);

        // A hardware threshold the host reports as non-finite is no threshold.
        assert_eq!(
            hottest_temperature([("CPU", Some(40.0), Some(f32::NAN))].into_iter())
                .unwrap_or_else(|| panic!("the reading itself is finite"))
                .critical_celsius,
            None
        );

        // A host with no sensor, and one whose every sensor is unreadable, both
        // report nothing rather than a 0 °C stand-in.
        assert!(hottest_temperature(std::iter::empty()).is_none());
        assert!(hottest_temperature([("Dead", None, None)].into_iter()).is_none());
    }

    #[test]
    fn an_unsupported_capability_carries_its_reason() {
        let supported = capability("cpu", true, "no CPU is readable");
        assert_eq!(supported.state, CapabilityState::Supported);
        assert_eq!(
            supported.detail, None,
            "a supported source needs no explanation"
        );

        let unsupported = capability("cpu", false, "no CPU is readable");
        assert_eq!(unsupported.state, CapabilityState::Unsupported);
        assert_eq!(unsupported.detail.as_deref(), Some("no CPU is readable"));

        // Swap is declared supported with no reason text; an empty reason must
        // stay absent rather than become an empty string.
        assert_eq!(capability("swap", false, "").detail, None);
    }

    #[test]
    fn declares_only_the_capabilities_this_host_can_actually_read() {
        let collector = SystemCollector::new();
        let capabilities = collector.capabilities();

        for name in ["cpu", "memory", "swap", "disk", "network", "temperature"] {
            let capability = capabilities
                .iter()
                .find(|capability| capability.name == name)
                .unwrap_or_else(|| panic!("{name} capability is missing"));
            // An unsupported source must carry a reason rather than silently
            // claiming support.
            if capability.state == CapabilityState::Unsupported {
                assert!(capability.detail.is_some(), "{name} lacks a reason");
            }
        }
    }

    #[test]
    fn an_unreadable_source_is_absent_rather_than_zero() {
        let mut collector = SystemCollector::new();
        let sample = collector.sample();

        // Whatever this host supports, no metric may be present as a zero
        // stand-in for an unreadable source: every reported error must
        // correspond to an absent value, and vice versa.
        let errored = |source: &str| sample.errors.iter().any(|error| error.source == source);
        assert_eq!(sample.cpu_percent.is_none(), errored("cpu"));
        assert_eq!(sample.memory_total_bytes.is_none(), errored("memory"));
        assert_eq!(sample.disks.is_empty(), errored("disk"));
        assert_eq!(sample.network.is_none(), errored("network"));
        if sample.memory_total_bytes.is_none() {
            assert!(sample.memory_used_bytes.is_none());
        }
        if let Some(cpu) = sample.cpu_percent {
            assert!(
                (0.0..=100.0).contains(&cpu),
                "a CPU reading is a percentage, got {cpu}"
            );
        }
    }

    #[test]
    fn a_declared_capability_agrees_with_what_the_sample_carries() {
        // Host-independent in both directions: this asserts that the two views
        // cannot disagree, not that this machine has any particular source.
        let mut collector = SystemCollector::new();
        let sample = collector.sample();
        let capabilities = collector.capabilities();
        let state = |name: &str| {
            capabilities
                .iter()
                .find(|capability| capability.name == name)
                .unwrap_or_else(|| panic!("{name} capability is missing"))
                .state
        };

        if sample.cpu_percent.is_some() {
            assert_eq!(state("cpu"), CapabilityState::Supported);
        }
        if sample.memory_total_bytes.is_some() {
            assert_eq!(state("memory"), CapabilityState::Supported);
        }
        if !sample.disks.is_empty() {
            assert_eq!(
                state("disk"),
                CapabilityState::Supported,
                "a sample carrying a filesystem cannot come with an unsupported disk capability"
            );
        }
        if sample.network.is_some() {
            assert_eq!(state("network"), CapabilityState::Supported);
        }
        if sample.temperature.is_some() {
            assert_eq!(
                state("temperature"),
                CapabilityState::Supported,
                "a sample carrying a sensor cannot come with an unsupported temperature capability"
            );
        }
    }

    #[test]
    fn an_unreadable_sensor_degrades_nothing_and_publishes_nothing() {
        // Hosts without sensors are the majority of a fleet — cloud VMs and
        // containers. They must stay healthy, so the absence is a capability
        // statement and never a collector error.
        let mut collector = SystemCollector::new();
        let sample = collector.sample();

        assert!(
            !sample
                .errors
                .iter()
                .any(|error| error.source == "temperature"),
            "an absent sensor is not a degraded collector"
        );
        if let Some(temperature) = &sample.temperature {
            assert!(
                temperature.celsius.is_finite(),
                "{} reported a non-finite reading",
                temperature.label
            );
            assert!(
                temperature.sensor_count > 0,
                "a published reading came from at least the sensor it names"
            );
        }
    }

    #[test]
    fn converts_delta_to_rate() {
        assert_eq!(rate_per_second(2_048, Duration::from_secs(2)), 1_024);
    }

    #[test]
    fn does_not_underflow_on_tiny_intervals() {
        assert!(rate_per_second(1, Duration::from_nanos(1)) > 0);
    }
}
