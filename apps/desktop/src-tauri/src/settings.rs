//! Typed desktop commands for the daemon's local health-threshold settings.
//!
//! The renderer never sends a daemon command of its own: each Tauri command
//! here validates its input, builds the one IPC command it stands for, and
//! returns the daemon's own read-back. Validation that the daemon repeats
//! (rule ids, metrics) is left to the daemon so the two cannot disagree.

use serde::Deserialize;
use serde_json::{Value, json};

use crate::daemon;

/// The severities a threshold rule may raise. Other node states are
/// connectivity outcomes, not something an operator can configure.
const RULE_SEVERITIES: [&str; 2] = ["warning", "critical"];
const COMPARISONS: [&str; 2] = ["greater_than_or_equal", "less_than_or_equal"];

/// A partial edit of one rule. Absent fields keep the rule's current value.
#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct AlertRuleEdit {
    pub id: String,
    pub metric: Option<String>,
    pub comparison: Option<String>,
    pub threshold: Option<f64>,
    pub consecutive_samples: Option<u32>,
    pub severity: Option<String>,
}

fn require_id(id: &str) -> Result<(), String> {
    if id.trim().is_empty() {
        return Err(String::from("An alert rule id is required."));
    }
    Ok(())
}

pub(crate) fn alerts_set_command(edit: &AlertRuleEdit) -> Result<Value, String> {
    require_id(&edit.id)?;
    if let Some(threshold) = edit.threshold
        && !threshold.is_finite()
    {
        return Err(String::from("The threshold must be a finite number."));
    }
    if edit.consecutive_samples == Some(0) {
        return Err(String::from(
            "A rule needs at least one consecutive sample.",
        ));
    }
    if let Some(severity) = &edit.severity
        && !RULE_SEVERITIES.contains(&severity.as_str())
    {
        return Err(String::from("The severity must be warning or critical."));
    }
    if let Some(comparison) = &edit.comparison
        && !COMPARISONS.contains(&comparison.as_str())
    {
        return Err(String::from("Unknown comparison."));
    }
    let mut command = json!({ "command": "alerts", "alerts": "set", "id": edit.id });
    let fields = [
        ("metric", edit.metric.as_ref().map(|value| json!(value))),
        (
            "comparison",
            edit.comparison.as_ref().map(|value| json!(value)),
        ),
        ("threshold", edit.threshold.map(|value| json!(value))),
        (
            "consecutive_samples",
            edit.consecutive_samples.map(|value| json!(value)),
        ),
        ("severity", edit.severity.as_ref().map(|value| json!(value))),
    ];
    for (key, value) in fields {
        if let Some(value) = value {
            command[key] = value;
        }
    }
    Ok(command)
}

pub(crate) fn alerts_rule_enabled_command(id: &str, enabled: bool) -> Result<Value, String> {
    require_id(id)?;
    Ok(json!({ "command": "alerts", "alerts": "rule_enabled", "id": id, "enabled": enabled }))
}

pub(crate) fn alerts_reset_command(id: Option<&str>) -> Result<Value, String> {
    let mut command = json!({ "command": "alerts", "alerts": "reset" });
    if let Some(id) = id {
        require_id(id)?;
        command["id"] = json!(id);
    }
    Ok(command)
}

/// The daemon's read-back of its rules, or an error: a response without data
/// is never reported as an empty rule set.
fn alert_listing(response: &Value) -> Result<Value, String> {
    let data = response
        .get("data")
        .filter(|data| data.get("rules").is_some_and(Value::is_array))
        .ok_or_else(|| String::from("daemon alert response did not contain rules"))?;
    Ok(data.clone())
}

async fn exchange(command: Value) -> Result<Value, String> {
    let response = daemon::request(command)
        .await
        .map_err(|error| error.to_string())?;
    alert_listing(&response)
}

#[tauri::command]
pub(crate) async fn alerts_list() -> Result<Value, String> {
    exchange(json!({ "command": "alerts", "alerts": "list" })).await
}

#[tauri::command]
pub(crate) async fn alerts_set(edit: AlertRuleEdit) -> Result<Value, String> {
    exchange(alerts_set_command(&edit)?).await
}

#[tauri::command]
pub(crate) async fn alerts_rule_enabled(id: String, enabled: bool) -> Result<Value, String> {
    exchange(alerts_rule_enabled_command(&id, enabled)?).await
}

#[tauri::command]
pub(crate) async fn alerts_reset(id: Option<String>) -> Result<Value, String> {
    exchange(alerts_reset_command(id.as_deref())?).await
}

#[tauri::command]
pub(crate) async fn alerts_set_enabled(enabled: bool) -> Result<Value, String> {
    exchange(json!({ "command": "alerts", "alerts": "enabled", "enabled": enabled })).await
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn edit(id: &str) -> AlertRuleEdit {
        AlertRuleEdit {
            id: id.to_owned(),
            ..AlertRuleEdit::default()
        }
    }

    #[test]
    fn an_edit_sends_only_the_fields_it_changes() {
        let command = alerts_set_command(&AlertRuleEdit {
            threshold: Some(90.0),
            ..edit("cpu_warning")
        })
        .unwrap_or_else(|error| panic!("{error}"));
        assert_eq!(
            command,
            json!({ "command": "alerts", "alerts": "set", "id": "cpu_warning", "threshold": 90.0 })
        );
    }

    #[test]
    fn invalid_edits_are_refused_before_reaching_the_daemon() {
        for bad in [
            edit(" "),
            AlertRuleEdit {
                threshold: Some(f64::NAN),
                ..edit("a")
            },
            AlertRuleEdit {
                threshold: Some(f64::INFINITY),
                ..edit("a")
            },
            AlertRuleEdit {
                consecutive_samples: Some(0),
                ..edit("a")
            },
            AlertRuleEdit {
                severity: Some("offline".into()),
                ..edit("a")
            },
            AlertRuleEdit {
                comparison: Some("equal".into()),
                ..edit("a")
            },
        ] {
            assert!(alerts_set_command(&bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn reset_targets_one_rule_or_all_of_them() {
        assert_eq!(
            alerts_reset_command(None).unwrap_or_default(),
            json!({ "command": "alerts", "alerts": "reset" })
        );
        assert_eq!(
            alerts_reset_command(Some("disk")).unwrap_or_default(),
            json!({ "command": "alerts", "alerts": "reset", "id": "disk" })
        );
        assert!(alerts_reset_command(Some("")).is_err());
    }

    #[test]
    fn a_response_without_rules_is_an_error_not_an_empty_list() {
        assert!(alert_listing(&json!({ "ok": true })).is_err());
        assert!(alert_listing(&json!({ "ok": true, "data": {} })).is_err());
        let listing = json!({ "alerts_enabled": true, "rules": [] });
        assert_eq!(
            alert_listing(&json!({ "ok": true, "data": listing })).ok(),
            Some(listing)
        );
    }

    #[test]
    fn unknown_edit_fields_are_rejected() {
        let parsed = serde_json::from_value::<AlertRuleEdit>(json!({ "id": "a", "shell": "x" }));
        assert!(parsed.is_err());
    }
}
