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
        //
        // The endpoint ID becomes a DNS label. Its hexadecimal form is 64
        // characters, one past the 63-octet label limit, so every
        // advertisement was refused before it reached the network. z-base-32
        // is iroh's own DNS-safe encoding of the same key and round-trips
        // through `EndpointId::from_z32`.
        let mut discoverer =
            Discoverer::new_interactive(String::from(PAIRING_SERVICE_NAME), endpoint.id().to_z32());
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
    use iroh::SecretKey;

    use super::{PairingMdnsAdvertisement, PairingMdnsState};
    use crate::transport::{EndpointConfig, bind_endpoint};

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

    #[tokio::test]
    async fn closing_the_window_retires_the_generation_that_was_open() {
        // `close` has to move the generation on, not only drop the lease: the
        // expiry timer for the window just closed must not be able to close the
        // next one the operator opens.
        let state = PairingMdnsState::default();
        {
            let mut inner = state.state.lock().await;
            inner.generation = 7;
        }

        state.close().await;

        assert!(
            !state.close_if_generation(7).await,
            "the closed generation must no longer be current"
        );
    }

    #[tokio::test]
    async fn each_opened_window_gets_its_own_generation() {
        let endpoint = bind_endpoint(SecretKey::generate(), &EndpointConfig::default())
            .await
            .unwrap_or_else(|error| panic!("{error}"));

        let first = state_open(&endpoint).await;
        let (state, first) = first;
        let second = state
            .open(&endpoint)
            .await
            .unwrap_or_else(|error| panic!("{error}"));

        assert_ne!(
            first, second,
            "reopening must not reuse the generation of a replaced window"
        );
        assert!(
            !state.close_if_generation(first).await,
            "the replaced window's expiry must not close the current one"
        );
        assert!(
            state.close_if_generation(second).await,
            "the current window closes on its own generation"
        );

        endpoint.close().await;
    }

    async fn state_open(endpoint: &iroh::Endpoint) -> (PairingMdnsState, u64) {
        let state = PairingMdnsState::default();
        let generation = state
            .open(endpoint)
            .await
            .unwrap_or_else(|error| panic!("{error}"));
        (state, generation)
    }

    #[tokio::test]
    async fn the_advertised_endpoint_id_fits_a_dns_label_and_round_trips() {
        // The published instance name is a DNS label, so a 64-character
        // hexadecimal endpoint ID is one octet too long and the advertisement
        // never starts. It also has to be decodable, or a peer that finds the
        // service cannot tell which machine it found.
        let endpoint = bind_endpoint(SecretKey::generate(), &EndpointConfig::default())
            .await
            .unwrap_or_else(|error| panic!("{error}"));
        let published = endpoint.id().to_z32();

        assert!(
            published.len() <= 63,
            "a DNS label holds at most 63 octets, got {}",
            published.len()
        );
        assert_eq!(
            iroh::EndpointId::from_z32(&published).unwrap_or_else(|error| panic!("{error}")),
            endpoint.id(),
            "the advertised name must identify the endpoint it came from"
        );

        endpoint.close().await;
    }

    #[tokio::test]
    async fn the_advertisement_debug_output_names_the_type_without_its_lease() {
        let endpoint = bind_endpoint(SecretKey::generate(), &EndpointConfig::default())
            .await
            .unwrap_or_else(|error| panic!("{error}"));

        let advertisement =
            PairingMdnsAdvertisement::start(&endpoint).unwrap_or_else(|error| panic!("{error}"));

        let rendered = format!("{advertisement:?}");
        assert!(
            rendered.starts_with("PairingMdnsAdvertisement"),
            "an advertisement must still identify itself in logs: {rendered}"
        );
        assert!(rendered.contains(".."), "its lease stays out of the output");

        drop(advertisement);
        endpoint.close().await;
    }
}
