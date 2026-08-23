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

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub(super) struct AgentConfig {
    pub(super) relay_url: Option<String>,
    /// Operator-defined local health thresholds. Empty by default: Rackio does
    /// not invent thresholds for a machine it knows nothing about, and
    /// `docs/operations.md` documents `warning`/`critical` as "a *configured*
    /// local health threshold was crossed".
    #[serde(default)]
    pub(super) alerts: Vec<rackio_core::AlertRule>,
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
    use rackio_core::{AlertRule, Comparison, NodeState};

    use std::net::SocketAddr;

    use super::{
        AgentConfig, AppPaths, MAX_ADVERTISE_ADDRESSES, add_advertise_address, load_config,
        parse_advertise_address, remove_advertise_address, save_config, validate_bind_port,
        validate_relay_url,
    };

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

    #[test]
    fn relay_url_validation_fails_closed() {
        assert!(validate_relay_url(Some("not a relay URL")).is_err());
        assert!(validate_relay_url(Some("https://relay.example.test")).is_ok());
        assert!(validate_relay_url(None).is_ok());
    }

    #[test]
    fn a_missing_config_is_direct_only_without_invented_alerts() {
        let directory = tempfile::tempdir().unwrap_or_else(|error| panic!("{error}"));
        let config =
            load_config(&test_paths(directory.path())).unwrap_or_else(|error| panic!("{error}"));

        assert!(config.relay_url.is_none());
        assert!(config.alerts.is_empty());
        assert!(config.bind_port.is_none());
        assert!(config.advertise_addresses.is_empty());
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
    fn config_round_trips_every_operator_owned_field() {
        let directory = tempfile::tempdir().unwrap_or_else(|error| panic!("{error}"));
        let paths = test_paths(directory.path());
        let expected = AgentConfig {
            relay_url: Some(String::from("https://relay.example.test")),
            alerts: vec![AlertRule {
                id: String::from("cpu-warning"),
                metric: String::from("cpu_percent"),
                comparison: Comparison::GreaterThanOrEqual,
                threshold: 80.0,
                consecutive_samples: 3,
                severity: NodeState::Warning,
            }],
            bind_port: Some(7777),
            advertise_addresses: vec![address("198.51.100.7:7777"), address("[2001:db8::1]:7777")],
        };

        save_config(&paths, &expected).unwrap_or_else(|error| panic!("{error}"));
        let actual = load_config(&paths).unwrap_or_else(|error| panic!("{error}"));

        assert_eq!(actual.relay_url, expected.relay_url);
        assert_eq!(actual.alerts, expected.alerts);
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
    fn an_invalid_config_fails_closed_instead_of_using_defaults() {
        let directory = tempfile::tempdir().unwrap_or_else(|error| panic!("{error}"));
        let paths = test_paths(directory.path());
        std::fs::create_dir_all(&paths.config).unwrap_or_else(|error| panic!("{error}"));
        std::fs::write(paths.config.join("config.json"), b"not json")
            .unwrap_or_else(|error| panic!("{error}"));

        assert!(load_config(&paths).is_err());
    }
}
