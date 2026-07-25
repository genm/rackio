use std::{
    collections::BTreeMap,
    fs,
    io::{self, Write},
    net::SocketAddr,
    path::{Path, PathBuf},
    sync::{Arc, RwLock},
    time::Duration,
};

use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use chrono::Utc;
use iroh::{EndpointAddr, EndpointId, TransportAddr};
use rand::RngCore;
use serde::{Deserialize, Serialize};
use subtle::ConstantTimeEq;
use thiserror::Error;
use uuid::Uuid;
use zeroize::Zeroizing;

const PAIRING_SECRET_BYTES: usize = 32;
const PAIRING_WINDOW: Duration = Duration::from_secs(5 * 60);
const MAX_FAILURES: u8 = 5;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PairingBundle {
    pub format_version: u8,
    pub node_id: Uuid,
    pub endpoint_id: String,
    pub direct_addresses: Vec<SocketAddr>,
    pub relay_urls: Vec<String>,
    pub one_time_secret: String,
    pub expires_at_ms: i64,
}

impl PairingBundle {
    pub fn encode(&self) -> Result<String, PairingError> {
        let bytes = serde_json::to_vec(self)?;
        Ok(format!("rackio-pair:{}", URL_SAFE_NO_PAD.encode(bytes)))
    }

    pub fn decode(encoded: &str) -> Result<Self, PairingError> {
        let payload = encoded
            .strip_prefix("rackio-pair:")
            .ok_or(PairingError::InvalidBundle)?;
        let bytes = URL_SAFE_NO_PAD
            .decode(payload)
            .map_err(|_| PairingError::InvalidBundle)?;
        let bundle: Self = serde_json::from_slice(&bytes)?;
        if bundle.format_version != 1 {
            return Err(PairingError::InvalidBundle);
        }
        Ok(bundle)
    }

    pub fn endpoint_addr(&self) -> Result<EndpointAddr, PairingError> {
        let endpoint_id = self
            .endpoint_id
            .parse::<EndpointId>()
            .map_err(|_| PairingError::InvalidBundle)?;
        let mut addresses = self
            .direct_addresses
            .iter()
            .copied()
            .map(TransportAddr::Ip)
            .collect::<Vec<_>>();
        for relay_url in &self.relay_urls {
            addresses.push(TransportAddr::Relay(
                relay_url.parse().map_err(|_| PairingError::InvalidBundle)?,
            ));
        }
        Ok(EndpointAddr::from_parts(endpoint_id, addresses))
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub struct PeerPermissions {
    pub read_metrics: bool,
    pub read_history: bool,
}

impl Default for PeerPermissions {
    fn default() -> Self {
        Self {
            read_metrics: true,
            read_history: true,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PeerRecord {
    pub endpoint_id: String,
    pub paired_at_ms: i64,
    pub permissions: PeerPermissions,
}

#[derive(Debug, Error)]
pub enum PairingError {
    #[error("no pairing window is active")]
    WindowClosed,
    #[error("pairing window has expired")]
    Expired,
    #[error("pairing request was rejected")]
    Rejected,
    #[error("pairing secret is malformed")]
    InvalidSecret,
    #[error("pairing bundle is malformed")]
    InvalidBundle,
    #[error("peer registry lock is unavailable")]
    RegistryUnavailable,
    #[error("peer registry I/O failed: {0}")]
    Io(#[from] io::Error),
    #[error("peer registry is invalid: {0}")]
    Json(#[from] serde_json::Error),
}

#[derive(Debug)]
struct PairingWindow {
    secret: Zeroizing<[u8; PAIRING_SECRET_BYTES]>,
    expires_at_ms: i64,
    failures: u8,
}

#[derive(Debug, Default)]
pub struct PairingManager {
    active: Option<PairingWindow>,
}

impl PairingManager {
    pub fn open(
        &mut self,
        node_id: Uuid,
        endpoint_id: EndpointId,
        direct_addresses: Vec<SocketAddr>,
        relay_urls: Vec<String>,
    ) -> PairingBundle {
        let mut secret = Zeroizing::new([0_u8; PAIRING_SECRET_BYTES]);
        rand::rng().fill_bytes(secret.as_mut());
        let expires_at_ms =
            Utc::now().timestamp_millis() + i64::try_from(PAIRING_WINDOW.as_millis()).unwrap_or(0);
        let encoded_secret = URL_SAFE_NO_PAD.encode(secret.as_ref());
        self.active = Some(PairingWindow {
            secret,
            expires_at_ms,
            failures: 0,
        });
        PairingBundle {
            format_version: 1,
            node_id,
            endpoint_id: endpoint_id.to_string(),
            direct_addresses,
            relay_urls,
            one_time_secret: encoded_secret,
            expires_at_ms,
        }
    }

    pub fn verify_and_consume(&mut self, supplied: &str) -> Result<(), PairingError> {
        let window = self.active.as_mut().ok_or(PairingError::WindowClosed)?;
        if Utc::now().timestamp_millis() > window.expires_at_ms {
            self.active = None;
            return Err(PairingError::Expired);
        }
        let supplied = URL_SAFE_NO_PAD.decode(supplied).ok();
        let valid = supplied.as_ref().is_some_and(|supplied| {
            supplied.len() == PAIRING_SECRET_BYTES
                && bool::from(window.secret.as_ref().ct_eq(supplied.as_slice()))
        });
        if valid {
            self.active = None;
            return Ok(());
        }
        window.failures = window.failures.saturating_add(1);
        if window.failures >= MAX_FAILURES {
            self.active = None;
        }
        Err(PairingError::Rejected)
    }
}

#[derive(Debug, Clone)]
pub struct PeerRegistry {
    path: PathBuf,
    records: Arc<RwLock<BTreeMap<String, PeerRecord>>>,
}

impl PeerRegistry {
    pub fn load(path: impl Into<PathBuf>) -> Result<Self, PairingError> {
        let path = path.into();
        let records = match fs::read(&path) {
            Ok(bytes) => serde_json::from_slice(&bytes)?,
            Err(error) if error.kind() == io::ErrorKind::NotFound => BTreeMap::new(),
            Err(error) => return Err(error.into()),
        };
        Ok(Self {
            path,
            records: Arc::new(RwLock::new(records)),
        })
    }

    pub fn contains(&self, endpoint_id: EndpointId) -> Result<bool, PairingError> {
        Ok(self.permissions(endpoint_id)?.is_some())
    }

    pub fn permissions(
        &self,
        endpoint_id: EndpointId,
    ) -> Result<Option<PeerPermissions>, PairingError> {
        let records = self
            .records
            .read()
            .map_err(|_| PairingError::RegistryUnavailable)?;
        Ok(records
            .get(&endpoint_id.to_string())
            .map(|record| record.permissions))
    }

    pub fn list(&self) -> Result<Vec<PeerRecord>, PairingError> {
        let records = self
            .records
            .read()
            .map_err(|_| PairingError::RegistryUnavailable)?;
        Ok(records.values().cloned().collect())
    }

    pub fn authorize(
        &self,
        endpoint_id: EndpointId,
        permissions: PeerPermissions,
    ) -> Result<(), PairingError> {
        let mut records = self
            .records
            .write()
            .map_err(|_| PairingError::RegistryUnavailable)?;
        let mut next = records.clone();
        next.insert(
            endpoint_id.to_string(),
            PeerRecord {
                endpoint_id: endpoint_id.to_string(),
                paired_at_ms: Utc::now().timestamp_millis(),
                permissions,
            },
        );
        persist_records(&self.path, &next)?;
        *records = next;
        Ok(())
    }

    pub fn revoke(&self, endpoint_id: &str) -> Result<bool, PairingError> {
        let mut records = self
            .records
            .write()
            .map_err(|_| PairingError::RegistryUnavailable)?;
        let mut next = records.clone();
        let removed = next.remove(endpoint_id).is_some();
        if !removed {
            return Ok(false);
        }
        persist_records(&self.path, &next)?;
        *records = next;
        Ok(removed)
    }
}

fn persist_records(
    path: &Path,
    records: &BTreeMap<String, PeerRecord>,
) -> Result<(), PairingError> {
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    fs::create_dir_all(parent)?;
    let mut file = tempfile::Builder::new()
        .prefix(".peers-")
        .tempfile_in(parent)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
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
    use std::fs;

    use iroh::SecretKey;
    use uuid::Uuid;

    use super::{PairingBundle, PairingError, PairingManager, PeerPermissions, PeerRegistry};

    #[test]
    fn pairing_secret_is_single_use() {
        let key = SecretKey::generate();
        let mut manager = PairingManager::default();
        let bundle = manager.open(Uuid::new_v4(), key.public(), Vec::new(), Vec::new());

        assert!(manager.verify_and_consume(&bundle.one_time_secret).is_ok());
        assert!(matches!(
            manager.verify_and_consume(&bundle.one_time_secret),
            Err(PairingError::WindowClosed)
        ));
        assert_eq!(
            PairingBundle::decode(&bundle.encode().unwrap_or_else(|error| panic!("{error}")))
                .unwrap_or_else(|error| panic!("{error}")),
            bundle
        );
    }

    #[test]
    fn closes_window_after_five_failures() {
        let key = SecretKey::generate();
        let mut manager = PairingManager::default();
        manager.open(Uuid::new_v4(), key.public(), Vec::new(), Vec::new());
        for _ in 0..5 {
            assert!(manager.verify_and_consume("invalid").is_err());
        }
        assert!(matches!(
            manager.verify_and_consume("invalid"),
            Err(PairingError::WindowClosed)
        ));
    }

    #[test]
    fn registry_persists_authorization_and_revoke() {
        let directory = tempfile::tempdir().unwrap_or_else(|error| panic!("{error}"));
        let path = directory.path().join("peers.json");
        let endpoint_id = SecretKey::generate().public();
        let registry = PeerRegistry::load(&path).unwrap_or_else(|error| panic!("{error}"));
        registry
            .authorize(endpoint_id, PeerPermissions::default())
            .unwrap_or_else(|error| panic!("{error}"));

        let reloaded = PeerRegistry::load(&path).unwrap_or_else(|error| panic!("{error}"));
        assert!(reloaded.contains(endpoint_id).unwrap_or(false));
        assert!(reloaded.revoke(&endpoint_id.to_string()).unwrap_or(false));
        assert!(!reloaded.contains(endpoint_id).unwrap_or(true));
    }

    #[test]
    fn registry_does_not_authorize_in_memory_when_persistence_fails() {
        let directory = tempfile::tempdir().unwrap_or_else(|error| panic!("{error}"));
        let registry_directory = directory.path().join("registry");
        fs::create_dir(&registry_directory).unwrap_or_else(|error| panic!("{error}"));
        let registry = PeerRegistry::load(registry_directory.join("peers.json"))
            .unwrap_or_else(|error| panic!("{error}"));
        fs::remove_dir(&registry_directory).unwrap_or_else(|error| panic!("{error}"));
        fs::write(&registry_directory, b"not a directory")
            .unwrap_or_else(|error| panic!("{error}"));
        let endpoint_id = SecretKey::generate().public();

        assert!(
            registry
                .authorize(endpoint_id, PeerPermissions::default())
                .is_err()
        );
        assert!(!registry.contains(endpoint_id).unwrap_or(true));
    }
}
