//! The in-memory view of one paired machine and the freshness rule that
//! derives stale/offline from when this viewer last saw it.

use rackio_core::{ConnectionPath, MetricSample, NodeInfo, NodeState, TrendWindow};
use serde::Serialize;

use super::registry::RemoteMachineRecord;

pub(super) const STALE_AFTER_MS: i64 = 10_000;
pub(super) const OFFLINE_AFTER_MS: i64 = 30_000;

#[derive(Debug, Clone, Serialize)]
pub struct RemoteMachineSnapshot {
    pub node: NodeInfo,
    pub endpoint_id: String,
    pub latest: Option<MetricSample>,
    pub state: NodeState,
    pub path: ConnectionPath,
    pub rtt_ms: Option<u64>,
    pub last_seen_ms: Option<i64>,
    pub trend: TrendWindow,
    pub details: Vec<String>,
}

impl RemoteMachineSnapshot {
    pub(super) fn offline(record: &RemoteMachineRecord) -> Self {
        if let Some(persisted) = &record.last_snapshot {
            return Self {
                node: record.node.clone(),
                endpoint_id: record.endpoint_id.clone(),
                latest: persisted.latest.clone(),
                state: persisted.state,
                path: persisted.path,
                rtt_ms: persisted.rtt_ms,
                last_seen_ms: persisted.last_seen_ms,
                trend: persisted.trend.clone(),
                details: persisted.details.clone(),
            };
        }
        Self {
            node: record.node.clone(),
            endpoint_id: record.endpoint_id.clone(),
            latest: None,
            state: NodeState::Offline,
            path: ConnectionPath::Unknown,
            rtt_ms: None,
            last_seen_ms: None,
            trend: TrendWindow::default(),
            details: vec![String::from("Waiting for remote connection")],
        }
    }

    pub(super) fn state_at(&self, now_ms: i64) -> NodeState {
        if matches!(self.state, NodeState::AuthError | NodeState::Incompatible) {
            return self.state;
        }
        let Some(last_seen_ms) = self.last_seen_ms else {
            return self.state;
        };
        // `saturating_sub` on i64 does not clamp at zero, so a clock that
        // moved backwards produced a negative age and froze this derivation at
        // the stored state. A machine cannot have been seen in the future:
        // treat that as just-seen rather than silently trusting stale state.
        let age_ms = now_ms.saturating_sub(last_seen_ms).max(0);
        if age_ms >= OFFLINE_AFTER_MS {
            NodeState::Offline
        } else if age_ms >= STALE_AFTER_MS {
            NodeState::Stale
        } else {
            self.state
        }
    }
}

#[cfg(test)]
mod tests {
    use rackio_core::NodeState;

    use super::{OFFLINE_AFTER_MS, RemoteMachineSnapshot, STALE_AFTER_MS};
    use crate::remote::test_support::record;

    #[test]
    fn stale_and_offline_are_derived_from_local_last_seen_time() {
        let record = record();
        let mut snapshot = RemoteMachineSnapshot::offline(&record);
        snapshot.state = NodeState::Healthy;
        snapshot.last_seen_ms = Some(10_000);

        assert_eq!(snapshot.state_at(10_000 + STALE_AFTER_MS), NodeState::Stale);
        assert_eq!(
            snapshot.state_at(10_000 + OFFLINE_AFTER_MS),
            NodeState::Offline
        );

        snapshot.state = NodeState::AuthError;
        assert_eq!(
            snapshot.state_at(10_000 + OFFLINE_AFTER_MS),
            NodeState::AuthError
        );
    }
}
