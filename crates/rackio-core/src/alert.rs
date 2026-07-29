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
    use crate::{AlertEvaluator, AlertRule, Comparison, DiskMetric, MetricSample, NodeState};

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

    fn rule(id: &str, metric: &str, comparison: Comparison, threshold: f64) -> AlertRule {
        AlertRule {
            id: id.into(),
            metric: metric.into(),
            comparison,
            threshold,
            consecutive_samples: 1,
            severity: NodeState::Warning,
        }
    }

    #[test]
    fn a_less_than_or_equal_rule_fires_below_its_threshold() {
        // "Alert when the value falls to or under X" is the shape every
        // free-capacity rule takes. Inverting this comparison would raise the
        // alert exactly when the machine is healthy.
        let rule = rule("idle", "cpu_percent", Comparison::LessThanOrEqual, 10.0);
        let mut evaluator = AlertEvaluator::default();

        assert!(
            evaluator
                .evaluate(&sample(50.0), std::slice::from_ref(&rule))
                .is_empty(),
            "a value above the threshold must not raise a less-than-or-equal rule"
        );

        let raised = evaluator.evaluate(&sample(5.0), std::slice::from_ref(&rule));
        assert!(raised[0].active);

        let recovered = evaluator.evaluate(&sample(50.0), std::slice::from_ref(&rule));
        assert!(!recovered[0].active);
    }

    #[test]
    fn both_comparisons_treat_the_threshold_itself_as_matching() {
        for comparison in [Comparison::GreaterThanOrEqual, Comparison::LessThanOrEqual] {
            let rule = rule("edge", "cpu_percent", comparison, 42.0);
            let raised =
                AlertEvaluator::default().evaluate(&sample(42.0), std::slice::from_ref(&rule));
            assert!(
                raised[0].active,
                "{comparison:?} must include its own threshold"
            );
        }
    }

    #[test]
    fn critical_outranks_warning_among_active_alerts() {
        // The node badge shows one state. If ordering collapses, a machine with
        // a critical alert can present as merely warning.
        let warning = AlertRule {
            severity: NodeState::Warning,
            ..rule("warn", "cpu_percent", Comparison::GreaterThanOrEqual, 10.0)
        };
        let critical = AlertRule {
            severity: NodeState::Critical,
            ..rule("crit", "cpu_percent", Comparison::GreaterThanOrEqual, 20.0)
        };
        let degraded = AlertRule {
            severity: NodeState::Degraded,
            ..rule(
                "degraded",
                "cpu_percent",
                Comparison::GreaterThanOrEqual,
                10.0,
            )
        };
        let rules = [warning.clone(), critical.clone(), degraded.clone()];
        let mut evaluator = AlertEvaluator::default();

        let _ = evaluator.evaluate(&sample(90.0), &rules);
        assert_eq!(
            evaluator.worst_active_severity(&rules),
            Some(NodeState::Critical)
        );

        // Below the critical threshold the warning rule alone stays active, and
        // an unranked severity must not displace it.
        let _ = evaluator.evaluate(&sample(15.0), &rules);
        assert_eq!(
            evaluator.worst_active_severity(&rules),
            Some(NodeState::Warning)
        );

        // With only the unranked rule active there is still an answer.
        let _ = evaluator.evaluate(&sample(5.0), &rules);
        assert_eq!(evaluator.worst_active_severity(&rules), None);
    }

    #[test]
    fn memory_percent_is_derived_from_used_over_total() {
        let rule = rule(
            "mem",
            "memory_percent",
            Comparison::GreaterThanOrEqual,
            75.0,
        );
        let mut at_threshold = sample(0.0);
        at_threshold.memory_used_bytes = Some(3);
        at_threshold.memory_total_bytes = Some(4);

        let raised = AlertEvaluator::default().evaluate(&at_threshold, std::slice::from_ref(&rule));
        assert!(raised[0].active, "3 of 4 bytes used is 75 percent");

        let mut below = at_threshold.clone();
        below.memory_used_bytes = Some(2);
        assert!(
            AlertEvaluator::default()
                .evaluate(&below, std::slice::from_ref(&rule))
                .is_empty(),
            "2 of 4 bytes used is 50 percent"
        );
    }

    #[test]
    fn memory_percent_is_absent_when_the_total_is_zero_or_missing() {
        // A zero total is an unreadable source, not a fully used machine. The
        // guard also keeps the percentage from dividing by zero.
        let rule = rule("mem", "memory_percent", Comparison::GreaterThanOrEqual, 0.0);

        for (used, total) in [(Some(1), Some(0)), (Some(1), None), (None, Some(4))] {
            let mut unreadable = sample(0.0);
            unreadable.memory_used_bytes = used;
            unreadable.memory_total_bytes = total;
            assert!(
                AlertEvaluator::default()
                    .evaluate(&unreadable, std::slice::from_ref(&rule))
                    .is_empty(),
                "used {used:?} of total {total:?} must not resolve to a percentage"
            );
        }
    }

    #[test]
    fn disk_percent_takes_the_fullest_filesystem_and_skips_empty_totals() {
        let nearly_full = rule("disk", "disk_percent", Comparison::GreaterThanOrEqual, 90.0);
        let mut with_disks = sample(0.0);
        with_disks.disks = vec![
            DiskMetric {
                mount: "/".into(),
                total_bytes: 100,
                used_bytes: 10,
            },
            DiskMetric {
                mount: "/data".into(),
                total_bytes: 100,
                used_bytes: 95,
            },
            // A pseudo-filesystem reporting nothing must not be ranked at all.
            DiskMetric {
                mount: "/proc".into(),
                total_bytes: 0,
                used_bytes: 50,
            },
        ];

        let raised =
            AlertEvaluator::default().evaluate(&with_disks, std::slice::from_ref(&nearly_full));
        assert!(raised[0].active, "the fullest filesystem is 95 percent");

        let mut only_zero_total = sample(0.0);
        only_zero_total.disks = vec![DiskMetric {
            mount: "/proc".into(),
            total_bytes: 0,
            used_bytes: 50,
        }];
        // The threshold is zero, so a zero-total filesystem admitted by a
        // widened guard would compute 0 percent and raise the alert. Only
        // excluding it entirely leaves the metric unresolved and silent.
        let always_matching = rule("any", "disk_percent", Comparison::GreaterThanOrEqual, 0.0);
        assert!(
            AlertEvaluator::default()
                .evaluate(&only_zero_total, std::slice::from_ref(&always_matching))
                .is_empty(),
            "a zero-total filesystem must not produce a disk percentage"
        );
    }

    #[test]
    fn percentage_converts_basis_points_to_percent() {
        // Guards the /100.0 conversion: a multiply here would report 10 000x.
        assert!((super::percentage(1, 4) - 25.0).abs() < f64::EPSILON);
        assert!((super::percentage(1, 1) - 100.0).abs() < f64::EPSILON);
        assert!((super::percentage(0, 4) - 0.0).abs() < f64::EPSILON);
        assert!((super::percentage(1, 3) - 33.33).abs() < 0.005);
        // A zero total cannot divide; the caller filters it, and the fallback
        // must still be a real number rather than a panic or an infinity.
        assert!((super::percentage(1, 0) - 0.0).abs() < f64::EPSILON);
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
