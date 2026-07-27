use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::{MetricSample, NodeState};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Comparison {
    GreaterThanOrEqual,
    LessThanOrEqual,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AlertRule {
    pub id: String,
    pub metric: String,
    pub comparison: Comparison,
    pub threshold: f64,
    pub consecutive_samples: u32,
    pub severity: NodeState,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AlertSignal {
    pub rule_id: String,
    pub active: bool,
    pub severity: NodeState,
}

#[derive(Default)]
pub struct AlertEvaluator {
    counts: BTreeMap<String, u32>,
    active: BTreeMap<String, bool>,
}

impl AlertEvaluator {
    #[must_use]
    pub fn evaluate(&mut self, sample: &MetricSample, rules: &[AlertRule]) -> Vec<AlertSignal> {
        rules
            .iter()
            .filter_map(|rule| {
                // A metric that became unreadable is not a metric below its
                // threshold. Skipping the rule outright left an already-raised
                // alert latched forever with no recovery transition, so clear
                // it explicitly instead.
                let now_active = match metric_value(sample, &rule.metric) {
                    None => {
                        self.counts.insert(rule.id.clone(), 0);
                        false
                    }
                    Some(value) => {
                        let matches = match rule.comparison {
                            Comparison::GreaterThanOrEqual => value >= rule.threshold,
                            Comparison::LessThanOrEqual => value <= rule.threshold,
                        };
                        let count = self.counts.entry(rule.id.clone()).or_default();
                        *count = if matches { count.saturating_add(1) } else { 0 };
                        *count >= rule.consecutive_samples.max(1)
                    }
                };
                let was_active = self
                    .active
                    .insert(rule.id.clone(), now_active)
                    .unwrap_or(false);

                (now_active != was_active).then(|| AlertSignal {
                    rule_id: rule.id.clone(),
                    active: now_active,
                    severity: rule.severity,
                })
            })
            .collect()
    }

    /// The most severe currently-active severity, if any.
    #[must_use]
    pub fn worst_active_severity(&self, rules: &[AlertRule]) -> Option<NodeState> {
        rules
            .iter()
            .filter(|rule| self.active.get(&rule.id).copied().unwrap_or(false))
            .map(|rule| rule.severity)
            .max_by_key(|severity| match severity {
                NodeState::Critical => 2_u8,
                NodeState::Warning => 1,
                _ => 0,
            })
    }
}

fn metric_value(sample: &MetricSample, metric: &str) -> Option<f64> {
    match metric {
        "cpu_percent" => sample.cpu_percent.map(f64::from),
        "memory_percent" => {
            let used = sample.memory_used_bytes?;
            let total = sample.memory_total_bytes?;
            (total > 0).then(|| percentage(used, total))
        }
        "disk_percent" => sample
            .disks
            .iter()
            .filter(|disk| disk.total_bytes > 0)
            .map(|disk| percentage(disk.used_bytes, disk.total_bytes))
            .max_by(f64::total_cmp),
        _ => None,
    }
}

fn percentage(used: u64, total: u64) -> f64 {
    let basis_points = u128::from(used)
        .saturating_mul(10_000)
        .checked_div(u128::from(total))
        .unwrap_or_default();
    let basis_points = u32::try_from(basis_points).unwrap_or(u32::MAX);
    f64::from(basis_points) / 100.0
}

#[cfg(test)]
mod tests {
    use crate::{AlertEvaluator, AlertRule, Comparison, MetricSample, NodeState};

    fn sample(cpu: f32) -> MetricSample {
        MetricSample {
            timestamp_ms: 1,
            sequence: 1,
            cpu_percent: Some(cpu),
            memory_used_bytes: None,
            memory_total_bytes: None,
            swap_used_bytes: None,
            swap_total_bytes: None,
            disks: Vec::new(),
            network: None,
            uptime_seconds: 1,
            errors: Vec::new(),
        }
    }

    #[test]
    fn emits_only_transitions_after_required_samples() {
        let rule = AlertRule {
            id: "cpu".into(),
            metric: "cpu_percent".into(),
            comparison: Comparison::GreaterThanOrEqual,
            threshold: 80.0,
            consecutive_samples: 2,
            severity: NodeState::Warning,
        };
        let mut evaluator = AlertEvaluator::default();

        assert!(
            evaluator
                .evaluate(&sample(90.0), std::slice::from_ref(&rule))
                .is_empty()
        );
        let raised = evaluator.evaluate(&sample(90.0), std::slice::from_ref(&rule));
        assert!(raised[0].active);
        assert!(
            evaluator
                .evaluate(&sample(95.0), std::slice::from_ref(&rule))
                .is_empty()
        );
        let recovered = evaluator.evaluate(&sample(20.0), std::slice::from_ref(&rule));
        assert!(!recovered[0].active);
    }

    #[test]
    fn an_alert_clears_when_its_metric_becomes_unreadable() {
        let rule = AlertRule {
            id: "cpu".into(),
            metric: "cpu_percent".into(),
            comparison: Comparison::GreaterThanOrEqual,
            threshold: 80.0,
            consecutive_samples: 1,
            severity: NodeState::Critical,
        };
        let mut evaluator = AlertEvaluator::default();

        let raised = evaluator.evaluate(&sample(90.0), std::slice::from_ref(&rule));
        assert!(raised[0].active);
        assert_eq!(
            evaluator.worst_active_severity(std::slice::from_ref(&rule)),
            Some(NodeState::Critical)
        );

        // The CPU source becomes unreadable. Without an explicit recovery the
        // operator would see a permanently latched critical alert.
        let mut unreadable = sample(0.0);
        unreadable.cpu_percent = None;
        let cleared = evaluator.evaluate(&unreadable, std::slice::from_ref(&rule));
        assert_eq!(cleared.len(), 1);
        assert!(!cleared[0].active);
        assert_eq!(
            evaluator.worst_active_severity(std::slice::from_ref(&rule)),
            None
        );
    }

    #[test]
    fn unsupported_metric_does_not_create_a_false_alert() {
        let rule = AlertRule {
            id: "gpu".into(),
            metric: "gpu_percent".into(),
            comparison: Comparison::GreaterThanOrEqual,
            threshold: 1.0,
            consecutive_samples: 1,
            severity: NodeState::Critical,
        };

        assert!(
            AlertEvaluator::default()
                .evaluate(&sample(90.0), &[rule])
                .is_empty()
        );
    }
}
