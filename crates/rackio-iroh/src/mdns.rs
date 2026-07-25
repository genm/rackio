use iroh::Endpoint;
use iroh_mdns_address_lookup::MdnsAddressLookup;
use thiserror::Error;
use tokio::sync::Mutex;

const PAIRING_SERVICE_NAME: &str = "rackio-pairing-v1";

#[derive(Debug, Error)]
pub enum PairingMdnsError {
    #[error("endpoint address lookup is unavailable: {0}")]
    Endpoint(String),
    #[error("mDNS advertisement could not start: {0}")]
    Start(String),
}

/// A short-lived LAN advertisement. The one-time pairing secret is never
/// included: mDNS only publishes the endpoint ID and reachable addresses.
#[derive(Debug)]
pub struct PairingMdnsAdvertisement {
    endpoint: Endpoint,
}

impl PairingMdnsAdvertisement {
    pub fn start(endpoint: &Endpoint) -> Result<Self, PairingMdnsError> {
        let services = endpoint
            .address_lookup()
            .map_err(|error| PairingMdnsError::Endpoint(error.to_string()))?;
        let mdns = MdnsAddressLookup::builder()
            .service_name(PAIRING_SERVICE_NAME)
            .build(endpoint.id())
            .map_err(|error| PairingMdnsError::Start(error.to_string()))?;

        // Rackio's Minimal endpoint installs no other lookup services. Clearing
        // here makes reopening a pairing window replace, rather than stack,
        // the previous short-lived advertisement.
        services.clear();
        services.add(mdns);
        Ok(Self {
            endpoint: endpoint.clone(),
        })
    }
}

impl Drop for PairingMdnsAdvertisement {
    fn drop(&mut self) {
        if let Ok(services) = self.endpoint.address_lookup() {
            services.clear();
        }
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
