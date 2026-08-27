//! Persistence for the paired-machine registry: which machines this viewer is
//! paired with, where they were last reachable, and the last snapshot each one
//! was seen with.

use std::{
    collections::BTreeMap,
    fs,
    io::Write,
    net::SocketAddr,
    path::{Path, PathBuf},
    sync::{Arc, RwLock},
};

use rackio_core::{ConnectionPath, MetricSample, NodeInfo, NodeState, TrendWindow};
use rackio_iroh::{PairingBundle, PairingError};
use serde::{Deserialize, Serialize};

use super::{RemoteFleetError, snapshot::RemoteMachineSnapshot};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub(super) struct RemoteMachineRecord {
    pub(super) node: NodeInfo,
    pub(super) endpoint_id: String,
    pub(super) direct_addresses: Vec<SocketAddr>,
    pub(super) relay_urls: Vec<String>,
    pub(super) paired_at_ms: i64,
    #[serde(default)]
    pub(super) last_snapshot: Option<PersistedRemoteSnapshot>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub(super) struct PersistedRemoteSnapshot {
    pub(super) latest: Option<MetricSample>,
    pub(super) state: NodeState,
    pub(super) path: ConnectionPath,
    pub(super) rtt_ms: Option<u64>,
    pub(super) last_seen_ms: Option<i64>,
    /// Defaulted so registries written before the trend window existed still
    /// deserialise; their pre-trend `history` array is ignored and the window
    /// refills from the live stream.
    #[serde(default)]
    pub(super) trend: TrendWindow,
    pub(super) details: Vec<String>,
}

impl RemoteMachineRecord {
    pub(super) fn endpoint_addr(&self) -> Result<iroh::EndpointAddr, PairingError> {
        PairingBundle {
            format_version: 1,
            node_id: self.node.node_id,
            endpoint_id: self.endpoint_id.clone(),
            direct_addresses: self.direct_addresses.clone(),
            relay_urls: self.relay_urls.clone(),
            one_time_secret: String::new(),
            expires_at_ms: i64::MAX,
        }
        .endpoint_addr()
    }
}

impl From<&RemoteMachineSnapshot> for PersistedRemoteSnapshot {
    fn from(snapshot: &RemoteMachineSnapshot) -> Self {
        Self {
            latest: snapshot.latest.clone(),
            state: snapshot.state,
            path: snapshot.path,
            rtt_ms: snapshot.rtt_ms,
            last_seen_ms: snapshot.last_seen_ms,
            trend: snapshot.trend.clone(),
            details: snapshot.details.clone(),
        }
    }
}

#[derive(Debug, Clone)]
pub(super) struct RemoteMachineRegistry {
    path: PathBuf,
    records: Arc<RwLock<BTreeMap<String, RemoteMachineRecord>>>,
}

impl RemoteMachineRegistry {
    pub(super) fn load(path: impl Into<PathBuf>) -> Result<Self, RemoteFleetError> {
        let path = path.into();
        let records = match fs::read(&path) {
            Ok(bytes) => serde_json::from_slice(&bytes)?,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => BTreeMap::new(),
            Err(error) => return Err(error.into()),
        };
        Ok(Self {
            path,
            records: Arc::new(RwLock::new(records)),
        })
    }

    pub(super) fn list(&self) -> Result<Vec<RemoteMachineRecord>, RemoteFleetError> {
        Ok(self
            .records
            .read()
            .map_err(|_| RemoteFleetError::RegistryUnavailable)?
            .values()
            .cloned()
            .collect())
    }

    pub(super) fn contains(&self, endpoint_id: &str) -> Result<bool, RemoteFleetError> {
        Ok(self
            .records
            .read()
            .map_err(|_| RemoteFleetError::RegistryUnavailable)?
            .contains_key(endpoint_id))
    }

    pub(super) fn get(&self, endpoint_id: &str) -> Result<RemoteMachineRecord, RemoteFleetError> {
        self.records
            .read()
            .map_err(|_| RemoteFleetError::RegistryUnavailable)?
            .get(endpoint_id)
            .cloned()
            .ok_or(RemoteFleetError::UnknownMachine)
    }

    pub(super) fn insert(&self, record: RemoteMachineRecord) -> Result<(), RemoteFleetError> {
        let mut records = self
            .records
            .write()
            .map_err(|_| RemoteFleetError::RegistryUnavailable)?;
        let mut next = records.clone();
        next.insert(record.endpoint_id.clone(), record);
        persist_records(&self.path, &next)?;
        *records = next;
        Ok(())
    }

    /// Replace a machine's candidate direct addresses.
    ///
    /// The caller has already authenticated the session those addresses were
    /// observed on, so this refreshes where an existing pairing is reached and
    /// never adds a machine or widens what it is allowed to do.
    pub(super) fn update_addresses(
        &self,
        endpoint_id: &str,
        direct_addresses: Vec<SocketAddr>,
    ) -> Result<(), RemoteFleetError> {
        let mut records = self
            .records
            .write()
            .map_err(|_| RemoteFleetError::RegistryUnavailable)?;
        let mut next = records.clone();
        let record = next
            .get_mut(endpoint_id)
            .ok_or(RemoteFleetError::UnknownMachine)?;
        record.direct_addresses = direct_addresses;
        persist_records(&self.path, &next)?;
        *records = next;
        Ok(())
    }

    pub(super) fn update_snapshot(
        &self,
        endpoint_id: &str,
        snapshot: &RemoteMachineSnapshot,
    ) -> Result<(), RemoteFleetError> {
        let mut records = self
            .records
            .write()
            .map_err(|_| RemoteFleetError::RegistryUnavailable)?;
        let mut next = records.clone();
        let record = next
            .get_mut(endpoint_id)
            .ok_or(RemoteFleetError::UnknownMachine)?;
        record.last_snapshot = Some(PersistedRemoteSnapshot::from(snapshot));
        persist_records(&self.path, &next)?;
        *records = next;
        Ok(())
    }
}

fn persist_records(
    path: &Path,
    records: &BTreeMap<String, RemoteMachineRecord>,
) -> Result<(), RemoteFleetError> {
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    fs::create_dir_all(parent)?;
    let mut file = tempfile::Builder::new()
        .prefix(".machines-")
        .tempfile_in(parent)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        file.as_file()
            .set_permissions(fs::Permissions::from_mode(0o600))?;
    }
    file.write_all(&serde_json::to_vec_pretty(records)?)?;
    file.as_file().sync_all()?;
    file.persist(path).map_err(|error| error.error)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use rackio_core::NodeState;

    use super::{RemoteMachineRegistry, RemoteMachineSnapshot};
    use crate::remote::{
        RemoteFleetError,
        test_support::{address, record},
    };

    #[test]
    fn persisted_machine_registry_never_contains_pairing_secret() {
        let directory = tempfile::tempdir().unwrap_or_else(|error| panic!("{error}"));
        let path = directory.path().join("machines.json");
        let registry = RemoteMachineRegistry::load(&path).unwrap_or_else(|error| panic!("{error}"));
        registry
            .insert(record())
            .unwrap_or_else(|error| panic!("{error}"));
        let saved = std::fs::read_to_string(path).unwrap_or_else(|error| panic!("{error}"));

        assert!(!saved.contains("one_time_secret"));
        assert!(!saved.contains("must-not-persist"));
    }

    #[test]
    fn persisted_last_snapshot_survives_restart_without_becoming_zero() {
        let directory = tempfile::tempdir().unwrap_or_else(|error| panic!("{error}"));
        let path = directory.path().join("machines.json");
        let registry = RemoteMachineRegistry::load(&path).unwrap_or_else(|error| panic!("{error}"));
        let record = record();
        registry
            .insert(record.clone())
            .unwrap_or_else(|error| panic!("{error}"));
        let mut snapshot = RemoteMachineSnapshot::offline(&record);
        snapshot.state = NodeState::Healthy;
        snapshot.last_seen_ms = Some(42);
        snapshot.latest = Some(rackio_core::MetricSample {
            timestamp_ms: 42,
            sequence: 5,
            cpu_percent: Some(37.5),
            memory_used_bytes: None,
            memory_total_bytes: None,
            swap_used_bytes: None,
            swap_total_bytes: None,
            disks: Vec::new(),
            network: None,
            temperature: None,
            uptime_seconds: 1,
            errors: Vec::new(),
        });
        registry
            .update_snapshot(&record.endpoint_id, &snapshot)
            .unwrap_or_else(|error| panic!("{error}"));

        let reloaded = RemoteMachineRegistry::load(path).unwrap_or_else(|error| panic!("{error}"));
        let restored = RemoteMachineSnapshot::offline(
            &reloaded
                .get(&record.endpoint_id)
                .unwrap_or_else(|error| panic!("{error}")),
        );
        assert_eq!(
            restored.latest.and_then(|sample| sample.cpu_percent),
            Some(37.5)
        );
        assert_eq!(restored.last_seen_ms, Some(42));
    }

    #[test]
    fn a_refreshed_address_survives_a_restart_without_losing_the_last_snapshot() {
        let directory = tempfile::tempdir().unwrap_or_else(|error| panic!("{error}"));
        let path = directory.path().join("machines.json");
        let registry = RemoteMachineRegistry::load(&path).unwrap_or_else(|error| panic!("{error}"));
        let record = record();
        registry
            .insert(record.clone())
            .unwrap_or_else(|error| panic!("{error}"));
        let mut snapshot = RemoteMachineSnapshot::offline(&record);
        snapshot.last_seen_ms = Some(42);
        registry
            .update_snapshot(&record.endpoint_id, &snapshot)
            .unwrap_or_else(|error| panic!("{error}"));

        registry
            .update_addresses(&record.endpoint_id, vec![address("127.0.0.1:49200")])
            .unwrap_or_else(|error| panic!("{error}"));

        let reloaded = RemoteMachineRegistry::load(&path).unwrap_or_else(|error| panic!("{error}"));
        let restored = reloaded
            .get(&record.endpoint_id)
            .unwrap_or_else(|error| panic!("{error}"));
        assert_eq!(restored.direct_addresses, vec![address("127.0.0.1:49200")]);
        assert_eq!(
            restored
                .last_snapshot
                .and_then(|persisted| persisted.last_seen_ms),
            Some(42),
            "refreshing an address must not discard the last known values"
        );
        assert_eq!(
            restored.endpoint_id, record.endpoint_id,
            "a refresh reaches one already paired machine, never another"
        );
    }

    #[test]
    fn refreshing_an_unknown_machine_cannot_add_it() {
        // The refresh path must not be a way to write a machine into the
        // registry that no pairing ever authorized.
        let directory = tempfile::tempdir().unwrap_or_else(|error| panic!("{error}"));
        let path = directory.path().join("machines.json");
        let registry = RemoteMachineRegistry::load(&path).unwrap_or_else(|error| panic!("{error}"));

        let error = registry
            .update_addresses("never-paired", vec![address("127.0.0.1:49200")])
            .err()
            .unwrap_or_else(|| panic!("an unpaired machine must not be created by a refresh"));

        assert!(matches!(error, RemoteFleetError::UnknownMachine));
        assert!(
            registry
                .list()
                .unwrap_or_else(|error| panic!("{error}"))
                .is_empty()
        );
    }
}
