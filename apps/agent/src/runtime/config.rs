//! Operator-owned paths and persistent daemon configuration.

use std::{
    fs,
    io::Write as _,
    path::{Path, PathBuf},
};

use anyhow::{Context as _, anyhow};
use directories::ProjectDirs;
use serde::{Deserialize, Serialize};
use tracing_subscriber::{EnvFilter, layer::SubscriberExt as _, util::SubscriberInitExt as _};
use uuid::Uuid;

#[derive(Debug, Clone)]
pub struct AppPaths {
    pub config: PathBuf,
    pub data: PathBuf,
    pub state: PathBuf,
    pub log: PathBuf,
    #[cfg(unix)]
    pub local_socket: PathBuf,
}

pub fn app_paths() -> anyhow::Result<AppPaths> {
    let config_override = std::env::var_os("RACKIO_CONFIG_DIR").map(PathBuf::from);
    let data_override = std::env::var_os("RACKIO_DATA_DIR").map(PathBuf::from);
    let state_override = std::env::var_os("RACKIO_STATE_DIR").map(PathBuf::from);
    // Service accounts may intentionally provide every owned path while OS
    // user-profile directories are unavailable. Only require ProjectDirs for
    // values that actually need a platform default.
    let dirs = if config_override.is_none() || data_override.is_none() || state_override.is_none() {
        Some(
            ProjectDirs::from("dev", "rackio", "rackio")
                .ok_or_else(|| anyhow!("OS application directories are unavailable"))?,
        )
    } else {
        None
    };
    let config_dir = match config_override {
        Some(path) => path,
        None => dirs
            .as_ref()
            .ok_or_else(|| anyhow!("OS config directory is unavailable"))?
            .config_dir()
            .to_path_buf(),
    };
    let data_dir = match data_override {
        Some(path) => path,
        None => dirs
            .as_ref()
            .ok_or_else(|| anyhow!("OS data directory is unavailable"))?
            .data_local_dir()
            .to_path_buf(),
    };
    let state_dir = if let Some(path) = state_override {
        path
    } else {
        let dirs = dirs
            .as_ref()
            .ok_or_else(|| anyhow!("OS state directory is unavailable"))?;
        dirs.state_dir()
            .unwrap_or_else(|| dirs.data_local_dir())
            .to_path_buf()
    };
    #[cfg(unix)]
    let local_socket = std::env::var_os("RACKIO_SOCKET")
        .map_or_else(|| state_dir.join("agent.sock"), PathBuf::from);
    let log_dir =
        std::env::var_os("RACKIO_LOG_DIR").map_or_else(|| state_dir.join("logs"), PathBuf::from);
    Ok(AppPaths {
        config: config_dir,
        data: data_dir,
        state: state_dir,
        log: log_dir,
        #[cfg(unix)]
        local_socket,
    })
}

const fn enabled_by_default() -> bool {
    true
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(super) struct AgentConfig {
    pub(super) relay_url: Option<String>,
    /// Whether this machine evaluates local health thresholds at all.
    ///
    /// `false` is the operator saying "never raise a local alert on this
    /// machine"; it does not silence stale, offline or degraded, which are
    /// reported by the viewer from evidence rather than from a threshold.
    #[serde(default = "enabled_by_default")]
    pub(super) alerts_enabled: bool,
    /// Operator changes to the shipped health thresholds.
    ///
    /// Entries are merged over `rackio_core::default_alert_rules`, so an
    /// operator who retunes one level keeps every other shipped rule and still
    /// receives later releases' defaults for the rules they never touched. An
    /// entry naming a rule Rackio does not ship defines a new one and must
    /// describe it in full.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(super) alerts: Vec<rackio_core::AlertRuleConfig>,
    /// The fixed UDP port this machine listens on. Unset means an ephemeral
    /// port, which moves on every restart: viewers that hold only the previous
    /// direct addresses then cannot reach this machine again. Operators who
    /// monitor this machine over a direct path set it, and forward it if the
    /// machine is behind NAT.
    #[serde(default)]
    pub(super) bind_port: Option<u16>,
}

impl Default for AgentConfig {
    fn default() -> Self {
        Self {
            relay_url: None,
            alerts_enabled: true,
            alerts: Vec::new(),
            bind_port: None,
        }
    }
}

impl AgentConfig {
    /// Every effective rule, including the ones switched off, for the operator
    /// surfaces that have to show what exists before it can be changed.
    ///
    /// # Errors
    /// Returns the operator-facing reason a configured rule cannot be applied.
    pub(super) fn resolved_alert_rules(
        &self,
    ) -> Result<Vec<rackio_core::ResolvedAlertRule>, String> {
        rackio_core::resolve_alert_rules(&self.alerts)
    }

    /// The rules the sampler actually evaluates.
    ///
    /// # Errors
    /// Returns the operator-facing reason a configured rule cannot be applied.
    /// Reporting nothing would leave the machine silently unmonitored, which is
    /// indistinguishable from a healthy one.
    pub(super) fn alert_rules(&self) -> Result<Vec<rackio_core::AlertRule>, String> {
        if !self.alerts_enabled {
            return Ok(Vec::new());
        }
        Ok(self
            .resolved_alert_rules()?
            .into_iter()
            .filter(|entry| entry.enabled)
            .map(|entry| entry.rule)
            .collect())
    }
}

pub(super) fn create_directories(paths: &AppPaths) -> anyhow::Result<()> {
    for path in [&paths.config, &paths.data, &paths.state, &paths.log] {
        fs::create_dir_all(path)?;
    }
    Ok(())
}

fn config_path(paths: &AppPaths) -> PathBuf {
    paths.config.join("config.json")
}

pub(super) fn validate_relay_url(relay_url: Option<&str>) -> Result<(), &'static str> {
    if relay_url.is_some_and(|value| value.parse::<iroh::RelayUrl>().is_err()) {
        Err("relay URL is invalid")
    } else {
        Ok(())
    }
}

pub(super) fn validate_bind_port(bind_port: Option<u16>) -> Result<(), &'static str> {
    // Port 0 is iroh's ephemeral request. Storing it would record a promise of
    // a stable address that the next restart breaks, so reject it here rather
    // than letting the daemon fail to start on the following boot.
    if bind_port == Some(0) {
        Err("listen port 0 is ephemeral; choose a fixed port or clear the setting")
    } else {
        Ok(())
    }
}

pub(super) fn load_config(paths: &AppPaths) -> anyhow::Result<AgentConfig> {
    match fs::read(config_path(paths)) {
        Ok(bytes) => Ok(serde_json::from_slice(&bytes)?),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(AgentConfig::default()),
        Err(error) => Err(error.into()),
    }
}

pub(super) fn save_config(paths: &AppPaths, config: &AgentConfig) -> anyhow::Result<()> {
    fs::create_dir_all(&paths.config)?;
    let target = config_path(paths);
    let mut file = tempfile::Builder::new()
        .prefix(".config-")
        .tempfile_in(&paths.config)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        file.as_file()
            .set_permissions(fs::Permissions::from_mode(0o600))?;
    }
    file.write_all(&serde_json::to_vec_pretty(config)?)?;
    file.as_file().sync_all()?;
    file.persist(target).map_err(|error| error.error)?;
    Ok(())
}

pub(super) fn load_or_create_node_id(path: &Path) -> anyhow::Result<Uuid> {
    match fs::read_to_string(path) {
        Ok(value) => Ok(Uuid::parse_str(value.trim())?),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            let node_id = Uuid::new_v4();
            fs::write(path, node_id.to_string())?;
            Ok(node_id)
        }
        Err(error) => Err(error.into()),
    }
}

pub(super) fn init_logging(paths: &AppPaths) -> anyhow::Result<()> {
    fs::create_dir_all(&paths.log)?;
    let file = tracing_appender::rolling::daily(&paths.log, "agent.jsonl");
    tracing_subscriber::registry()
        .with(EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")))
        .with(tracing_subscriber::fmt::layer().json().with_writer(file))
        .try_init()
        .context("structured logging initialization failed")
}

#[cfg(test)]
mod tests {
    use rackio_core::{AlertRuleConfig, Comparison, NodeState};

    use super::{
        AgentConfig, AppPaths, load_config, save_config, validate_bind_port, validate_relay_url,
    };

    fn test_paths(root: &std::path::Path) -> AppPaths {
        AppPaths {
            config: root.join("config"),
            data: root.join("data"),
            state: root.join("state"),
            log: root.join("log"),
            #[cfg(unix)]
            local_socket: root.join("agent.sock"),
        }
    }

    fn write_config(paths: &AppPaths, body: &str) {
        std::fs::create_dir_all(&paths.config).unwrap_or_else(|error| panic!("{error}"));
        std::fs::write(paths.config.join("config.json"), body)
            .unwrap_or_else(|error| panic!("{error}"));
    }

    fn rules(config: &AgentConfig) -> Vec<rackio_core::AlertRule> {
        config
            .alert_rules()
            .unwrap_or_else(|error| panic!("{error}"))
    }

    #[test]
    fn relay_url_validation_fails_closed() {
        assert!(validate_relay_url(Some("not a relay URL")).is_err());
        assert!(validate_relay_url(Some("https://relay.example.test")).is_ok());
        assert!(validate_relay_url(None).is_ok());
    }

    #[test]
    fn a_missing_config_is_direct_only_with_every_shipped_threshold() {
        let directory = tempfile::tempdir().unwrap_or_else(|error| panic!("{error}"));
        let config =
            load_config(&test_paths(directory.path())).unwrap_or_else(|error| panic!("{error}"));

        assert!(config.relay_url.is_none());
        assert!(config.bind_port.is_none());
        // Unconfigured is not unmonitored: a machine nobody has tuned still
        // reports a filling disk, memory pressure and a saturated processor.
        assert!(config.alerts.is_empty());
        assert!(config.alerts_enabled);
        assert_eq!(rules(&config), rackio_core::default_alert_rules());
    }

    #[test]
    fn switching_alerting_off_leaves_the_sampler_no_rules_to_evaluate() {
        // The operator's global "never alert on this machine". The rules stay
        // in the file so turning it back on restores what they had.
        let directory = tempfile::tempdir().unwrap_or_else(|error| panic!("{error}"));
        let paths = test_paths(directory.path());
        write_config(&paths, r#"{"alerts_enabled": false}"#);

        let config = load_config(&paths).unwrap_or_else(|error| panic!("{error}"));

        assert!(rules(&config).is_empty());
        assert_eq!(
            config
                .resolved_alert_rules()
                .unwrap_or_else(|error| panic!("{error}"))
                .len(),
            rackio_core::default_alert_rules().len(),
            "the rules must stay visible so they can be switched back on"
        );
    }

    #[test]
    fn one_retuned_threshold_keeps_every_other_shipped_rule() {
        // The reason overrides are merged rather than replacing the list: an
        // operator who raises one level must not silently stop watching the
        // rest of the machine.
        let directory = tempfile::tempdir().unwrap_or_else(|error| panic!("{error}"));
        let paths = test_paths(directory.path());
        write_config(
            &paths,
            r#"{"alerts": [{"id": "disk-capacity-warning", "threshold": 80.0}]}"#,
        );

        let effective = rules(&load_config(&paths).unwrap_or_else(|error| panic!("{error}")));

        assert_eq!(effective.len(), rackio_core::default_alert_rules().len());
        let disk = effective
            .iter()
            .find(|rule| rule.id == "disk-capacity-warning")
            .unwrap_or_else(|| panic!("the retuned rule is missing"));
        assert!((disk.threshold - 80.0).abs() < f64::EPSILON);
        assert_eq!(
            disk.consecutive_samples, 3,
            "the sample window is untouched"
        );
    }

    #[test]
    fn a_disabled_rule_is_withheld_from_the_sampler_but_still_listed() {
        let directory = tempfile::tempdir().unwrap_or_else(|error| panic!("{error}"));
        let paths = test_paths(directory.path());
        write_config(
            &paths,
            r#"{"alerts": [{"id": "cpu-saturation-warning", "enabled": false}]}"#,
        );
        let config = load_config(&paths).unwrap_or_else(|error| panic!("{error}"));

        assert!(
            !rules(&config)
                .iter()
                .any(|rule| rule.id == "cpu-saturation-warning")
        );
        let listed = config
            .resolved_alert_rules()
            .unwrap_or_else(|error| panic!("{error}"));
        let cpu = listed
            .iter()
            .find(|entry| entry.rule.id == "cpu-saturation-warning")
            .unwrap_or_else(|| panic!("a disabled rule must stay listed"));
        assert!(!cpu.enabled);
    }

    #[test]
    fn a_rule_that_could_never_fire_is_reported_rather_than_run() {
        // Silently dropping it would leave the machine reporting healthy for a
        // threshold its operator believes is being watched.
        let directory = tempfile::tempdir().unwrap_or_else(|error| panic!("{error}"));
        let paths = test_paths(directory.path());
        write_config(
            &paths,
            r#"{"alerts": [{"id": "disk-capacity-warning", "metric": "gpu_percent"}]}"#,
        );

        let config = load_config(&paths).unwrap_or_else(|error| panic!("{error}"));

        let Err(error) = config.alert_rules() else {
            panic!("an unknown metric must not resolve");
        };
        assert!(error.contains("gpu_percent"), "{error}");
    }

    #[test]
    fn an_ephemeral_listen_port_is_not_accepted_as_a_stable_one() {
        assert!(validate_bind_port(Some(0)).is_err());
        assert!(validate_bind_port(Some(7777)).is_ok());
        assert!(validate_bind_port(None).is_ok());
    }

    #[test]
    fn saving_an_untouched_alert_list_does_not_freeze_the_defaults_into_the_file() {
        // Setting a relay must not write today's default thresholds into the
        // operator's configuration as if they had chosen them.
        let directory = tempfile::tempdir().unwrap_or_else(|error| panic!("{error}"));
        let paths = test_paths(directory.path());
        let config = AgentConfig {
            relay_url: Some(String::from("https://relay.example.test")),
            ..AgentConfig::default()
        };

        save_config(&paths, &config).unwrap_or_else(|error| panic!("{error}"));

        let written = std::fs::read_to_string(paths.config.join("config.json"))
            .unwrap_or_else(|error| panic!("{error}"));
        assert!(!written.contains("\"alerts\""), "{written}");
        assert!(
            load_config(&paths)
                .unwrap_or_else(|error| panic!("{error}"))
                .alerts
                .is_empty()
        );
    }

    #[test]
    fn config_round_trips_every_operator_owned_field() {
        let directory = tempfile::tempdir().unwrap_or_else(|error| panic!("{error}"));
        let paths = test_paths(directory.path());
        let expected = AgentConfig {
            relay_url: Some(String::from("https://relay.example.test")),
            alerts_enabled: false,
            alerts: vec![AlertRuleConfig {
                metric: Some(String::from("cpu_percent")),
                comparison: Some(Comparison::GreaterThanOrEqual),
                threshold: Some(80.0),
                consecutive_samples: Some(3),
                severity: Some(NodeState::Warning),
                enabled: Some(false),
                ..AlertRuleConfig::new("cpu-warning")
            }],
            bind_port: Some(7777),
        };

        save_config(&paths, &expected).unwrap_or_else(|error| panic!("{error}"));
        let actual = load_config(&paths).unwrap_or_else(|error| panic!("{error}"));

        assert_eq!(actual.relay_url, expected.relay_url);
        assert_eq!(actual.alerts, expected.alerts);
        assert_eq!(actual.alerts_enabled, expected.alerts_enabled);
        assert_eq!(actual.bind_port, expected.bind_port);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            let mode = std::fs::metadata(paths.config.join("config.json"))
                .unwrap_or_else(|error| panic!("{error}"))
                .permissions()
                .mode();
            assert_eq!(mode & 0o777, 0o600);
        }
    }

    #[test]
    fn an_invalid_config_fails_closed_instead_of_using_defaults() {
        let directory = tempfile::tempdir().unwrap_or_else(|error| panic!("{error}"));
        let paths = test_paths(directory.path());
        write_config(&paths, "not json");

        assert!(load_config(&paths).is_err());
    }
}
