use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::{MetricSample, NodeState};

/// Samples per minute at the two-second sampling cadence.
const SAMPLES_PER_MINUTE: u32 = 30;

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

/// The levels Rackio treats as "generally worth acting on" when an operator has
/// said nothing about a machine.
///
/// Each one is chosen so that a machine which is merely busy stays quiet and a
/// machine heading for a failure does not:
///
/// - **Disk** at 90 % still leaves headroom to act, 95 % spends the usual
///   reserve. Free space is finite and non-renewable, so six seconds is enough
///   confirmation.
/// - **Memory** at 90 % for a full minute means the working set no longer fits
///   and the machine is about to trade throughput for swap; 97 % is OOM-killer
///   territory. The minute is what separates it from a large transient job.
/// - **CPU** at 90 % for five minutes is saturation rather than work: a burst
///   is normal, a sustained pin means requests are queueing. It is a warning
///   only — a busy processor is not an outage — and it is the rule build and
///   render machines most often turn off.
/// - **Temperature** is measured against the limit the hardware itself
///   publishes, never a number Rackio picked: within 5 °C of that limit is the
///   warning, reaching it is critical. A machine whose OS publishes no limit
///   resolves neither rule instead of being judged against a guess.
///
/// Every level is a default, not a limit. `rackio alerts` changes a threshold,
/// switches one rule off, or turns local alerting off entirely.
#[must_use]
pub fn default_alert_rules() -> Vec<AlertRule> {
    let rule = |id: &str, metric: &str, comparison, threshold, samples, severity| AlertRule {
        id: String::from(id),
        metric: String::from(metric),
        comparison,
        threshold,
        consecutive_samples: samples,
        severity,
    };
    vec![
        rule(
            "disk-capacity-warning",
            "disk_percent",
            Comparison::GreaterThanOrEqual,
            90.0,
            3,
            NodeState::Warning,
        ),
        rule(
            "disk-capacity-critical",
            "disk_percent",
            Comparison::GreaterThanOrEqual,
            95.0,
            3,
            NodeState::Critical,
        ),
        rule(
            "memory-pressure-warning",
            "memory_percent",
            Comparison::GreaterThanOrEqual,
            90.0,
            SAMPLES_PER_MINUTE,
            NodeState::Warning,
        ),
        rule(
            "memory-pressure-critical",
            "memory_percent",
            Comparison::GreaterThanOrEqual,
            97.0,
            SAMPLES_PER_MINUTE,
            NodeState::Critical,
        ),
        rule(
            "cpu-saturation-warning",
            "cpu_percent",
            Comparison::GreaterThanOrEqual,
            90.0,
            SAMPLES_PER_MINUTE * 5,
            NodeState::Warning,
        ),
        rule(
            "temperature-headroom-warning",
            "temperature_headroom_celsius",
            Comparison::LessThanOrEqual,
            5.0,
            15,
            NodeState::Warning,
        ),
        rule(
            "temperature-critical",
            "temperature_headroom_celsius",
            Comparison::LessThanOrEqual,
            0.0,
            15,
            NodeState::Critical,
        ),
    ]
}

/// Where an effective rule came from, so `rackio alerts list` can say whether a
/// level is Rackio's or the operator's.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AlertRuleSource {
    BuiltIn,
    Configured,
}

/// An operator's change to the shipped rules.
///
/// Only the fields an operator actually wants to change are present, so raising
/// one disk threshold does not require restating every other level — and a
/// later release that adds or retunes a default still reaches a machine whose
/// operator only ever touched one rule. A configuration entry naming an unknown
/// rule has to describe it in full; a half-described one is rejected rather
/// than quietly ignored, because a rule that silently never fires is
/// indistinguishable from a healthy machine.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AlertRuleConfig {
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metric: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub comparison: Option<Comparison>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub threshold: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub consecutive_samples: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub severity: Option<NodeState>,
    /// `false` switches the rule off while keeping it visible and restorable.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub enabled: Option<bool>,
}

impl AlertRuleConfig {
    #[must_use]
    pub fn new(id: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            metric: None,
            comparison: None,
            threshold: None,
            consecutive_samples: None,
            severity: None,
            enabled: None,
        }
    }
}

/// One effective rule, including the ones an operator switched off: a disabled
/// rule an operator cannot see is a rule they cannot turn back on.
#[derive(Debug, Clone, PartialEq)]
pub struct ResolvedAlertRule {
    pub rule: AlertRule,
    pub enabled: bool,
    pub source: AlertRuleSource,
}

/// The metrics a rule may name. A typo here would produce a rule that can never
/// fire, so resolution rejects anything else.
pub const ALERT_METRICS: [&str; 5] = [
    "cpu_percent",
    "memory_percent",
    "swap_percent",
    "disk_percent",
    "temperature_headroom_celsius",
];

/// Merge operator changes over the shipped rules.
///
/// Fails closed on anything that would produce a rule which cannot fire — an
/// unknown metric, an undescribed new rule, a non-finite threshold, a severity
/// that is not a health level — because such a rule is silent in exactly the
/// same way a healthy machine is.
///
/// # Errors
/// Returns the operator-facing reason the configuration cannot be applied.
pub fn resolve_alert_rules(
    overrides: &[AlertRuleConfig],
) -> Result<Vec<ResolvedAlertRule>, String> {
    let mut resolved: Vec<ResolvedAlertRule> = default_alert_rules()
        .into_iter()
        .map(|rule| ResolvedAlertRule {
            rule,
            enabled: true,
            source: AlertRuleSource::BuiltIn,
        })
        .collect();
    for change in overrides {
        if let Some(known) = resolved.iter_mut().find(|known| known.rule.id == change.id) {
            apply_override(&mut known.rule, change);
            known.enabled = change.enabled.unwrap_or(known.enabled);
            if changes_a_level(change) {
                known.source = AlertRuleSource::Configured;
            }
        } else {
            resolved.push(ResolvedAlertRule {
                rule: new_rule(change)?,
                enabled: change.enabled.unwrap_or(true),
                source: AlertRuleSource::Configured,
            });
        }
    }
    for entry in &resolved {
        validate(&entry.rule)?;
    }
    Ok(resolved)
}

/// A duplicate id would otherwise let a later entry silently win.
fn new_rule(change: &AlertRuleConfig) -> Result<AlertRule, String> {
    let missing = |field: &str| {
        format!(
            "alert rule `{}` is not one Rackio ships, so it must define `{field}`",
            change.id
        )
    };
    Ok(AlertRule {
        id: change.id.clone(),
        metric: change.metric.clone().ok_or_else(|| missing("metric"))?,
        comparison: change.comparison.ok_or_else(|| missing("comparison"))?,
        threshold: change.threshold.ok_or_else(|| missing("threshold"))?,
        consecutive_samples: change.consecutive_samples.unwrap_or(1),
        severity: change.severity.ok_or_else(|| missing("severity"))?,
    })
}

fn apply_override(rule: &mut AlertRule, change: &AlertRuleConfig) {
    if let Some(metric) = change.metric.clone() {
        rule.metric = metric;
    }
    if let Some(comparison) = change.comparison {
        rule.comparison = comparison;
    }
    if let Some(threshold) = change.threshold {
        rule.threshold = threshold;
    }
    if let Some(samples) = change.consecutive_samples {
        rule.consecutive_samples = samples;
    }
    if let Some(severity) = change.severity {
        rule.severity = severity;
    }
}

/// Switching a shipped rule off is not the same as retuning it: a rule an
/// operator only disabled should still follow the shipped level if they turn it
/// back on after an upgrade.
const fn changes_a_level(change: &AlertRuleConfig) -> bool {
    change.metric.is_some()
        || change.comparison.is_some()
        || change.threshold.is_some()
        || change.consecutive_samples.is_some()
        || change.severity.is_some()
}

fn validate(rule: &AlertRule) -> Result<(), String> {
    if !ALERT_METRICS.contains(&rule.metric.as_str()) {
        return Err(format!(
            "alert rule `{}` names the unknown metric `{}`; known metrics are {}",
            rule.id,
            rule.metric,
            ALERT_METRICS.join(", ")
        ));
    }
    if !rule.threshold.is_finite() {
        return Err(format!(
            "alert rule `{}` has a threshold that is not a finite number",
            rule.id
        ));
    }
    if !matches!(rule.severity, NodeState::Warning | NodeState::Critical) {
        return Err(format!(
            "alert rule `{}` must report `warning` or `critical`; `{:?}` is not a level a threshold can reach",
            rule.id, rule.severity
        ));
    }
    Ok(())
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AlertSignal {
    pub rule_id: String,
    pub active: bool,
    pub severity: NodeState,
    /// What an operator needs to read on the notification: which resource, the
    /// value that crossed, and the threshold it crossed. A rule id alone does
    /// not say which filesystem to clear.
    pub detail: String,
}

#[derive(Debug, Clone)]
struct ActiveAlert {
    severity: NodeState,
    detail: String,
}

#[derive(Default)]
pub struct AlertEvaluator {
    counts: BTreeMap<String, u32>,
    active: BTreeMap<String, ActiveAlert>,
}

impl AlertEvaluator {
    #[must_use]
    pub fn evaluate(&mut self, sample: &MetricSample, rules: &[AlertRule]) -> Vec<AlertSignal> {
        // A rule the caller no longer supplies must not keep a severity or a
        // detail line alive for a threshold nobody is evaluating any more.
        self.counts.retain(|id, _| contains_rule(rules, id));
        self.active.retain(|id, _| contains_rule(rules, id));
        rules
            .iter()
            .filter_map(|rule| {
                // A metric that became unreadable is not a metric below its
                // threshold. Skipping the rule outright left an already-raised
                // alert latched forever with no recovery transition, so clear
                // it explicitly instead.
                let breach = match metric_reading(sample, &rule.metric) {
                    None => {
                        self.counts.insert(rule.id.clone(), 0);
                        None
                    }
                    Some(reading) => {
                        let matches = match rule.comparison {
                            Comparison::GreaterThanOrEqual => reading.value >= rule.threshold,
                            Comparison::LessThanOrEqual => reading.value <= rule.threshold,
                        };
                        let count = self.counts.entry(rule.id.clone()).or_default();
                        *count = if matches { count.saturating_add(1) } else { 0 };
                        (*count >= rule.consecutive_samples.max(1)).then_some(reading)
                    }
                };
                let was_active = self.active.remove(&rule.id).is_some();
                // Rewritten on every tick while active: the fullest filesystem
                // can change, and a stale mount name would send an operator to
                // the wrong disk.
                let detail = breach.map(|reading| breach_description(rule, &reading));
                if let Some(detail) = detail.clone() {
                    self.active.insert(
                        rule.id.clone(),
                        ActiveAlert {
                            severity: rule.severity,
                            detail,
                        },
                    );
                }
                let now_active = detail.is_some();

                (now_active != was_active).then(|| AlertSignal {
                    rule_id: rule.id.clone(),
                    active: now_active,
                    severity: rule.severity,
                    detail: detail.unwrap_or_else(|| recovery_description(rule)),
                })
            })
            .collect()
    }

    /// The most severe currently-active severity, if any.
    #[must_use]
    pub fn worst_active_severity(&self) -> Option<NodeState> {
        self.active
            .values()
            .map(|alert| alert.severity)
            .max_by_key(|severity| severity_rank(*severity))
    }

    /// One human-readable line per active alert, most severe first, for the
    /// health snapshot an operator and every paired viewer read.
    #[must_use]
    pub fn active_details(&self) -> Vec<String> {
        let mut active: Vec<&ActiveAlert> = self.active.values().collect();
        // Stable, so equally severe alerts keep their rule-id order instead of
        // reshuffling the displayed line between ticks.
        active.sort_by_key(|alert| std::cmp::Reverse(severity_rank(alert.severity)));
        active
            .into_iter()
            .map(|alert| alert.detail.clone())
            .collect()
    }
}

fn contains_rule(rules: &[AlertRule], id: &str) -> bool {
    rules.iter().any(|rule| rule.id == id)
}

const fn severity_rank(severity: NodeState) -> u8 {
    match severity {
        NodeState::Critical => 2,
        NodeState::Warning => 1,
        _ => 0,
    }
}

/// A metric value together with what it belongs to, so a machine with several
/// filesystems or sensors can name the one that breached.
struct Reading {
    value: f64,
    scope: Option<String>,
}

fn metric_reading(sample: &MetricSample, metric: &str) -> Option<Reading> {
    let unscoped = |value: f64| Reading { value, scope: None };
    match metric {
        "cpu_percent" => sample.cpu_percent.map(f64::from).map(unscoped),
        "memory_percent" => {
            let used = sample.memory_used_bytes?;
            let total = sample.memory_total_bytes?;
            (total > 0).then(|| unscoped(percentage(used, total)))
        }
        // A machine with swap disabled reports a zero total, which is not a
        // machine whose swap is empty; it has no swap percentage at all.
        "swap_percent" => {
            let used = sample.swap_used_bytes?;
            let total = sample.swap_total_bytes?;
            (total > 0).then(|| unscoped(percentage(used, total)))
        }
        "disk_percent" => sample
            .disks
            .iter()
            .filter(|disk| disk.total_bytes > 0)
            .map(|disk| Reading {
                value: percentage(disk.used_bytes, disk.total_bytes),
                scope: Some(disk.mount.clone()),
            })
            .max_by(|left, right| left.value.total_cmp(&right.value)),
        // The hottest sensor, so one rule covers a machine whose sensor labels
        // the operator cannot know in advance. A host with no readable sensor
        // resolves to `None`, which clears the rule instead of latching it.
        "temperature_celsius" => sample.temperature.as_ref().map(|temperature| Reading {
            value: f64::from(temperature.celsius),
            scope: Some(temperature.label.clone()),
        }),
        // How far the hottest sensor still is from the limit the hardware
        // itself declares. This is the only temperature level Rackio can state
        // without inventing one, and a machine whose OS publishes no limit
        // resolves to `None` rather than being judged against a guess.
        "temperature_headroom_celsius" => {
            let temperature = sample.temperature.as_ref()?;
            let critical = temperature.critical_celsius?;
            Some(Reading {
                value: f64::from(critical - temperature.celsius),
                scope: Some(temperature.label.clone()),
            })
        }
        _ => None,
    }
}

fn metric_subject(metric: &str) -> &str {
    match metric {
        "cpu_percent" => "CPU",
        "memory_percent" => "Memory",
        "swap_percent" => "Swap",
        "disk_percent" => "Disk",
        "temperature_celsius" => "Temperature",
        // Named for what the number is. "Temperature 3 °C" would read as a cold
        // machine when it means three degrees from the hardware's own limit.
        "temperature_headroom_celsius" => "Temperature headroom",
        other => other,
    }
}

fn metric_unit(metric: &str) -> &'static str {
    if metric.ends_with("_celsius") {
        " °C"
    } else {
        "%"
    }
}

const fn comparison_phrase(comparison: Comparison) -> &'static str {
    match comparison {
        Comparison::GreaterThanOrEqual => "at or above",
        Comparison::LessThanOrEqual => "at or below",
    }
}

fn severity_label(severity: NodeState) -> &'static str {
    match severity {
        NodeState::Critical => "critical",
        NodeState::Warning => "warning",
        _ => "alert",
    }
}

/// One decimal, and no trailing `.0`.
///
/// Rounding to whole units printed a 94.6 % filesystem as "95% is at or above
/// the warning threshold of 90%", which reads as a machine that should have
/// been critical and was not.
fn number(value: f64) -> String {
    let mut rounded = format!("{value:.1}");
    if rounded.ends_with(".0") {
        rounded.truncate(rounded.len().saturating_sub(2));
    }
    rounded
}

fn breach_description(rule: &AlertRule, reading: &Reading) -> String {
    let unit = metric_unit(&rule.metric);
    let subject = match reading.scope.as_deref() {
        Some(scope) => format!("{} {scope}", metric_subject(&rule.metric)),
        None => String::from(metric_subject(&rule.metric)),
    };
    format!(
        "{subject} {}{unit} is {} the {} threshold of {}{unit}",
        number(reading.value),
        comparison_phrase(rule.comparison),
        severity_label(rule.severity),
        number(rule.threshold),
    )
}

fn recovery_description(rule: &AlertRule) -> String {
    let unit = metric_unit(&rule.metric);
    format!(
        "{} left the {} threshold of {}{unit}",
        metric_subject(&rule.metric),
        severity_label(rule.severity),
        number(rule.threshold),
    )
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
    use crate::{
        AlertEvaluator, AlertRule, Comparison, DiskMetric, MetricSample, NodeState,
        TemperatureMetric,
    };

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
            temperature: None,
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
        assert_eq!(evaluator.worst_active_severity(), Some(NodeState::Critical));

        // The CPU source becomes unreadable. Without an explicit recovery the
        // operator would see a permanently latched critical alert.
        let mut unreadable = sample(0.0);
        unreadable.cpu_percent = None;
        let cleared = evaluator.evaluate(&unreadable, std::slice::from_ref(&rule));
        assert_eq!(cleared.len(), 1);
        assert!(!cleared[0].active);
        assert_eq!(evaluator.worst_active_severity(), None);
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
        assert_eq!(evaluator.worst_active_severity(), Some(NodeState::Critical));

        // Below the critical threshold the warning rule alone stays active, and
        // an unranked severity must not displace it.
        let _ = evaluator.evaluate(&sample(15.0), &rules);
        assert_eq!(evaluator.worst_active_severity(), Some(NodeState::Warning));

        // With only the unranked rule active there is still an answer.
        let _ = evaluator.evaluate(&sample(5.0), &rules);
        assert_eq!(evaluator.worst_active_severity(), None);
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
    fn a_temperature_rule_follows_the_hottest_sensor_and_clears_without_one() {
        let hot = rule(
            "temp",
            "temperature_celsius",
            Comparison::GreaterThanOrEqual,
            85.0,
        );
        let mut with_sensors = sample(0.0);
        with_sensors.temperature = Some(TemperatureMetric {
            label: "CPU die".into(),
            celsius: 92.0,
            critical_celsius: Some(100.0),
            sensor_count: 2,
        });

        let mut evaluator = AlertEvaluator::default();
        let raised = evaluator.evaluate(&with_sensors, std::slice::from_ref(&hot));
        assert!(raised[0].active, "the hottest sensor is 92 °C");

        // The sensor stops reporting. A latched critical alert on a machine
        // whose temperature is simply unknown would be a false alarm.
        let mut unreadable = with_sensors.clone();
        unreadable.temperature = None;
        let cleared = evaluator.evaluate(&unreadable, std::slice::from_ref(&hot));
        assert_eq!(cleared.len(), 1);
        assert!(!cleared[0].active);
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
    fn every_shipped_rule_is_a_level_the_evaluator_can_actually_reach() {
        // The defaults are the product's only voice on an untouched machine. A
        // rule naming a metric the evaluator does not resolve, or a severity a
        // node badge cannot show, would be silent in exactly the way a healthy
        // machine is.
        let resolved = super::resolve_alert_rules(&[]).unwrap_or_else(|error| panic!("{error}"));

        assert_eq!(resolved.len(), super::default_alert_rules().len());
        for entry in &resolved {
            assert!(entry.enabled, "{} ships disabled", entry.rule.id);
            assert_eq!(entry.source, super::AlertRuleSource::BuiltIn);
            assert!(
                super::ALERT_METRICS.contains(&entry.rule.metric.as_str()),
                "{} names {}",
                entry.rule.id,
                entry.rule.metric
            );
            assert!(
                matches!(
                    entry.rule.severity,
                    NodeState::Warning | NodeState::Critical
                ),
                "{} reports {:?}",
                entry.rule.id,
                entry.rule.severity
            );
        }
    }

    #[test]
    fn a_busy_machine_is_not_alerted_on_before_the_condition_is_sustained() {
        // The line between "busy" and "in trouble" is time. If these windows
        // collapse, every compile and every backup becomes an alert and the
        // operator learns to ignore the product.
        let by_id = |id: &str| {
            super::default_alert_rules()
                .into_iter()
                .find(|rule| rule.id == id)
                .unwrap_or_else(|| panic!("{id} is not a shipped rule"))
        };

        assert_eq!(by_id("cpu-saturation-warning").consecutive_samples, 150);
        assert_eq!(by_id("memory-pressure-warning").consecutive_samples, 30);
        assert_eq!(by_id("disk-capacity-warning").consecutive_samples, 3);
        assert_eq!(
            by_id("cpu-saturation-warning").severity,
            NodeState::Warning,
            "a saturated processor is not an outage"
        );
    }

    #[test]
    fn the_shipped_temperature_rules_defer_to_the_hardwares_own_limit() {
        // Rackio has no basis for an absolute temperature. Both rules measure
        // headroom against the limit the board publishes.
        for id in ["temperature-headroom-warning", "temperature-critical"] {
            let rule = super::default_alert_rules()
                .into_iter()
                .find(|rule| rule.id == id)
                .unwrap_or_else(|| panic!("{id} is not a shipped rule"));
            assert_eq!(rule.metric, "temperature_headroom_celsius");
            assert_eq!(rule.comparison, Comparison::LessThanOrEqual);
        }
    }

    fn with_disk(used: u64, total: u64, mount: &str) -> MetricSample {
        let mut with_disks = sample(0.0);
        with_disks.disks = vec![DiskMetric {
            mount: mount.into(),
            total_bytes: total,
            used_bytes: used,
        }];
        with_disks
    }

    #[test]
    fn a_filling_disk_reaches_warning_then_critical_under_the_default_rules() {
        let rules = super::default_alert_rules();
        let mut evaluator = AlertEvaluator::default();

        // A healthy machine stays silent no matter how long it runs.
        for _ in 0..5 {
            assert!(
                evaluator
                    .evaluate(&with_disk(50, 100, "/"), &rules)
                    .is_empty()
            );
        }
        assert_eq!(evaluator.worst_active_severity(), None);
        assert!(evaluator.active_details().is_empty());

        // 91 % crosses the warning level, but only after the required run of
        // samples: the first two ticks must stay quiet.
        let filling = with_disk(91, 100, "/");
        assert!(evaluator.evaluate(&filling, &rules).is_empty());
        assert!(evaluator.evaluate(&filling, &rules).is_empty());
        let raised = evaluator.evaluate(&filling, &rules);
        assert_eq!(raised.len(), 1);
        assert!(raised[0].active);
        assert_eq!(raised[0].severity, NodeState::Warning);
        assert_eq!(evaluator.worst_active_severity(), Some(NodeState::Warning));

        // Still filling: the critical rule takes over and outranks the warning.
        let critical = with_disk(96, 100, "/");
        for _ in 0..3 {
            let _ = evaluator.evaluate(&critical, &rules);
        }
        assert_eq!(evaluator.worst_active_severity(), Some(NodeState::Critical));
        assert_eq!(
            evaluator.active_details().len(),
            2,
            "both breached rules stay visible"
        );
        assert!(
            evaluator.active_details()[0].contains("critical"),
            "the most severe line leads: {:?}",
            evaluator.active_details()
        );

        // Space is freed. Both rules recover rather than latching.
        let freed = with_disk(10, 100, "/");
        for _ in 0..3 {
            let _ = evaluator.evaluate(&freed, &rules);
        }
        assert_eq!(evaluator.worst_active_severity(), None);
        assert!(evaluator.active_details().is_empty());
    }

    #[test]
    fn a_breach_detail_names_the_filesystem_value_and_threshold() {
        // The notification body is built from this string. "Disk is critical"
        // does not tell an operator which mount to clear.
        let rule = rule("disk", "disk_percent", Comparison::GreaterThanOrEqual, 90.0);
        let mut sample_with_disks = sample(0.0);
        sample_with_disks.disks = vec![
            DiskMetric {
                mount: "/".into(),
                total_bytes: 100,
                used_bytes: 20,
            },
            DiskMetric {
                mount: "/data".into(),
                total_bytes: 100,
                used_bytes: 93,
            },
        ];

        let raised = AlertEvaluator::default()
            .evaluate(&sample_with_disks, std::slice::from_ref(&rule))
            .remove(0);

        assert!(raised.detail.contains("/data"), "{}", raised.detail);
        assert!(raised.detail.contains("93%"), "{}", raised.detail);
        assert!(raised.detail.contains("90%"), "{}", raised.detail);
        assert!(raised.detail.contains("warning"), "{}", raised.detail);
    }

    #[test]
    fn a_reported_value_is_not_rounded_into_the_next_threshold() {
        // A 94.6 % filesystem printed as "95%" beside a 95 % critical rule that
        // has not fired reads as a broken alert.
        let rule = rule("disk", "disk_percent", Comparison::GreaterThanOrEqual, 90.0);
        let mut nearly_critical = sample(0.0);
        nearly_critical.disks = vec![DiskMetric {
            mount: "/".into(),
            total_bytes: 1_000,
            used_bytes: 946,
        }];

        let raised = AlertEvaluator::default()
            .evaluate(&nearly_critical, std::slice::from_ref(&rule))
            .remove(0);

        assert!(raised.detail.contains("94.6%"), "{}", raised.detail);
        // A whole-number threshold still reads as one.
        assert!(raised.detail.contains("of 90%"), "{}", raised.detail);
    }

    #[test]
    fn a_recovery_signal_states_the_threshold_that_was_left() {
        let rule = rule("cpu", "cpu_percent", Comparison::GreaterThanOrEqual, 80.0);
        let mut evaluator = AlertEvaluator::default();

        let _ = evaluator.evaluate(&sample(90.0), std::slice::from_ref(&rule));
        let recovered = evaluator
            .evaluate(&sample(10.0), std::slice::from_ref(&rule))
            .remove(0);

        assert!(!recovered.active);
        assert!(recovered.detail.contains("CPU"), "{}", recovered.detail);
        assert!(recovered.detail.contains("80%"), "{}", recovered.detail);
    }

    #[test]
    fn a_temperature_detail_is_reported_in_degrees_and_names_the_sensor() {
        // Percent-suffixing a temperature would misreport the hardware.
        let hot = rule(
            "temp",
            "temperature_celsius",
            Comparison::GreaterThanOrEqual,
            85.0,
        );
        let mut with_sensors = sample(0.0);
        with_sensors.temperature = Some(TemperatureMetric {
            label: "CPU die".into(),
            celsius: 92.0,
            critical_celsius: Some(100.0),
            sensor_count: 2,
        });

        let raised = AlertEvaluator::default()
            .evaluate(&with_sensors, std::slice::from_ref(&hot))
            .remove(0);

        assert!(raised.detail.contains("CPU die"), "{}", raised.detail);
        assert!(raised.detail.contains("92 °C"), "{}", raised.detail);
        assert!(!raised.detail.contains('%'), "{}", raised.detail);
    }

    #[test]
    fn a_removed_rule_stops_holding_the_machine_in_its_severity() {
        // Rules are reloaded from operator configuration. A dropped rule whose
        // severity survived would keep a machine alerting on a threshold that
        // no longer exists, with no way to clear it.
        let rule = rule("cpu", "cpu_percent", Comparison::GreaterThanOrEqual, 80.0);
        let mut evaluator = AlertEvaluator::default();

        let _ = evaluator.evaluate(&sample(90.0), std::slice::from_ref(&rule));
        assert_eq!(evaluator.worst_active_severity(), Some(NodeState::Warning));

        let _ = evaluator.evaluate(&sample(90.0), &[]);
        assert_eq!(evaluator.worst_active_severity(), None);
        assert!(evaluator.active_details().is_empty());
    }

    fn override_for(id: &str) -> super::AlertRuleConfig {
        super::AlertRuleConfig::new(id)
    }

    fn resolved(overrides: &[super::AlertRuleConfig], id: &str) -> super::ResolvedAlertRule {
        super::resolve_alert_rules(overrides)
            .unwrap_or_else(|error| panic!("{error}"))
            .into_iter()
            .find(|entry| entry.rule.id == id)
            .unwrap_or_else(|| panic!("{id} is missing from the resolved rules"))
    }

    #[test]
    fn changing_one_threshold_leaves_every_other_shipped_level_alone() {
        // The whole point of overrides: raising one level must not silently
        // drop the rules the operator never mentioned.
        let change = super::AlertRuleConfig {
            threshold: Some(80.0),
            ..override_for("disk-capacity-warning")
        };
        let all = super::resolve_alert_rules(std::slice::from_ref(&change))
            .unwrap_or_else(|error| panic!("{error}"));

        assert_eq!(all.len(), super::default_alert_rules().len());
        let changed = resolved(std::slice::from_ref(&change), "disk-capacity-warning");
        assert!((changed.rule.threshold - 80.0).abs() < f64::EPSILON);
        assert_eq!(changed.source, super::AlertRuleSource::Configured);
        // The untouched sibling keeps both its level and its built-in origin.
        let untouched = resolved(std::slice::from_ref(&change), "disk-capacity-critical");
        assert!((untouched.rule.threshold - 95.0).abs() < f64::EPSILON);
        assert_eq!(untouched.source, super::AlertRuleSource::BuiltIn);
    }

    #[test]
    fn a_disabled_rule_stays_visible_and_keeps_following_the_shipped_level() {
        // A rule that disappears when switched off cannot be switched back on,
        // and an operator who only silenced a rule should still inherit a
        // retuned default if they re-enable it after an upgrade.
        let change = super::AlertRuleConfig {
            enabled: Some(false),
            ..override_for("cpu-saturation-warning")
        };
        let entry = resolved(std::slice::from_ref(&change), "cpu-saturation-warning");

        assert!(!entry.enabled);
        assert_eq!(entry.source, super::AlertRuleSource::BuiltIn);
        assert!((entry.rule.threshold - 90.0).abs() < f64::EPSILON);
    }

    #[test]
    fn an_operator_can_add_a_rule_rackio_does_not_ship() {
        let addition = super::AlertRuleConfig {
            metric: Some(String::from("swap_percent")),
            comparison: Some(Comparison::GreaterThanOrEqual),
            threshold: Some(50.0),
            consecutive_samples: Some(30),
            severity: Some(NodeState::Warning),
            ..override_for("swap-warning")
        };
        let entry = resolved(std::slice::from_ref(&addition), "swap-warning");

        assert_eq!(entry.rule.metric, "swap_percent");
        assert!(entry.enabled);
        assert_eq!(entry.source, super::AlertRuleSource::Configured);
    }

    #[test]
    fn a_rule_that_could_never_fire_is_rejected_instead_of_accepted_silently() {
        // Each of these produces a rule that is quiet in exactly the same way a
        // healthy machine is, which is the one failure mode a monitor cannot
        // have. The operator must hear about it at configuration time.
        let cases = [
            (
                super::AlertRuleConfig {
                    metric: Some(String::from("gpu_percent")),
                    ..override_for("disk-capacity-warning")
                },
                "unknown metric",
            ),
            (
                super::AlertRuleConfig {
                    threshold: Some(f64::NAN),
                    ..override_for("disk-capacity-warning")
                },
                "not a finite number",
            ),
            (
                super::AlertRuleConfig {
                    severity: Some(NodeState::Offline),
                    ..override_for("disk-capacity-warning")
                },
                "not a level a threshold can reach",
            ),
            (
                super::AlertRuleConfig {
                    threshold: Some(70.0),
                    ..override_for("invented-rule")
                },
                "must define",
            ),
        ];

        for (change, expected) in cases {
            let Err(error) = super::resolve_alert_rules(std::slice::from_ref(&change)) else {
                panic!("this configuration must be rejected");
            };
            assert!(error.contains(expected), "{error}");
        }
    }

    #[test]
    fn swap_and_temperature_headroom_resolve_from_the_sample() {
        let mut with_swap = sample(0.0);
        with_swap.swap_used_bytes = Some(3);
        with_swap.swap_total_bytes = Some(4);
        with_swap.temperature = Some(TemperatureMetric {
            label: "CPU die".into(),
            celsius: 96.0,
            critical_celsius: Some(100.0),
            sensor_count: 1,
        });

        let swap = rule("swap", "swap_percent", Comparison::GreaterThanOrEqual, 75.0);
        assert!(
            AlertEvaluator::default().evaluate(&with_swap, std::slice::from_ref(&swap))[0].active,
            "3 of 4 swap bytes in use is 75 percent"
        );

        // Four degrees of headroom is below a five-degree rule.
        let headroom = rule(
            "headroom",
            "temperature_headroom_celsius",
            Comparison::LessThanOrEqual,
            5.0,
        );
        let raised = AlertEvaluator::default()
            .evaluate(&with_swap, std::slice::from_ref(&headroom))
            .remove(0);
        assert!(raised.active);
        assert!(raised.detail.contains("4 °C"), "{}", raised.detail);
        assert!(raised.detail.contains("CPU die"), "{}", raised.detail);

        // A machine with swap disabled, or a sensor with no published limit,
        // resolves neither metric rather than reading as zero.
        let mut without = sample(0.0);
        without.swap_used_bytes = Some(0);
        without.swap_total_bytes = Some(0);
        without.temperature = Some(TemperatureMetric {
            label: "CPU die".into(),
            celsius: 96.0,
            critical_celsius: None,
            sensor_count: 1,
        });
        for rule in [&swap, &headroom] {
            assert!(
                AlertEvaluator::default()
                    .evaluate(&without, std::slice::from_ref(rule))
                    .is_empty(),
                "{} must stay unresolved",
                rule.id
            );
        }
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
