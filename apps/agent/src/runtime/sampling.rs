//! Metric sampling, persistence batching, and health-state projection.

use std::{sync::Arc, time::Duration};

use rackio_core::{NodeState, SystemCollector};
use rackio_iroh::NodeRuntime;
use tokio::sync::watch;

/// The single owner of the published node state and its detail lines.
///
/// Degradation still wins: a machine whose collector, storage or remote
/// listener is broken is reported as `Degraded` rather than as a threshold
/// verdict, because the underlying data is no longer trustworthy. Once every
/// subsystem is healthy again the state is the worst active alert severity, so
/// clearing a degradation flag can never claim `Healthy` while an operator
/// threshold is still breached.
///
/// `details` is derived here rather than appended to from each call site. The
/// viewer shows the first line, so a degradation discovered after an alert was
/// raised would otherwise leave a machine reading `degraded` while displaying
/// a disk percentage as its explanation. Degradation lines therefore always
/// lead, and no line can outlive the condition that produced it.
fn project_health(
    health: &mut rackio_core::HealthSnapshot,
    severity: Option<NodeState>,
    alert_details: &[String],
) {
    let degraded = [
        (health.collector_degraded, "collector_degraded"),
        (health.storage_degraded, "storage_degraded"),
        (health.remote_listener_degraded, "remote_listener_degraded"),
    ];
    health.details = degraded
        .into_iter()
        .filter(|&(active, _)| active)
        .map(|(_, token)| String::from(token))
        .chain(alert_details.iter().cloned())
        .collect();
    health.state = if health.collector_degraded
        || health.storage_degraded
        || health.remote_listener_degraded
    {
        NodeState::Degraded
    } else {
        severity.unwrap_or(NodeState::Healthy)
    };
}

/// Reflect the collector's own errors in the published health snapshot, and
/// clear them again once every source reads.
async fn apply_health(
    runtime: &NodeRuntime,
    errors: &[rackio_core::CollectorError],
    severity: Option<NodeState>,
    alert_details: &[String],
) {
    let degraded = !errors.is_empty();
    let mut health = runtime.health.write().await;
    if degraded && !health.collector_degraded {
        tracing::warn!(
            sources = ?errors.iter().map(|error| error.source.as_str()).collect::<Vec<_>>(),
            "one or more metric sources are unreadable on this host"
        );
    }
    health.collector_degraded = degraded;
    project_health(&mut health, severity, alert_details);
}

/// Commit whatever is buffered so a graceful stop does not discard history.
/// A failure here is reported, never presented as a successful flush.
async fn flush_pending(runtime: &NodeRuntime, pending: &mut Vec<rackio_core::MetricSample>) {
    if pending.is_empty() {
        return;
    }
    match runtime.store.lock().await.insert_batch(pending) {
        Ok(()) => {
            tracing::info!(
                samples = pending.len(),
                "flushed buffered samples on shutdown"
            );
            pending.clear();
        }
        Err(error) => tracing::error!(
            error = %error,
            samples = pending.len(),
            "failed to flush buffered samples on shutdown; this history is lost"
        ),
    }
}

pub(super) async fn sample_loop(
    runtime: Arc<NodeRuntime>,
    mut collector: SystemCollector,
    alert_rules: Vec<rackio_core::AlertRule>,
    latest: watch::Sender<Option<rackio_core::MetricSample>>,
    mut shutdown: watch::Receiver<bool>,
) {
    let clock = rackio_core::Clock::new();
    let mut alerts = rackio_core::AlertEvaluator::default();
    tokio::time::sleep(sysinfo::MINIMUM_CPU_UPDATE_INTERVAL).await;
    let mut ticker = tokio::time::interval(Duration::from_secs(2));
    let mut pending = Vec::with_capacity(5);
    let mut prune_counter = 0_u16;
    loop {
        tokio::select! {
            _ = ticker.tick() => {}
            changed = shutdown.changed() => {
                // A closed sender means the daemon is going away too.
                if changed.is_err() || *shutdown.borrow() {
                    flush_pending(&runtime, &mut pending).await;
                    return;
                }
                continue;
            }
        }
        let sample = collector.sample();
        for signal in alerts.evaluate(&sample, &alert_rules) {
            // A raised alert is the event an operator is waiting for, so it is
            // logged at warn even when its severity is only `Warning`; the
            // recovery that ends it stays at info.
            if signal.active {
                tracing::warn!(
                    rule = %signal.rule_id,
                    severity = ?signal.severity,
                    detail = %signal.detail,
                    "local health threshold crossed"
                );
            } else {
                tracing::info!(
                    rule = %signal.rule_id,
                    detail = %signal.detail,
                    "local health threshold recovered"
                );
            }
        }
        // Every state decision in this tick, including the recovery branches
        // below, is derived from this one reading.
        let severity = alerts.worst_active_severity();
        let alert_details = alerts.active_details();
        // A source the collector could not read must be visible as a degraded
        // collector, not hidden behind an otherwise healthy snapshot.
        apply_health(&runtime, &sample.errors, severity, &alert_details).await;
        let _ = latest.send(Some(sample.clone()));
        runtime
            .trend
            .write()
            .await
            .push(rackio_core::TrendSample::from(&sample));
        pending.push(sample);
        if pending.len() >= 5 {
            let store_result = runtime.store.lock().await.insert_batch(&pending);
            match store_result {
                Ok(()) => {
                    pending.clear();
                    let mut health = runtime.health.write().await;
                    if health.storage_degraded {
                        health.storage_degraded = false;
                        project_health(&mut health, severity, &alert_details);
                    }
                }
                Err(error) => {
                    // A failed disk must not turn the live sampler into an unbounded queue.
                    pending.clear();
                    let mut health = runtime.health.write().await;
                    health.storage_degraded = true;
                    project_health(&mut health, severity, &alert_details);
                    tracing::error!(error = %error, "metric storage is degraded; live sampling continues");
                }
            }
        }
        prune_counter = prune_counter.saturating_add(1);
        if prune_counter >= 300 {
            prune_counter = 0;
            if let Err(error) = runtime
                .store
                .lock()
                .await
                // Monotonic: a forward clock step must not delete the whole
                // history, and a backward one must not stop pruning.
                .prune(clock.now_ms())
            {
                let mut health = runtime.health.write().await;
                health.storage_degraded = true;
                project_health(&mut health, severity, &alert_details);
                tracing::warn!(error = %error, "metric history pruning failed");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use rackio_core::HealthSnapshot;

    use super::{NodeState, project_health};

    fn healthy() -> HealthSnapshot {
        HealthSnapshot {
            state: NodeState::Healthy,
            collector_degraded: false,
            storage_degraded: false,
            remote_listener_degraded: false,
            details: Vec::new(),
        }
    }

    fn disk_alert() -> Vec<String> {
        vec![String::from(
            "Disk /data 93% is at or above the warning threshold of 90%",
        )]
    }

    #[test]
    fn storage_recovery_keeps_an_active_alert_severity() {
        // A recovered disk must not erase a threshold breach that is still live.
        let mut health = healthy();
        health.storage_degraded = true;
        project_health(&mut health, Some(NodeState::Critical), &disk_alert());
        assert_eq!(health.state, NodeState::Degraded);

        health.storage_degraded = false;
        project_health(&mut health, Some(NodeState::Critical), &disk_alert());

        assert_eq!(health.state, NodeState::Critical);
        assert_eq!(health.details, disk_alert());
    }

    #[test]
    fn collector_recovery_keeps_an_active_alert_severity() {
        let mut health = healthy();
        health.collector_degraded = true;
        health.state = NodeState::Degraded;

        health.collector_degraded = false;
        project_health(&mut health, Some(NodeState::Warning), &disk_alert());

        assert_eq!(health.state, NodeState::Warning);
    }

    #[test]
    fn degradation_outranks_an_alert_severity() {
        let mut health = healthy();
        health.storage_degraded = true;

        project_health(&mut health, Some(NodeState::Critical), &disk_alert());

        assert_eq!(health.state, NodeState::Degraded);
    }

    #[test]
    fn recovery_without_an_active_alert_reports_healthy() {
        let mut health = healthy();
        health.state = NodeState::Degraded;

        project_health(&mut health, None, &[]);

        assert_eq!(health.state, NodeState::Healthy);
        assert!(health.details.is_empty());
    }

    #[test]
    fn a_degradation_detail_leads_the_alert_lines_that_preceded_it() {
        // The viewer displays the first detail line beside the state. A machine
        // reading `degraded` while explaining itself with a disk percentage
        // would send the operator after the wrong problem.
        let mut health = healthy();
        project_health(&mut health, Some(NodeState::Warning), &disk_alert());
        assert_eq!(health.details, disk_alert());

        health.collector_degraded = true;
        project_health(&mut health, Some(NodeState::Warning), &disk_alert());

        assert_eq!(health.state, NodeState::Degraded);
        assert_eq!(
            health.details.first().map(String::as_str),
            Some("collector_degraded")
        );
        assert_eq!(health.details.len(), 2, "the live alert stays visible too");
    }

    #[test]
    fn a_cleared_alert_leaves_no_detail_line_behind() {
        // Details are derived, never accumulated: a stale line would keep
        // reporting a full disk after the space was freed.
        let mut health = healthy();
        health.storage_degraded = true;
        project_health(&mut health, Some(NodeState::Warning), &disk_alert());
        assert_eq!(health.details.len(), 2);

        health.storage_degraded = false;
        project_health(&mut health, None, &[]);

        assert!(health.details.is_empty());
        assert_eq!(health.state, NodeState::Healthy);
    }

    #[test]
    fn every_active_degradation_is_named_once() {
        let mut health = healthy();
        health.collector_degraded = true;
        health.storage_degraded = true;
        health.remote_listener_degraded = true;

        project_health(&mut health, None, &[]);
        project_health(&mut health, None, &[]);

        assert_eq!(
            health.details,
            vec![
                String::from("collector_degraded"),
                String::from("storage_degraded"),
                String::from("remote_listener_degraded"),
            ]
        );
    }
}
