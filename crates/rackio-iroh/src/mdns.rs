use std::{collections::BTreeMap, net::IpAddr};

use iroh::Endpoint;
use swarm_discovery::{Discoverer, DropGuard};
use thiserror::Error;
use tokio::sync::Mutex;

const PAIRING_SERVICE_NAME: &str = "rackio-pairing-v1";

#[derive(Debug, Error)]
pub enum PairingMdnsError {
    #[error("the endpoint has no direct address to advertise")]
    NoDirectAddress,
    #[error("mDNS advertisement could not start: {0}")]
    Start(String),
}

/// A short-lived LAN advertisement. The one-time pairing secret is never
/// included: mDNS only publishes the endpoint ID and reachable addresses.
pub struct PairingMdnsAdvertisement {
    _guard: DropGuard,
}

impl std::fmt::Debug for PairingMdnsAdvertisement {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("PairingMdnsAdvertisement")
            .finish_non_exhaustive()
    }
}

impl PairingMdnsAdvertisement {
    pub fn start(endpoint: &Endpoint) -> Result<Self, PairingMdnsError> {
        let mut addresses_by_port = BTreeMap::<u16, Vec<IpAddr>>::new();
        for address in endpoint.addr().ip_addrs() {
            addresses_by_port
                .entry(address.port())
                .or_default()
                .push(address.ip());
        }
        if addresses_by_port.is_empty() {
            return Err(PairingMdnsError::NoDirectAddress);
        }

        // Use swarm-discovery directly so the LAN-only pairing feature cannot
        // re-enable iroh's vendor relay defaults through a transitive feature.
        let mut discoverer = Discoverer::new_interactive(
            String::from(PAIRING_SERVICE_NAME),
            endpoint.id().to_string(),
        );
        for (port, addresses) in addresses_by_port {
            discoverer = discoverer.with_addrs(port, addresses);
        }
        let guard = discoverer
            .spawn(&tokio::runtime::Handle::current())
            .map_err(|error| PairingMdnsError::Start(error.to_string()))?;
        Ok(Self { _guard: guard })
    }
}

#[derive(Debug, Default)]
pub struct PairingMdnsState {
    state: Mutex<PairingMdnsInner>,
}

#[derive(Debug, Default)]
struct PairingMdnsInner {
    generation: u64,
    active: Option<PairingMdnsAdvertisement>,
}

impl PairingMdnsState {
    pub async fn open(&self, endpoint: &Endpoint) -> Result<u64, PairingMdnsError> {
        let mut state = self.state.lock().await;
        state.generation = state.generation.saturating_add(1);
        let generation = state.generation;
        // Drop first so the old lease cannot clear the newly registered
        // service after replacement.
        state.active.take();
        state.active = Some(PairingMdnsAdvertisement::start(endpoint)?);
        Ok(generation)
    }

    pub async fn close(&self) {
        let mut state = self.state.lock().await;
        state.generation = state.generation.saturating_add(1);
        state.active.take();
    }

    pub async fn close_if_generation(&self, generation: u64) -> bool {
        let mut state = self.state.lock().await;
        if state.generation != generation {
            return false;
        }
        state.generation = state.generation.saturating_add(1);
        state.active.take();
        true
    }
}

#[cfg(test)]
mod tests {
    use super::PairingMdnsState;

    #[tokio::test]
    async fn stale_expiry_cannot_close_a_newer_pairing_window() {
        let state = PairingMdnsState::default();
        {
            let mut inner = state.state.lock().await;
            inner.generation = 2;
        }

        assert!(!state.close_if_generation(1).await);
        assert!(state.close_if_generation(2).await);
    }
}
