use std::time::{Duration, Instant};

use chrono::Utc;
use sysinfo::{Disks, Networks, System};

use crate::{
    CapabilityState, CollectorError, DiskMetric, MetricCapability, MetricSample, NetworkMetric,
};

pub struct SystemCollector {
    system: System,
    disks: Disks,
    networks: Networks,
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
        let mut capabilities = vec![
            capability("cpu", self.cpu_percent().is_some(), "no CPU is readable"),
            capability(
                "memory",
                self.memory_total().is_some(),
                "total memory is not readable",
            ),
            // A machine with swap disabled genuinely reports zero total swap,
            // so absence of swap is not absence of the capability.
            capability("swap", true, ""),
        ];
        capabilities.push(capability(
            "disk",
            !self.disks.list().is_empty(),
            "no filesystem is enumerable",
        ));
        capabilities.push(capability(
            "network",
            !self.networks.list().is_empty(),
            "no network interface is enumerable",
        ));
        capabilities
    }

    /// `None` when no CPU is readable, or when the reading is not a finite
    /// percentage. An idle machine legitimately reports `Some(0.0)`, so zero
    /// is never treated as absence.
    fn cpu_percent(&self) -> Option<f32> {
        if self.system.cpus().is_empty() {
            return None;
        }
        let usage = self.system.global_cpu_usage();
        usage.is_finite().then_some(usage)
    }

    /// `None` when the total is zero: every machine has memory, so a zero
    /// total means the source could not be read rather than that the machine
    /// has none.
    fn memory_total(&self) -> Option<u64> {
        let total = self.system.total_memory();
        (total > 0).then_some(total)
    }

    #[must_use]
    pub fn sample(&mut self) -> MetricSample {
        self.system.refresh_cpu_usage();
        self.system.refresh_memory();
        self.disks.refresh(true);

        let elapsed = self.last_network_refresh.elapsed();
        self.networks.refresh(true);
        self.last_network_refresh = Instant::now();

        self.sequence = self.sequence.saturating_add(1);

        let mut errors = Vec::new();

        let cpu_percent = self.cpu_percent();
        if cpu_percent.is_none() {
            errors.push(unavailable("cpu", "no CPU usage is readable on this host"));
        }

        let memory_total_bytes = self.memory_total();
        let memory_used_bytes = memory_total_bytes.map(|_| self.system.used_memory());
        if memory_total_bytes.is_none() {
            errors.push(unavailable("memory", "total memory is not readable"));
        }

        let disks: Vec<DiskMetric> = self
            .disks
            .list()
            .iter()
            .filter_map(|disk| {
                let total = disk.total_space();
                (total > 0).then(|| DiskMetric {
                    mount: disk.mount_point().to_string_lossy().into_owned(),
                    total_bytes: total,
                    used_bytes: total.saturating_sub(disk.available_space()),
                })
            })
            .collect();
        if disks.is_empty() {
            errors.push(unavailable("disk", "no filesystem could be enumerated"));
        }

        // With no readable interface there is no traffic to divide, and
        // reporting 0 B/s would present an unreadable source as an idle one.
        let network = if self.networks.list().is_empty() || elapsed.is_zero() {
            if self.networks.list().is_empty() {
                errors.push(unavailable(
                    "network",
                    "no network interface could be enumerated",
                ));
            }
            None
        } else {
            let (received, sent) =
                self.networks
                    .list()
                    .values()
                    .fold((0_u64, 0_u64), |(received, sent), network| {
                        (
                            received.saturating_add(network.received()),
                            sent.saturating_add(network.transmitted()),
                        )
                    });
            Some(NetworkMetric {
                received_bytes_per_second: rate_per_second(received, elapsed),
                sent_bytes_per_second: rate_per_second(sent, elapsed),
            })
        };

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
            uptime_seconds: System::uptime(),
            errors,
        }
    }
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

    use super::{SystemCollector, rate_per_second};
    use crate::CapabilityState;

    #[test]
    fn declares_only_the_capabilities_this_host_can_actually_read() {
        let collector = SystemCollector::new();
        let capabilities = collector.capabilities();

        for name in ["cpu", "memory", "swap", "disk", "network"] {
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
        if errored("network") {
            assert!(sample.network.is_none());
        }
        if sample.memory_total_bytes.is_none() {
            assert!(sample.memory_used_bytes.is_none());
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
