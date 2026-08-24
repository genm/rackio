//! Operator-owned paths and persistent daemon configuration.

use std::{
    fs,
    io::Write as _,
    net::SocketAddr,
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
    /// The PEM file holding the certificate authority that signs the configured
    /// relay's TLS certificate.
    ///
    /// Unset — the default — means the relay must present a publicly trusted
    /// certificate, verified against the compiled-in public root set. Set, it
    /// *replaces* that root set for relay connections, which is how a relay on
    /// an internal network with an internal CA becomes reachable.
    ///
    /// The path is stored exactly as the operator wrote it and is only ever
    /// meaningful alongside `relay_url`: clearing the relay clears this too.
    #[serde(default)]
    pub(super) relay_ca_certificate: Option<PathBuf>,
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
    /// Addresses this machine is reachable on but cannot observe on any of its
    /// own interfaces — typically the `IP:PORT` a router forwards to it. They
    /// are carried in the pairing bundle as ordinary direct candidates.
    ///
    /// Rackio never probes, resolves or otherwise contacts them: this is an
    /// operator-known fact, exactly like the fixed listen port, and a wrong
    /// entry simply behaves as an unreachable candidate.
    #[serde(default)]
    pub(super) advertise_addresses: Vec<SocketAddr>,
}

/// How many advertised addresses one machine may configure.
///
/// A viewer keeps at most `MAX_RECORD_DIRECT_ADDRESSES` candidates per paired
/// machine, so a bundle that carried more would hand over addresses the viewer
/// drops. The same bound here keeps the configured set inside what a pairing
/// can actually deliver while still covering a machine forwarded on several
/// paths. Exceeding it is refused rather than truncated: silently dropping an
/// address the operator asked for would strand the viewers it was meant to
/// reach.
pub(super) const MAX_ADVERTISE_ADDRESSES: usize = 8;

/// Parse an operator-supplied advertised address, failing closed with the
/// correction to make. No lookup happens here — a hostname is rejected rather
/// than resolved, because resolving it would contact a name server that
/// direct-only mode promises not to.
pub(super) fn parse_advertise_address(value: &str) -> Result<SocketAddr, String> {
    let address = value.parse::<SocketAddr>().map_err(|_| {
        format!(
            "`{value}` is not an IP address with a port; write it as `198.51.100.7:41641`, \
             or `[2001:db8::1]:41641` for IPv6"
        )
    })?;
    // Port 0 is the OS "choose one for me" request. Advertising it would tell
    // viewers to connect to a port nothing listens on.
    if address.port() == 0 {
        return Err(String::from(
            "port 0 is ephemeral and cannot be advertised; use the UDP port forwarded to this machine",
        ));
    }
    Ok(address)
}

pub(super) fn add_advertise_address(
    addresses: &mut Vec<SocketAddr>,
    address: SocketAddr,
) -> Result<(), String> {
    if addresses.contains(&address) {
        return Err(format!("{address} is already advertised"));
    }
    if addresses.len() >= MAX_ADVERTISE_ADDRESSES {
        return Err(format!(
            "at most {MAX_ADVERTISE_ADDRESSES} advertised addresses are kept; \
             remove one with `rackio advertise-address remove <IP:PORT>` first"
        ));
    }
    addresses.push(address);
    Ok(())
}

pub(super) fn remove_advertise_address(
    addresses: &mut Vec<SocketAddr>,
    address: SocketAddr,
) -> Result<(), String> {
    let before = addresses.len();
    addresses.retain(|kept| *kept != address);
    if addresses.len() == before {
        // Reporting success for an address that was never configured would let
        // an operator believe a stale advertisement is gone when it is not.
        return Err(format!("{address} is not advertised"));
    }
    Ok(())
}

impl Default for AgentConfig {
    fn default() -> Self {
        Self {
            relay_url: None,
            relay_ca_certificate: None,
            alerts_enabled: true,
            alerts: Vec::new(),
            bind_port: None,
            advertise_addresses: Vec::new(),
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

/// Check an operator-supplied relay trust anchor before it is stored.
///
/// A CA without a relay is refused rather than kept: it would sit in the
/// configuration doing nothing while reading as though a relay were pinned.
/// A CA the daemon cannot turn into a trust anchor is refused too, so a relay
/// that could never be reached is never recorded as configured.
///
/// Nothing here is contacted. The file is read from this machine's own
/// filesystem and parsed; no name is resolved and no relay is dialled.
pub(super) fn validate_relay_ca_certificate(
    relay_url: Option<&str>,
    ca_certificate: Option<&Path>,
) -> Result<(), String> {
    let Some(path) = ca_certificate else {
        return Ok(());
    };
    if relay_url.is_none() {
        return Err(String::from(
            "a relay CA certificate only applies to a relay; set it together with the relay URL",
        ));
    }
    rackio_iroh::validate_relay_ca_certificate(path).map_err(|error| error.to_string())
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

    use std::net::SocketAddr;

    use super::{
        AgentConfig, AppPaths, MAX_ADVERTISE_ADDRESSES, PathBuf, add_advertise_address,
        load_config, parse_advertise_address, remove_advertise_address, save_config,
        validate_bind_port, validate_relay_ca_certificate, validate_relay_url,
    };

    const RELAY_URL: &str = "https://relay.example.test";

    /// Whether a PEM file really holds a usable certificate authority is
    /// `rackio-iroh`'s question, and its tests answer it against a real
    /// certificate. What is checked here is the configuration layer: that the
    /// anchor is tied to a relay, that a refusal names the file, and that a
    /// refused change leaves the operator's working relay alone.
    fn write_file(root: &std::path::Path, name: &str, contents: &str) -> PathBuf {
        let path = root.join(name);
        std::fs::write(&path, contents).unwrap_or_else(|error| panic!("{error}"));
        path
    }

    fn address(value: &str) -> SocketAddr {
        value.parse().unwrap_or_else(|error| panic!("{error}"))
    }

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
        assert!(
            config.relay_ca_certificate.is_none(),
            "the default trust anchor is the public root set, not a pinned file"
        );
        assert!(config.bind_port.is_none());
        // Unconfigured is not unmonitored: a machine nobody has tuned still
        // reports a filling disk, memory pressure and a saturated processor.
        assert!(config.alerts.is_empty());
        assert!(config.alerts_enabled);
        assert!(config.advertise_addresses.is_empty());
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
    fn an_advertised_address_must_be_a_usable_socket_address() {
        assert!(parse_advertise_address("198.51.100.7:41641").is_ok());
        assert!(parse_advertise_address("[2001:db8::1]:41641").is_ok());
        // A hostname is refused rather than resolved: direct-only mode does not
        // contact a name server, and the operator knows the address already.
        assert!(parse_advertise_address("gateway.example.test:41641").is_err());
        assert!(parse_advertise_address("198.51.100.7").is_err());
        assert!(parse_advertise_address("not an address").is_err());
        assert!(parse_advertise_address("").is_err());
        let Err(error) = parse_advertise_address("198.51.100.7:0") else {
            panic!("an ephemeral port must not be advertised");
        };
        assert!(error.contains("ephemeral"), "{error}");
    }

    #[test]
    fn advertised_addresses_are_unique_and_bounded() {
        let mut addresses = Vec::new();
        add_advertise_address(&mut addresses, address("198.51.100.7:41641"))
            .unwrap_or_else(|error| panic!("{error}"));

        let Err(error) = add_advertise_address(&mut addresses, address("198.51.100.7:41641"))
        else {
            panic!("the same address must not be advertised twice");
        };
        assert!(error.contains("already advertised"), "{error}");

        for index in 1..MAX_ADVERTISE_ADDRESSES {
            add_advertise_address(
                &mut addresses,
                address(&format!("198.51.100.7:{}", 41641 + index)),
            )
            .unwrap_or_else(|error| panic!("{error}"));
        }
        assert_eq!(addresses.len(), MAX_ADVERTISE_ADDRESSES);

        let Err(error) = add_advertise_address(&mut addresses, address("203.0.113.9:41641")) else {
            panic!("the configured set must be bounded");
        };
        assert!(error.contains("remove one"), "{error}");
        // Rejected, never silently truncated: the operator's existing
        // advertisements stay exactly as they were.
        assert_eq!(addresses.len(), MAX_ADVERTISE_ADDRESSES);
        assert!(!addresses.contains(&address("203.0.113.9:41641")));
    }

    #[test]
    fn removing_an_address_that_is_not_advertised_fails_closed() {
        let mut addresses = vec![address("198.51.100.7:41641")];

        let Err(error) = remove_advertise_address(&mut addresses, address("203.0.113.9:41641"))
        else {
            panic!("removing an unconfigured address must report that it was not configured");
        };
        assert!(error.contains("not advertised"), "{error}");
        assert_eq!(addresses, vec![address("198.51.100.7:41641")]);

        remove_advertise_address(&mut addresses, address("198.51.100.7:41641"))
            .unwrap_or_else(|error| panic!("{error}"));
        assert!(addresses.is_empty());
    }

    #[test]
    fn configuring_an_advertised_address_contacts_nothing() {
        // The address is an unroutable documentation address that no probe
        // could confirm. Accepting it immediately is the evidence that nothing
        // was contacted: a reachability check would block on the connect
        // timeout instead of returning within this bound.
        let directory = tempfile::tempdir().unwrap_or_else(|error| panic!("{error}"));
        let paths = test_paths(directory.path());
        let started = std::time::Instant::now();
        let parsed =
            parse_advertise_address("198.51.100.7:41641").unwrap_or_else(|error| panic!("{error}"));
        let mut config = load_config(&paths).unwrap_or_else(|error| panic!("{error}"));
        add_advertise_address(&mut config.advertise_addresses, parsed)
            .unwrap_or_else(|error| panic!("{error}"));
        save_config(&paths, &config).unwrap_or_else(|error| panic!("{error}"));
        let elapsed = started.elapsed();

        assert!(
            elapsed < std::time::Duration::from_secs(1),
            "configuring an advertised address took {elapsed:?}; it must not touch the network"
        );
        let reloaded = load_config(&paths).unwrap_or_else(|error| panic!("{error}"));
        assert_eq!(reloaded.advertise_addresses, vec![parsed]);
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
            advertise_addresses: vec![address("198.51.100.7:7777"), address("[2001:db8::1]:7777")],
            relay_ca_certificate: Some(PathBuf::from("/etc/rackio/relay-ca.pem")),
        };

        save_config(&paths, &expected).unwrap_or_else(|error| panic!("{error}"));
        let actual = load_config(&paths).unwrap_or_else(|error| panic!("{error}"));

        // Stored exactly as the operator wrote it: the daemon reads this file
        // at every start, so a path it silently rewrote would be a path the
        // operator could not correct from what they see.
        assert_eq!(actual.relay_ca_certificate, expected.relay_ca_certificate);
        assert_eq!(actual.relay_url, expected.relay_url);
        assert_eq!(actual.alerts, expected.alerts);
        assert_eq!(actual.alerts_enabled, expected.alerts_enabled);
        assert_eq!(actual.bind_port, expected.bind_port);
        // Order is stable across a restart so the bundle offers the candidates
        // in the order the operator configured them.
        assert_eq!(actual.advertise_addresses, expected.advertise_addresses);
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
    fn a_relay_without_a_pinned_ca_round_trips_as_public_trust() {
        // The common case has to stay the default on disk: no field, no pin,
        // and the public root set still verifying the relay.
        let directory = tempfile::tempdir().unwrap_or_else(|error| panic!("{error}"));
        let paths = test_paths(directory.path());
        let expected = AgentConfig {
            relay_url: Some(String::from(RELAY_URL)),
            ..AgentConfig::default()
        };

        save_config(&paths, &expected).unwrap_or_else(|error| panic!("{error}"));
        let actual = load_config(&paths).unwrap_or_else(|error| panic!("{error}"));

        assert_eq!(actual.relay_url, expected.relay_url);
        assert!(actual.relay_ca_certificate.is_none());
    }

    #[test]
    fn a_relay_ca_without_a_relay_is_refused() {
        let directory = tempfile::tempdir().unwrap_or_else(|error| panic!("{error}"));
        // Deliberately a file that exists and is a perfectly good PEM: the
        // reason for the refusal must be the missing relay, not the contents.
        let ca = write_file(directory.path(), "ca.pem", "-----BEGIN CERTIFICATE-----\n");

        let Err(error) = validate_relay_ca_certificate(None, Some(&ca)) else {
            panic!("a trust anchor for no relay anchors nothing");
        };
        assert!(error.contains("relay URL"), "{error}");

        // No relay and no anchor is the direct-only default, and a relay on the
        // public root set is the ordinary relay case. Neither is refused.
        validate_relay_ca_certificate(Some(RELAY_URL), None)
            .unwrap_or_else(|error| panic!("{error}"));
        validate_relay_ca_certificate(None, None).unwrap_or_else(|error| panic!("{error}"));
    }

    #[test]
    fn an_unusable_relay_ca_is_refused_by_name_and_reason() {
        let directory = tempfile::tempdir().unwrap_or_else(|error| panic!("{error}"));

        for (name, contents) in [
            ("garbage.pem", "this is not a certificate\n"),
            ("empty.pem", ""),
            (
                "key.pem",
                "-----BEGIN PRIVATE KEY-----\naGVsbG8gcmFja2lv\n-----END PRIVATE KEY-----\n",
            ),
            (
                "fake.pem",
                "-----BEGIN CERTIFICATE-----\naGVsbG8gcmFja2lv\n-----END CERTIFICATE-----\n",
            ),
        ] {
            let path = write_file(directory.path(), name, contents);
            let Err(error) = validate_relay_ca_certificate(Some(RELAY_URL), Some(&path)) else {
                panic!("{name} must not be accepted as a certificate authority");
            };
            assert!(
                error.contains(name),
                "the operator must be told which file failed: {error}"
            );
        }

        let missing = directory.path().join("absent.pem");
        let Err(error) = validate_relay_ca_certificate(Some(RELAY_URL), Some(&missing)) else {
            panic!("a CA file that does not exist must not be accepted");
        };
        assert!(error.contains("absent.pem"), "{error}");
    }

    #[test]
    fn a_refused_relay_ca_leaves_the_previous_configuration_in_place() {
        // The order matters as much as the refusal: the relay change is
        // validated before anything is loaded or written, so a bad CA cannot
        // take a working relay down with it.
        let directory = tempfile::tempdir().unwrap_or_else(|error| panic!("{error}"));
        let paths = test_paths(directory.path());
        let working = AgentConfig {
            relay_url: Some(String::from(RELAY_URL)),
            relay_ca_certificate: Some(PathBuf::from("/etc/rackio/relay-ca.pem")),
            ..AgentConfig::default()
        };
        save_config(&paths, &working).unwrap_or_else(|error| panic!("{error}"));

        let broken = write_file(directory.path(), "broken.pem", "not a certificate\n");
        let outcome =
            validate_relay_ca_certificate(Some("https://other-relay.example.test"), Some(&broken))
                .and_then(|()| {
                    let mut config = load_config(&paths).map_err(|error| error.to_string())?;
                    config.relay_url = Some(String::from("https://other-relay.example.test"));
                    config.relay_ca_certificate = Some(broken.clone());
                    save_config(&paths, &config).map_err(|error| error.to_string())
                });

        assert!(outcome.is_err(), "a broken CA must not be stored");
        let reloaded = load_config(&paths).unwrap_or_else(|error| panic!("{error}"));
        assert_eq!(reloaded.relay_url, working.relay_url);
        assert_eq!(
            reloaded.relay_ca_certificate, working.relay_ca_certificate,
            "the relay the operator had working must survive a rejected change"
        );
    }

    #[test]
    fn an_invalid_config_fails_closed_instead_of_using_defaults() {
        let directory = tempfile::tempdir().unwrap_or_else(|error| panic!("{error}"));
        let paths = test_paths(directory.path());
        write_config(&paths, "not json");

        assert!(load_config(&paths).is_err());
    }
}
