//! Metric sampling, persistence batching, and health-state projection.

use std::{sync::Arc, time::Duration};

use rackio_core::{NodeState, SystemCollector};
use rackio_iroh::NodeRuntime;
use tokio::sync::watch;

/// The single owner of the published node state.
///
/// Degradation still wins: a machine whose collector, storage or remote
/// listener is broken is reported as `Degraded` rather than as a threshold
/// verdict, because the underlying data is no longer trustworthy. Once every
/// subsystem is healthy again the state is the worst active alert severity, so
/// clearing a degradation flag can never claim `Healthy` while an operator
/// threshold is still breached.
fn derive_state(health: &mut rackio_core::HealthSnapshot, severity: Option<NodeState>) {
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
async fn apply_collector_health(
    runtime: &NodeRuntime,
    errors: &[rackio_core::CollectorError],
    severity: Option<NodeState>,
) {
    let degraded = !errors.is_empty();
    let mut health = runtime.health.write().await;
    if health.collector_degraded == degraded {
        return;
    }
    health.collector_degraded = degraded;
    if degraded {
        if !health
            .details
            .iter()
            .any(|detail| detail == "collector_degraded")
        {
            health.details.push(String::from("collector_degraded"));
        }
        tracing::warn!(
            sources = ?errors.iter().map(|error| error.source.as_str()).collect::<Vec<_>>(),
            "one or more metric sources are unreadable on this host"
        );
    } else {
        health
            .details
            .retain(|detail| detail != "collector_degraded");
    }
    derive_state(&mut health, severity);
}

/// Publish an operator-configured threshold breach as the machine's state.
async fn apply_alert_health(runtime: &NodeRuntime, severity: Option<NodeState>) {
    let mut health = runtime.health.write().await;
    derive_state(&mut health, severity);
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
            tracing::info!(
                rule = %signal.rule_id,
                active = signal.active,
                severity = ?signal.severity,
                "local health threshold transition"
            );
        }
        // Every state decision in this tick, including the recovery branches
        // below, is derived from this one severity reading.
        let severity = alerts.worst_active_severity(&alert_rules);
        // A source the collector could not read must be visible as a degraded
        // collector, not hidden behind an otherwise healthy snapshot.
        apply_collector_health(&runtime, &sample.errors, severity).await;
        apply_alert_health(&runtime, severity).await;
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
                        health.details.retain(|detail| detail != "storage_degraded");
                        derive_state(&mut health, severity);
                    }
                }
                Err(error) => {
                    // A failed disk must not turn the live sampler into an unbounded queue.
                    pending.clear();
                    let mut health = runtime.health.write().await;
                    health.storage_degraded = true;
                    derive_state(&mut health, severity);
                    if !health
                        .details
                        .iter()
                        .any(|detail| detail == "storage_degraded")
                    {
                        health.details.push(String::from("storage_degraded"));
                    }
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
                health.state = NodeState::Degraded;
                if !health
                    .details
                    .iter()
                    .any(|detail| detail == "storage_degraded")
                {
                    health.details.push(String::from("storage_degraded"));
                }
                tracing::warn!(error = %error, "metric history pruning failed");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use rackio_core::HealthSnapshot;

    use super::{NodeState, derive_state};

    fn healthy() -> HealthSnapshot {
        HealthSnapshot {
            state: NodeState::Healthy,
            collector_degraded: false,
            storage_degraded: false,
            remote_listener_degraded: false,
            details: Vec::new(),
        }
    }

    #[test]
    fn storage_recovery_keeps_an_active_alert_severity() {
        // A recovered disk must not erase a threshold breach that is still live.
        let mut health = healthy();
        health.storage_degraded = true;
        health.state = NodeState::Degraded;

        health.storage_degraded = false;
        derive_state(&mut health, Some(NodeState::Critical));

        assert_eq!(health.state, NodeState::Critical);
    }

    #[test]
    fn collector_recovery_keeps_an_active_alert_severity() {
        let mut health = healthy();
        health.collector_degraded = true;
        health.state = NodeState::Degraded;

        health.collector_degraded = false;
        derive_state(&mut health, Some(NodeState::Warning));

        assert_eq!(health.state, NodeState::Warning);
    }

    #[test]
    fn degradation_outranks_an_alert_severity() {
        let mut health = healthy();
        health.storage_degraded = true;

        derive_state(&mut health, Some(NodeState::Critical));

        assert_eq!(health.state, NodeState::Degraded);
    }

    #[test]
    fn recovery_without_an_active_alert_reports_healthy() {
        let mut health = healthy();
        health.state = NodeState::Degraded;

        derive_state(&mut health, None);

        assert_eq!(health.state, NodeState::Healthy);
    }
}
