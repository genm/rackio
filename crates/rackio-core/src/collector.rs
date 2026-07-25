use std::time::{Duration, Instant};

use chrono::Utc;
use sysinfo::{Disks, Networks, System};

use crate::{DiskMetric, MetricSample, NetworkMetric};

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

    #[must_use]
    pub fn sample(&mut self) -> MetricSample {
        self.system.refresh_cpu_usage();
        self.system.refresh_memory();
        self.disks.refresh(true);

        let elapsed = self.last_network_refresh.elapsed();
        self.networks.refresh(true);
        self.last_network_refresh = Instant::now();

        self.sequence = self.sequence.saturating_add(1);

        let disks = self
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

        let network = (!elapsed.is_zero()).then(|| NetworkMetric {
            received_bytes_per_second: rate_per_second(received, elapsed),
            sent_bytes_per_second: rate_per_second(sent, elapsed),
        });

        MetricSample {
            timestamp_ms: Utc::now().timestamp_millis(),
            sequence: self.sequence,
            cpu_percent: Some(self.system.global_cpu_usage()),
            memory_used_bytes: Some(self.system.used_memory()),
            memory_total_bytes: Some(self.system.total_memory()),
            swap_used_bytes: Some(self.system.used_swap()),
            swap_total_bytes: Some(self.system.total_swap()),
            disks,
            network,
            uptime_seconds: System::uptime(),
            errors: Vec::new(),
        }
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

    use super::rate_per_second;

    #[test]
    fn converts_delta_to_rate() {
        assert_eq!(rate_per_second(2_048, Duration::from_secs(2)), 1_024);
    }

    #[test]
    fn does_not_underflow_on_tiny_intervals() {
        assert!(rate_per_second(1, Duration::from_nanos(1)) > 0);
    }
}
