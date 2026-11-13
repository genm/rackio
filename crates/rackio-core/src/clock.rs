use std::time::Instant;

use chrono::Utc;

/// Wall-clock timestamps that cannot jump backwards or forwards while the
/// process runs.
///
/// Retention and staleness are *durations*, but they were derived from
/// `Utc::now()`. A forward NTP step or a restored VM snapshot then made the
/// next prune delete the whole history, and a backward step froze staleness
/// derivation and let new samples overwrite persisted rows. Anchoring the wall
/// clock once and advancing it monotonically keeps those decisions stable for
/// the lifetime of the process; only a restart re-anchors.
///
/// The value is still a real wall-clock timestamp, so it remains meaningful as
/// a stored label and comparable across machines.
#[derive(Debug, Clone)]
pub struct Clock {
    wall_anchor_ms: i64,
    instant_anchor: Instant,
}

impl Clock {
    #[must_use]
    pub fn new() -> Self {
        Self {
            wall_anchor_ms: Utc::now().timestamp_millis(),
            instant_anchor: Instant::now(),
        }
    }

    /// The anchored wall-clock time advanced by monotonic elapsed time.
    #[must_use]
    pub fn now_ms(&self) -> i64 {
        let elapsed = i64::try_from(self.instant_anchor.elapsed().as_millis()).unwrap_or(i64::MAX);
        self.wall_anchor_ms.saturating_add(elapsed)
    }
}

impl Default for Clock {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::Clock;

    #[test]
    fn advances_monotonically_from_its_anchor() {
        let clock = Clock::new();
        let first = clock.now_ms();
        let second = clock.now_ms();
        assert!(second >= first);
    }

    #[test]
    fn stays_close_to_wall_clock_at_creation() {
        let clock = Clock::new();
        let drift = (clock.now_ms() - chrono::Utc::now().timestamp_millis()).abs();
        assert!(drift < 1_000, "anchored clock drifted by {drift} ms");
    }
}
