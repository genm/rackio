//! Monitoring of paired remote machines.
//!
//! `RemoteFleet` is the entry point; the submodules own one concern each:
//! [`registry`] persists which machines are paired and where they were last
//! reachable, [`snapshot`] holds the in-memory view and its freshness rule,
//! [`monitor`] runs the per-machine reconnect-and-watch loop, and [`client`]
//! speaks the read-only wire protocol and decodes it into domain types.

mod client;
mod monitor;
mod registry;
mod snapshot;
#[cfg(test)]
mod test_support;

use std::{
    collections::BTreeMap,
    path::PathBuf,
    sync::Arc,
    time::{Duration, Instant},
};

use chrono::Utc;
use rackio_core::{ConnectionPath, MetricSample, NodeState, TrendWindow};
use rackio_iroh::{ClientConnection, PairingBundle, PairingError, TransportError};
use rackio_protocol::{
    current_version,
    v1::{HistoryQuery, PairRequest, Request, history_query, request, response},
};
use serde::{Deserialize, Serialize};
use thiserror::Error;
use tokio::sync::RwLock as AsyncRwLock;

use client::{CONNECT_TIMEOUT, REQUEST_TIMEOUT};
use registry::{RemoteMachineRecord, RemoteMachineRegistry};
pub use snapshot::RemoteMachineSnapshot;

const HISTORY_RESPONSE_TIMEOUT: Duration = Duration::from_mins(1);
/// A peer cannot retain more history than the retention contract allows, so
/// anything beyond it is a malfunctioning or hostile peer rather than a range
/// this viewer asked for.
const MAX_REMOTE_HISTORY_SAMPLES: usize = rackio_core::MAX_QUERY_ROWS;
const MAX_HISTORY_RANGE_MS: i64 = 7 * 24 * 60 * 60 * 1_000;

#[derive(Debug, Error)]
pub enum RemoteFleetError {
    #[error("pairing failed: {0}")]
    Pairing(#[from] PairingError),
    #[error("remote transport failed: {0}")]
    Transport(#[from] TransportError),
    #[error("remote operation timed out: {0}")]
    Timeout(&'static str),
    #[error("paired machine identity does not match the pinned bundle")]
    IdentityMismatch,
    #[error("pairing response was not accepted")]
    PairingRejected,
    #[error("remote response did not contain the expected {0}")]
    UnexpectedResponse(&'static str),
    #[error("machine is already paired")]
    AlreadyPaired,
    #[error("paired machine was not found")]
    UnknownMachine,
    #[error("history range is invalid or exceeds seven days")]
    InvalidHistoryRange,
    #[error("paired machine returned more history than its retention can hold")]
    HistoryResponseTooLarge,
    #[error("paired machine registry lock is unavailable")]
    RegistryUnavailable,
    #[error("paired machine registry I/O failed: {0}")]
    Io(#[from] std::io::Error),
    #[error("paired machine registry is invalid: {0}")]
    Json(#[from] serde_json::Error),
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RemoteHistoryResolution {
    Raw,
    Minute,
}

#[derive(Clone)]
pub struct RemoteFleet {
    endpoint: iroh::Endpoint,
    registry: RemoteMachineRegistry,
    snapshots: Arc<AsyncRwLock<BTreeMap<String, RemoteMachineSnapshot>>>,
}

impl RemoteFleet {
    pub fn load(
        endpoint: iroh::Endpoint,
        registry_path: impl Into<PathBuf>,
    ) -> Result<Self, RemoteFleetError> {
        let registry = RemoteMachineRegistry::load(registry_path)?;
        let initial = registry
            .list()?
            .into_iter()
            .map(|record| {
                (
                    record.endpoint_id.clone(),
                    RemoteMachineSnapshot::offline(&record),
                )
            })
            .collect();
        Ok(Self {
            endpoint,
            registry,
            snapshots: Arc::new(AsyncRwLock::new(initial)),
        })
    }

    pub fn start(&self) -> Result<(), RemoteFleetError> {
        for record in self.registry.list()? {
            self.spawn_monitor(record);
        }
        Ok(())
    }

    pub async fn snapshots(&self) -> Vec<RemoteMachineSnapshot> {
        let now_ms = Utc::now().timestamp_millis();
        self.snapshots
            .read()
            .await
            .values()
            .cloned()
            .map(|mut snapshot| {
                snapshot.state = snapshot.state_at(now_ms);
                snapshot
            })
            .collect()
    }

    pub async fn query_history(
        &self,
        endpoint_id: &str,
        from_ms: i64,
        to_ms: i64,
        resolution: RemoteHistoryResolution,
    ) -> Result<Vec<MetricSample>, RemoteFleetError> {
        if from_ms > to_ms || to_ms.saturating_sub(from_ms) > MAX_HISTORY_RANGE_MS {
            return Err(RemoteFleetError::InvalidHistoryRange);
        }
        let record = self.registry.get(endpoint_id)?;
        let client = client::connect_record(self.endpoint.clone(), &record).await?;
        let mut stream = tokio::time::timeout(
            REQUEST_TIMEOUT,
            client.stream(&Request {
                body: Some(request::Body::QueryHistory(HistoryQuery {
                    from_ms,
                    to_ms,
                    resolution: match resolution {
                        RemoteHistoryResolution::Raw => history_query::Resolution::Raw as i32,
                        RemoteHistoryResolution::Minute => history_query::Resolution::Minute as i32,
                    },
                    protocol: Some(current_version()),
                })),
            }),
        )
        .await
        .map_err(|_| RemoteFleetError::Timeout("history request"))??;
        let mut samples = Vec::new();
        // `REQUEST_TIMEOUT` bounds each frame, not the exchange, so a peer
        // emitting one frame just inside it could stream forever. Bound the
        // whole response in both time and rows: this daemon is the viewer that
        // must stay up, and an unbounded peer response would take it down.
        let deadline = Instant::now() + HISTORY_RESPONSE_TIMEOUT;
        loop {
            if Instant::now() >= deadline {
                return Err(RemoteFleetError::Timeout("history response"));
            }
            let response = tokio::time::timeout(REQUEST_TIMEOUT, stream.next())
                .await
                .map_err(|_| RemoteFleetError::Timeout("history response"))??;
            match response.body {
                Some(response::Body::MetricSample(sample)) => {
                    if samples.len() >= MAX_REMOTE_HISTORY_SAMPLES {
                        return Err(RemoteFleetError::HistoryResponseTooLarge);
                    }
                    samples.push(client::metric_sample(sample));
                }
                Some(response::Body::StreamComplete(_)) => break,
                Some(response::Body::Error(error)) => {
                    return Err(RemoteFleetError::Transport(TransportError::Remote {
                        code: error.code,
                        message: error.message,
                    }));
                }
                _ => return Err(RemoteFleetError::UnexpectedResponse("history event")),
            }
        }
        client.close();
        Ok(samples)
    }

    pub async fn import_pairing(
        &self,
        encoded_bundle: &str,
    ) -> Result<RemoteMachineSnapshot, RemoteFleetError> {
        let bundle = PairingBundle::decode(encoded_bundle.trim())?;
        validate_bundle(&bundle, Utc::now().timestamp_millis())?;
        if self.registry.contains(&bundle.endpoint_id)? {
            return Err(RemoteFleetError::AlreadyPaired);
        }

        let address = bundle.endpoint_addr()?;
        let client = tokio::time::timeout(
            CONNECT_TIMEOUT,
            ClientConnection::connect(self.endpoint.clone(), address),
        )
        .await
        .map_err(|_| RemoteFleetError::Timeout("connect"))??;
        if client.remote_id().to_string() != bundle.endpoint_id {
            client.close();
            return Err(RemoteFleetError::IdentityMismatch);
        }

        let pair_response = client::request(
            &client,
            Request {
                body: Some(request::Body::Pair(PairRequest {
                    one_time_secret: bundle.one_time_secret.clone(),
                    viewer_endpoint_id: client.local_id().to_string(),
                })),
            },
            "pairing",
        )
        .await?;
        let Some(response::Body::Pair(paired)) = pair_response.body else {
            client.close();
            return Err(RemoteFleetError::UnexpectedResponse("pairing response"));
        };
        if !paired.accepted {
            client.close();
            return Err(RemoteFleetError::PairingRejected);
        }
        if paired.node_id != bundle.node_id.to_string() || paired.endpoint_id != bundle.endpoint_id
        {
            client.close();
            return Err(RemoteFleetError::IdentityMismatch);
        }

        let node = client::get_node_info(&client).await?;
        if node.node_id != bundle.node_id {
            client.close();
            return Err(RemoteFleetError::IdentityMismatch);
        }
        let mut snapshot = RemoteMachineSnapshot {
            node: node.clone(),
            endpoint_id: bundle.endpoint_id.clone(),
            latest: None,
            // Pairing succeeded but health is not yet known. Start degraded so a
            // machine whose first health request fails is never presented as
            // healthy; the first successful refresh replaces this.
            state: NodeState::Degraded,
            path: ConnectionPath::Unknown,
            rtt_ms: None,
            last_seen_ms: Some(Utc::now().timestamp_millis()),
            trend: TrendWindow::default(),
            details: vec![String::from("health_unknown")],
        };
        match client::get_health(&client).await {
            Ok(health) => {
                snapshot.state = health.state;
                snapshot.details = health.details;
            }
            Err(error) => {
                tracing::warn!(
                    endpoint_id = %snapshot.endpoint_id,
                    error = %error,
                    "paired machine did not return health; reporting it as degraded"
                );
            }
        }
        if let Ok((path, rtt_ms)) = client::get_connection_path(&client).await {
            snapshot.path = path;
            snapshot.rtt_ms = Some(rtt_ms);
        }

        let record = RemoteMachineRecord {
            node,
            endpoint_id: bundle.endpoint_id,
            // Prefer the address this pairing session actually reached. A
            // bundle may advertise interfaces that are unreachable from here,
            // and the reachable one belongs at the front of the list.
            direct_addresses: monitor::merged_direct_addresses(
                &client.observed_direct_addresses(),
                &bundle.direct_addresses,
            ),
            relay_urls: bundle.relay_urls,
            paired_at_ms: Utc::now().timestamp_millis(),
            last_snapshot: None,
        };
        self.registry.insert(record.clone())?;
        self.snapshots
            .write()
            .await
            .insert(record.endpoint_id.clone(), snapshot.clone());
        client.close();
        self.spawn_monitor(record);
        Ok(snapshot)
    }

    fn spawn_monitor(&self, record: RemoteMachineRecord) {
        let endpoint = self.endpoint.clone();
        let registry = self.registry.clone();
        let snapshots = Arc::clone(&self.snapshots);
        tokio::spawn(async move {
            monitor::monitor_machine(endpoint, record, registry, snapshots).await;
        });
    }
}

fn validate_bundle(bundle: &PairingBundle, now_ms: i64) -> Result<(), PairingError> {
    if bundle.expires_at_ms < now_ms {
        return Err(PairingError::Expired);
    }
    if bundle.direct_addresses.is_empty() && bundle.relay_urls.is_empty() {
        return Err(PairingError::InvalidBundle);
    }
    bundle.endpoint_addr()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use rackio_iroh::PairingBundle;

    use super::{test_support::record, validate_bundle};

    #[test]
    fn expired_and_unreachable_bundles_fail_before_network_access() {
        let record = record();
        let mut bundle = PairingBundle {
            format_version: 1,
            node_id: record.node.node_id,
            endpoint_id: record.endpoint_id,
            direct_addresses: Vec::new(),
            relay_urls: Vec::new(),
            one_time_secret: String::from("must-not-persist"),
            expires_at_ms: 99,
        };
        assert!(validate_bundle(&bundle, 100).is_err());

        bundle.expires_at_ms = 101;
        assert!(validate_bundle(&bundle, 100).is_err());
    }
}
