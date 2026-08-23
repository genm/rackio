use std::{
    collections::BTreeMap,
    fs,
    io::Write,
    net::SocketAddr,
    path::{Path, PathBuf},
    sync::{Arc, RwLock},
    time::{Duration, Instant},
};

use chrono::Utc;
use rackio_core::{
    CapabilityState, CollectorError, ConnectionPath, DiskMetric, HealthSnapshot, MetricCapability,
    MetricSample, NetworkMetric, NodeInfo, NodeState, ProtocolVersion, TemperatureMetric,
    TrendSample, TrendWindow,
};
use rackio_iroh::{ClientConnection, PairingBundle, PairingError, TransportError};
use rackio_protocol::{
    current_version,
    v1::{HistoryQuery, PairRequest, Request, history_query, request, response},
};
use serde::{Deserialize, Serialize};
use thiserror::Error;
use tokio::sync::RwLock as AsyncRwLock;
use uuid::Uuid;

const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const REQUEST_TIMEOUT: Duration = Duration::from_secs(8);
const STREAM_SILENCE_TIMEOUT: Duration = Duration::from_secs(12);
const STALE_AFTER_MS: i64 = 10_000;
const OFFLINE_AFTER_MS: i64 = 30_000;
const HISTORY_RESPONSE_TIMEOUT: Duration = Duration::from_mins(1);
/// A peer cannot retain more history than the retention contract allows, so
/// anything beyond it is a malfunctioning or hostile peer rather than a range
/// this viewer asked for.
const MAX_REMOTE_HISTORY_SAMPLES: usize = rackio_core::MAX_QUERY_ROWS;
const INITIAL_RECONNECT_DELAY: Duration = Duration::from_secs(1);
const MAX_RECONNECT_DELAY: Duration = Duration::from_secs(30);
const MAX_HISTORY_RANGE_MS: i64 = 7 * 24 * 60 * 60 * 1_000;
/// How many candidate direct addresses one paired machine may keep. A peer that
/// takes an ephemeral port publishes a new address on every restart, so an
/// unbounded union would grow this registry for as long as the pairing lives.
/// The most recently observed addresses are kept and older candidates fall off.
const MAX_RECORD_DIRECT_ADDRESSES: usize = 8;

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

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
struct RemoteMachineRecord {
    node: NodeInfo,
    endpoint_id: String,
    direct_addresses: Vec<SocketAddr>,
    relay_urls: Vec<String>,
    paired_at_ms: i64,
    #[serde(default)]
    last_snapshot: Option<PersistedRemoteSnapshot>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
struct PersistedRemoteSnapshot {
    latest: Option<MetricSample>,
    state: NodeState,
    path: ConnectionPath,
    rtt_ms: Option<u64>,
    last_seen_ms: Option<i64>,
    /// Defaulted so registries written before the trend window existed still
    /// deserialise; their pre-trend `history` array is ignored and the window
    /// refills from the live stream.
    #[serde(default)]
    trend: TrendWindow,
    details: Vec<String>,
}

impl RemoteMachineRecord {
    fn endpoint_addr(&self) -> Result<iroh::EndpointAddr, PairingError> {
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

#[derive(Debug, Clone)]
struct RemoteMachineRegistry {
    path: PathBuf,
    records: Arc<RwLock<BTreeMap<String, RemoteMachineRecord>>>,
}

impl RemoteMachineRegistry {
    fn load(path: impl Into<PathBuf>) -> Result<Self, RemoteFleetError> {
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

    fn list(&self) -> Result<Vec<RemoteMachineRecord>, RemoteFleetError> {
        Ok(self
            .records
            .read()
            .map_err(|_| RemoteFleetError::RegistryUnavailable)?
            .values()
            .cloned()
            .collect())
    }

    fn contains(&self, endpoint_id: &str) -> Result<bool, RemoteFleetError> {
        Ok(self
            .records
            .read()
            .map_err(|_| RemoteFleetError::RegistryUnavailable)?
            .contains_key(endpoint_id))
    }

    fn get(&self, endpoint_id: &str) -> Result<RemoteMachineRecord, RemoteFleetError> {
        self.records
            .read()
            .map_err(|_| RemoteFleetError::RegistryUnavailable)?
            .get(endpoint_id)
            .cloned()
            .ok_or(RemoteFleetError::UnknownMachine)
    }

    fn insert(&self, record: RemoteMachineRecord) -> Result<(), RemoteFleetError> {
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
    fn update_addresses(
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

    fn update_snapshot(
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

#[derive(Debug, Clone, Serialize)]
pub struct RemoteMachineSnapshot {
    pub node: NodeInfo,
    pub endpoint_id: String,
    pub latest: Option<MetricSample>,
    pub state: NodeState,
    pub path: ConnectionPath,
    pub rtt_ms: Option<u64>,
    pub last_seen_ms: Option<i64>,
    pub trend: TrendWindow,
    pub details: Vec<String>,
}

impl RemoteMachineSnapshot {
    fn offline(record: &RemoteMachineRecord) -> Self {
        if let Some(persisted) = &record.last_snapshot {
            return Self {
                node: record.node.clone(),
                endpoint_id: record.endpoint_id.clone(),
                latest: persisted.latest.clone(),
                state: persisted.state,
                path: persisted.path,
                rtt_ms: persisted.rtt_ms,
                last_seen_ms: persisted.last_seen_ms,
                trend: persisted.trend.clone(),
                details: persisted.details.clone(),
            };
        }
        Self {
            node: record.node.clone(),
            endpoint_id: record.endpoint_id.clone(),
            latest: None,
            state: NodeState::Offline,
            path: ConnectionPath::Unknown,
            rtt_ms: None,
            last_seen_ms: None,
            trend: TrendWindow::default(),
            details: vec![String::from("Waiting for remote connection")],
        }
    }

    fn state_at(&self, now_ms: i64) -> NodeState {
        if matches!(self.state, NodeState::AuthError | NodeState::Incompatible) {
            return self.state;
        }
        let Some(last_seen_ms) = self.last_seen_ms else {
            return self.state;
        };
        // `saturating_sub` on i64 does not clamp at zero, so a clock that
        // moved backwards produced a negative age and froze this derivation at
        // the stored state. A machine cannot have been seen in the future:
        // treat that as just-seen rather than silently trusting stale state.
        let age_ms = now_ms.saturating_sub(last_seen_ms).max(0);
        if age_ms >= OFFLINE_AFTER_MS {
            NodeState::Offline
        } else if age_ms >= STALE_AFTER_MS {
            NodeState::Stale
        } else {
            self.state
        }
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
        let client = connect_record(self.endpoint.clone(), &record).await?;
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
                    samples.push(metric_sample(sample));
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

        let pair_response = request(
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

        let node = get_node_info(&client).await?;
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
        match get_health(&client).await {
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
        if let Ok((path, rtt_ms)) = get_connection_path(&client).await {
            snapshot.path = path;
            snapshot.rtt_ms = Some(rtt_ms);
        }

        let record = RemoteMachineRecord {
            node,
            endpoint_id: bundle.endpoint_id,
            // Prefer the address this pairing session actually reached. A
            // bundle may advertise interfaces that are unreachable from here,
            // and the reachable one belongs at the front of the list.
            direct_addresses: merged_direct_addresses(
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
            monitor_machine(endpoint, record, registry, snapshots).await;
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

async fn monitor_machine(
    endpoint: iroh::Endpoint,
    record: RemoteMachineRecord,
    registry: RemoteMachineRegistry,
    snapshots: Arc<AsyncRwLock<BTreeMap<String, RemoteMachineSnapshot>>>,
) {
    // The record is owned rather than borrowed because a session may learn that
    // the machine moved to another address; the next reconnect has to use the
    // refreshed set, not the one this task started with.
    let mut record = record;
    let mut retry_delay = INITIAL_RECONNECT_DELAY;
    loop {
        let started = Instant::now();
        let result = monitor_session(
            endpoint.clone(),
            &mut record,
            &registry,
            Arc::clone(&snapshots),
        )
        .await;
        if let Err(error) = result {
            update_error(&snapshots, &record, &error).await;
        }
        // A session that outlived the stream-silence timeout was genuinely
        // established, so the next failure starts from the base delay again.
        // Without this reset the backoff only ever grows, and a daemon running
        // for days reconnects at the 30-second ceiling even from healthy peers.
        if started.elapsed() >= STREAM_SILENCE_TIMEOUT {
            retry_delay = INITIAL_RECONNECT_DELAY;
        }
        tokio::time::sleep(retry_delay).await;
        retry_delay = retry_delay.saturating_mul(2).min(MAX_RECONNECT_DELAY);
    }
}

async fn monitor_session(
    endpoint: iroh::Endpoint,
    record: &mut RemoteMachineRecord,
    registry: &RemoteMachineRegistry,
    snapshots: Arc<AsyncRwLock<BTreeMap<String, RemoteMachineSnapshot>>>,
) -> Result<(), RemoteFleetError> {
    let client = connect_record(endpoint, record).await?;
    refresh_direct_addresses(&client, record, registry);
    let node = get_node_info(&client).await?;
    let health = get_health(&client).await?;
    let (path, rtt_ms) = get_connection_path(&client).await?;
    {
        let mut entries = snapshots.write().await;
        let snapshot = entries
            .entry(record.endpoint_id.clone())
            .or_insert_with(|| RemoteMachineSnapshot::offline(record));
        snapshot.node = node;
        snapshot.state = health.state;
        snapshot.details = health.details;
        apply_connection_path(snapshot, &record.endpoint_id, path, rtt_ms);
        snapshot.last_seen_ms = Some(Utc::now().timestamp_millis());
    }
    // A reconnect that lands on the same path is not a path change, so the
    // event above is silent for it — yet an operator reading the log still
    // needs to see when monitoring resumed and over what. Sessions start on
    // pairing and on recovery, not on a cadence, so this stays an event rather
    // than becoming noise.
    tracing::info!(
        endpoint_id = %record.endpoint_id,
        path = ?path,
        rtt_ms,
        "remote monitoring session established"
    );

    let mut stream = tokio::time::timeout(
        REQUEST_TIMEOUT,
        client.stream(&Request {
            body: Some(request::Body::WatchMetrics(current_version())),
        }),
    )
    .await
    .map_err(|_| RemoteFleetError::Timeout("watch metrics"))??;
    let mut state_refresh = tokio::time::interval(Duration::from_secs(5));
    state_refresh.tick().await;

    loop {
        let response = tokio::select! {
            response = tokio::time::timeout(STREAM_SILENCE_TIMEOUT, stream.next()) => {
                response
                    .map_err(|_| RemoteFleetError::Timeout("metrics heartbeat"))??
            }
            _ = state_refresh.tick() => {
                refresh_remote_state(&client, record, &snapshots).await?;
                continue;
            }
        };
        match response.body {
            Some(response::Body::MetricSample(sample)) => {
                let sample = metric_sample(sample);
                let should_persist = sample.sequence.is_multiple_of(5);
                let mut entries = snapshots.write().await;
                let snapshot = entries
                    .entry(record.endpoint_id.clone())
                    .or_insert_with(|| RemoteMachineSnapshot::offline(record));
                // RTT is the viewer's own measurement of this connection, so
                // it is stamped here rather than carried by the peer's sample.
                let mut point = TrendSample::from(&sample);
                point.rtt_ms = snapshot.rtt_ms;
                snapshot.trend.push(point);
                snapshot.latest = Some(sample);
                // Do not restamp `state` here. `refresh_remote_state` owns it
                // and refreshes it every five seconds; re-applying the
                // session-start health would pin a remote that later degraded
                // to its original value for the whole session.
                snapshot.last_seen_ms = Some(Utc::now().timestamp_millis());
                if should_persist {
                    let snapshot = snapshot.clone();
                    drop(entries);
                    if let Err(error) = registry.update_snapshot(&record.endpoint_id, &snapshot) {
                        tracing::warn!(
                            endpoint_id = %record.endpoint_id,
                            error = %error,
                            "failed to persist last-known remote snapshot"
                        );
                    }
                }
            }
            Some(response::Body::Heartbeat(_)) => {
                let mut entries = snapshots.write().await;
                if let Some(snapshot) = entries.get_mut(&record.endpoint_id) {
                    snapshot.last_seen_ms = Some(Utc::now().timestamp_millis());
                }
            }
            _ => return Err(RemoteFleetError::UnexpectedResponse("metrics event")),
        }
    }
}

/// Persist where this machine is currently reachable.
///
/// `client` is an authenticated session with the pinned endpoint ID, so its
/// addresses describe the machine this viewer is already paired with. Learning
/// them is what lets a viewer follow a peer that rebound to another port
/// instead of retrying the address it was paired on forever. It cannot
/// introduce a new peer, change which peer is authorized, or reach any
/// discovery service.
fn refresh_direct_addresses(
    client: &ClientConnection,
    record: &mut RemoteMachineRecord,
    registry: &RemoteMachineRegistry,
) {
    let observed = client.observed_direct_addresses();
    if observed.is_empty() {
        // A relay-only session says nothing about direct reachability. Keeping
        // the known addresses is better than clearing them.
        return;
    }
    let merged = merged_direct_addresses(&observed, &record.direct_addresses);
    if merged == record.direct_addresses {
        return;
    }
    match registry.update_addresses(&record.endpoint_id, merged.clone()) {
        Ok(()) => {
            tracing::info!(
                endpoint_id = %record.endpoint_id,
                address_count = merged.len(),
                "refreshed the direct addresses of a paired machine"
            );
            record.direct_addresses = merged;
        }
        Err(error) => {
            // The session is live either way, so this is not fatal: only the
            // next restart loses the refreshed address.
            tracing::warn!(
                endpoint_id = %record.endpoint_id,
                error = %error,
                "failed to persist refreshed direct addresses"
            );
        }
    }
}

/// Order the candidate addresses for the next connection attempt: the ones just
/// observed first, then previously known ones that a different network still
/// makes reachable, bounded so a peer with an ephemeral port cannot grow this
/// list without limit.
fn merged_direct_addresses(observed: &[SocketAddr], known: &[SocketAddr]) -> Vec<SocketAddr> {
    let mut merged: Vec<SocketAddr> = Vec::with_capacity(observed.len() + known.len());
    for address in observed.iter().chain(known.iter()) {
        if !merged.contains(address) {
            merged.push(*address);
        }
    }
    merged.truncate(MAX_RECORD_DIRECT_ADDRESSES);
    merged
}

async fn refresh_remote_state(
    client: &ClientConnection,
    record: &RemoteMachineRecord,
    snapshots: &AsyncRwLock<BTreeMap<String, RemoteMachineSnapshot>>,
) -> Result<(), RemoteFleetError> {
    let health = get_health(client).await?;
    let (path, rtt_ms) = get_connection_path(client).await?;
    let mut entries = snapshots.write().await;
    let snapshot = entries
        .entry(record.endpoint_id.clone())
        .or_insert_with(|| RemoteMachineSnapshot::offline(record));
    snapshot.state = health.state;
    snapshot.details = health.details;
    apply_connection_path(snapshot, &record.endpoint_id, path, rtt_ms);
    Ok(())
}

/// Record the path a session is running over, announcing it whenever it differs
/// from the one this viewer last reported.
///
/// The single owner of that rule. A mid-session migration is not the only way a
/// path changes: a machine that goes offline on `lan_direct` and comes back
/// through a relay changes path across the reconnect, and assigning it silently
/// there — as the session-start path once did — left the operator with a
/// relayed connection and no record of when it stopped being direct.
fn apply_connection_path(
    snapshot: &mut RemoteMachineSnapshot,
    endpoint_id: &str,
    path: ConnectionPath,
    rtt_ms: u64,
) {
    if snapshot.path != path {
        tracing::info!(
            endpoint_id = %endpoint_id,
            previous_path = ?snapshot.path,
            current_path = ?path,
            rtt_ms,
            "remote connection path changed"
        );
    }
    snapshot.path = path;
    snapshot.rtt_ms = Some(rtt_ms);
}

async fn connect_record(
    endpoint: iroh::Endpoint,
    record: &RemoteMachineRecord,
) -> Result<ClientConnection, RemoteFleetError> {
    let address = record.endpoint_addr()?;
    let client = tokio::time::timeout(
        CONNECT_TIMEOUT,
        ClientConnection::connect(endpoint, address),
    )
    .await
    .map_err(|_| RemoteFleetError::Timeout("connect"))??;
    if client.remote_id().to_string() != record.endpoint_id {
        client.close();
        return Err(RemoteFleetError::IdentityMismatch);
    }
    let node = get_node_info(&client).await?;
    if node.node_id != record.node.node_id {
        client.close();
        return Err(RemoteFleetError::IdentityMismatch);
    }
    Ok(client)
}

async fn update_error(
    snapshots: &AsyncRwLock<BTreeMap<String, RemoteMachineSnapshot>>,
    record: &RemoteMachineRecord,
    error: &RemoteFleetError,
) {
    let mut entries = snapshots.write().await;
    let snapshot = entries
        .entry(record.endpoint_id.clone())
        .or_insert_with(|| RemoteMachineSnapshot::offline(record));
    snapshot.state = match error {
        RemoteFleetError::Transport(TransportError::Remote { code, .. })
            if code == "auth_error" =>
        {
            NodeState::AuthError
        }
        RemoteFleetError::Transport(TransportError::Remote { code, .. })
            if code == "incompatible" =>
        {
            NodeState::Incompatible
        }
        RemoteFleetError::IdentityMismatch => NodeState::AuthError,
        _ => snapshot.state_at(Utc::now().timestamp_millis()),
    };
    snapshot.details = vec![error.to_string()];
    if let Some(hint) = unreachable_hint(error, !record.relay_urls.is_empty()) {
        // A viewer that only says "connect timed out" leaves the operator
        // guessing. Name the recoverable cause, because a machine that rebound
        // to another port looks exactly like one that is switched off.
        snapshot.details.push(String::from(hint));
    }
}

/// The recovery step for an error that means "no known address answered".
///
/// Returns `None` for errors that a different address would not fix, so an
/// authorization or compatibility failure is never dressed up as a reachability
/// problem.
///
/// A machine with a configured relay has a second way to become unreachable,
/// and naming only the listen port sends its operator to inspect a setting that
/// was never the cause. The relay is named first there because a relay outage
/// takes every relay-dependent machine down at once, which a port change cannot
/// do.
fn unreachable_hint(error: &RemoteFleetError, relay_configured: bool) -> Option<&'static str> {
    match error {
        RemoteFleetError::Timeout("connect")
        | RemoteFleetError::Transport(TransportError::Connect(_))
            if relay_configured =>
        {
            Some(
                "no known address answered and the configured relay did not carry the session; \
             check that the relay is running and reachable, or if this machine restarted on a \
             new port, give it a fixed one with `rackio listen-port set <PORT>` and restart it",
            )
        }
        RemoteFleetError::Timeout("connect")
        | RemoteFleetError::Transport(TransportError::Connect(_)) => Some(
            "no known address answered; if this machine restarted on a new port, \
             give it a fixed one with `rackio listen-port set <PORT>` and restart it, \
             or pair again",
        ),
        _ => None,
    }
}

async fn request(
    client: &ClientConnection,
    request: Request,
    operation: &'static str,
) -> Result<rackio_protocol::v1::Response, RemoteFleetError> {
    let response = tokio::time::timeout(REQUEST_TIMEOUT, client.request(&request))
        .await
        .map_err(|_| RemoteFleetError::Timeout(operation))?
        .map_err(RemoteFleetError::from)?;
    if let Some(response::Body::Error(error)) = &response.body {
        return Err(RemoteFleetError::Transport(TransportError::Remote {
            code: error.code.clone(),
            message: error.message.clone(),
        }));
    }
    Ok(response)
}

async fn get_node_info(client: &ClientConnection) -> Result<NodeInfo, RemoteFleetError> {
    let response = request(
        client,
        Request {
            body: Some(request::Body::GetNodeInfo(current_version())),
        },
        "node info",
    )
    .await?;
    let Some(response::Body::NodeInfo(info)) = response.body else {
        return Err(RemoteFleetError::UnexpectedResponse("node info"));
    };
    node_info(info)
}

async fn get_health(client: &ClientConnection) -> Result<HealthSnapshot, RemoteFleetError> {
    let response = request(
        client,
        Request {
            body: Some(request::Body::GetHealth(current_version())),
        },
        "health",
    )
    .await?;
    let Some(response::Body::Health(health)) = response.body else {
        return Err(RemoteFleetError::UnexpectedResponse("health"));
    };
    Ok(HealthSnapshot {
        state: node_state(health.state),
        collector_degraded: health.collector_degraded,
        storage_degraded: health.storage_degraded,
        remote_listener_degraded: health.remote_listener_degraded,
        details: health.details,
    })
}

async fn get_connection_path(
    client: &ClientConnection,
) -> Result<(ConnectionPath, u64), RemoteFleetError> {
    let response = request(
        client,
        Request {
            body: Some(request::Body::GetConnectionPath(current_version())),
        },
        "connection path",
    )
    .await?;
    let Some(response::Body::ConnectionPath(details)) = response.body else {
        return Err(RemoteFleetError::UnexpectedResponse("connection path"));
    };
    Ok((connection_path(details.path), details.rtt_ms))
}

fn node_info(info: rackio_protocol::v1::NodeInfo) -> Result<NodeInfo, RemoteFleetError> {
    let protocol = info
        .protocol
        .ok_or(RemoteFleetError::UnexpectedResponse("protocol version"))?;
    Ok(NodeInfo {
        node_id: Uuid::parse_str(&info.node_id).map_err(|_| RemoteFleetError::IdentityMismatch)?,
        display_name: info.display_name,
        os: info.os,
        architecture: info.architecture,
        agent_version: info.agent_version,
        protocol: ProtocolVersion {
            major: protocol.major,
            minor: protocol.minor,
        },
        capabilities: info
            .capabilities
            .into_iter()
            .map(|capability| MetricCapability {
                name: capability.name,
                state: capability_state(capability.state),
                detail: capability.detail,
            })
            .collect(),
    })
}

fn metric_sample(sample: rackio_protocol::v1::MetricSample) -> MetricSample {
    MetricSample {
        timestamp_ms: sample.timestamp_ms,
        sequence: sample.sequence,
        cpu_percent: sample.cpu_percent,
        memory_used_bytes: sample.memory_used_bytes,
        memory_total_bytes: sample.memory_total_bytes,
        swap_used_bytes: sample.swap_used_bytes,
        swap_total_bytes: sample.swap_total_bytes,
        disks: sample
            .disks
            .into_iter()
            .map(|disk| DiskMetric {
                mount: disk.mount,
                total_bytes: disk.total_bytes,
                used_bytes: disk.used_bytes,
            })
            .collect(),
        network: sample.network.map(|network| NetworkMetric {
            received_bytes_per_second: network.received_bytes_per_second,
            sent_bytes_per_second: network.sent_bytes_per_second,
        }),
        temperature: sample.temperature.map(|temperature| TemperatureMetric {
            label: temperature.label,
            celsius: temperature.celsius,
            critical_celsius: temperature.critical_celsius,
            sensor_count: temperature.sensor_count,
        }),
        uptime_seconds: sample.uptime_seconds,
        errors: sample
            .errors
            .into_iter()
            .map(|error| CollectorError {
                source: error.source,
                kind: capability_state(error.kind),
                message: error.message,
            })
            .collect(),
    }
}

fn capability_state(state: i32) -> CapabilityState {
    match rackio_protocol::v1::CapabilityState::try_from(state) {
        Ok(rackio_protocol::v1::CapabilityState::Supported) => CapabilityState::Supported,
        Ok(rackio_protocol::v1::CapabilityState::PermissionDenied) => {
            CapabilityState::PermissionDenied
        }
        _ => CapabilityState::Unsupported,
    }
}

fn node_state(state: i32) -> NodeState {
    match rackio_protocol::v1::NodeState::try_from(state) {
        Ok(rackio_protocol::v1::NodeState::Healthy) => NodeState::Healthy,
        Ok(rackio_protocol::v1::NodeState::Warning) => NodeState::Warning,
        Ok(rackio_protocol::v1::NodeState::Critical) => NodeState::Critical,
        Ok(rackio_protocol::v1::NodeState::Stale) => NodeState::Stale,
        Ok(rackio_protocol::v1::NodeState::Offline) => NodeState::Offline,
        Ok(rackio_protocol::v1::NodeState::AuthError) => NodeState::AuthError,
        Ok(rackio_protocol::v1::NodeState::Incompatible) => NodeState::Incompatible,
        _ => NodeState::Degraded,
    }
}

fn connection_path(path: i32) -> ConnectionPath {
    match rackio_protocol::v1::ConnectionPath::try_from(path) {
        Ok(rackio_protocol::v1::ConnectionPath::LanDirect) => ConnectionPath::LanDirect,
        Ok(rackio_protocol::v1::ConnectionPath::WanDirect) => ConnectionPath::WanDirect,
        Ok(rackio_protocol::v1::ConnectionPath::Relayed) => ConnectionPath::Relayed,
        _ => ConnectionPath::Unknown,
    }
}

#[cfg(test)]
mod tests {
    use iroh::SecretKey;

    use super::{
        MAX_RECORD_DIRECT_ADDRESSES, OFFLINE_AFTER_MS, RemoteFleetError, RemoteMachineRecord,
        RemoteMachineRegistry, RemoteMachineSnapshot, STALE_AFTER_MS, SocketAddr,
        apply_connection_path, merged_direct_addresses, unreachable_hint, validate_bundle,
    };
    use rackio_core::{ConnectionPath, NodeInfo, NodeState, ProtocolVersion};
    use rackio_iroh::PairingBundle;
    use uuid::Uuid;

    fn record() -> RemoteMachineRecord {
        RemoteMachineRecord {
            node: NodeInfo {
                node_id: Uuid::new_v4(),
                display_name: String::from("Test server"),
                os: String::from("linux"),
                architecture: String::from("x86_64"),
                agent_version: String::from("0.1.0"),
                protocol: ProtocolVersion::V1,
                capabilities: Vec::new(),
            },
            endpoint_id: SecretKey::generate().public().to_string(),
            direct_addresses: vec![
                "127.0.0.1:49100"
                    .parse()
                    .unwrap_or_else(|error| panic!("{error}")),
            ],
            relay_urls: Vec::new(),
            paired_at_ms: 1,
            last_snapshot: None,
        }
    }

    /// Collect the tracing output of one call, so a test can assert on the
    /// event an operator actually reads rather than only on the field it left
    /// behind in memory.
    fn captured_logs(body: impl FnOnce()) -> String {
        #[derive(Clone, Default)]
        struct Buffer(std::sync::Arc<std::sync::Mutex<Vec<u8>>>);

        impl std::io::Write for Buffer {
            fn write(&mut self, data: &[u8]) -> std::io::Result<usize> {
                let mut buffer = self
                    .0
                    .lock()
                    .map_err(|_| std::io::Error::other("log buffer was poisoned"))?;
                buffer.extend_from_slice(data);
                Ok(data.len())
            }

            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }

        impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for Buffer {
            type Writer = Self;

            fn make_writer(&'a self) -> Self::Writer {
                self.clone()
            }
        }

        let buffer = Buffer::default();
        let subscriber = tracing_subscriber::fmt()
            .with_writer(buffer.clone())
            .with_ansi(false)
            .finish();
        tracing::subscriber::with_default(subscriber, body);
        let bytes = buffer
            .0
            .lock()
            .unwrap_or_else(|error| panic!("{error}"))
            .clone();
        String::from_utf8(bytes).unwrap_or_else(|error| panic!("{error}"))
    }

    #[test]
    fn a_path_that_changed_while_the_machine_was_away_is_announced() {
        // The reconnect case, not a mid-session migration: a machine that was
        // last seen on a direct path and comes back through a relay must say
        // so. Assigning the new path silently would leave the operator with a
        // relayed connection and no record of when it stopped being direct.
        let record = record();
        let mut snapshot = RemoteMachineSnapshot::offline(&record);
        snapshot.path = ConnectionPath::LanDirect;

        let logs = captured_logs(|| {
            apply_connection_path(
                &mut snapshot,
                &record.endpoint_id,
                ConnectionPath::Relayed,
                42,
            );
        });

        assert!(
            logs.contains("remote connection path changed"),
            "a path change must be announced, got: {logs}"
        );
        assert!(logs.contains("previous_path=LanDirect"), "got: {logs}");
        assert!(logs.contains("current_path=Relayed"), "got: {logs}");
        assert_eq!(snapshot.path, ConnectionPath::Relayed);
        assert_eq!(snapshot.rtt_ms, Some(42));
    }

    #[test]
    fn an_unchanged_path_is_not_announced_again() {
        // A refresh every few seconds must not narrate a connection that has
        // not moved, or the events that do matter are lost in the repetition.
        let record = record();
        let mut snapshot = RemoteMachineSnapshot::offline(&record);
        snapshot.path = ConnectionPath::WanDirect;

        let logs = captured_logs(|| {
            apply_connection_path(
                &mut snapshot,
                &record.endpoint_id,
                ConnectionPath::WanDirect,
                7,
            );
        });

        assert!(
            !logs.contains("remote connection path changed"),
            "an unchanged path must stay quiet, got: {logs}"
        );
        assert_eq!(snapshot.rtt_ms, Some(7));
    }

    #[test]
    fn stale_and_offline_are_derived_from_local_last_seen_time() {
        let record = record();
        let mut snapshot = RemoteMachineSnapshot::offline(&record);
        snapshot.state = NodeState::Healthy;
        snapshot.last_seen_ms = Some(10_000);

        assert_eq!(snapshot.state_at(10_000 + STALE_AFTER_MS), NodeState::Stale);
        assert_eq!(
            snapshot.state_at(10_000 + OFFLINE_AFTER_MS),
            NodeState::Offline
        );

        snapshot.state = NodeState::AuthError;
        assert_eq!(
            snapshot.state_at(10_000 + OFFLINE_AFTER_MS),
            NodeState::AuthError
        );
    }

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

    fn address(value: &str) -> SocketAddr {
        value
            .parse()
            .unwrap_or_else(|error| panic!("{value} is not a socket address: {error}"))
    }

    #[test]
    fn a_moved_machine_is_tried_at_its_current_address_first() {
        let merged = merged_direct_addresses(
            &[address("127.0.0.1:49200")],
            &[address("127.0.0.1:49100"), address("192.168.1.5:49100")],
        );

        assert_eq!(
            merged,
            vec![
                address("127.0.0.1:49200"),
                address("127.0.0.1:49100"),
                address("192.168.1.5:49100"),
            ],
            "the observed address leads, and an address on another network is kept"
        );
    }

    #[test]
    fn refreshed_addresses_neither_duplicate_nor_grow_without_limit() {
        let known: Vec<SocketAddr> = (0..MAX_RECORD_DIRECT_ADDRESSES + 4)
            .map(|index| address(&format!("127.0.0.1:{}", 49_100 + index)))
            .collect();

        let merged = merged_direct_addresses(&[known[0], known[0]], &known);

        assert_eq!(merged.len(), MAX_RECORD_DIRECT_ADDRESSES);
        assert_eq!(merged[0], known[0]);
        assert_eq!(
            merged.iter().filter(|entry| **entry == known[0]).count(),
            1,
            "an address observed again must not be stored twice"
        );
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

    #[test]
    fn only_a_reachability_failure_suggests_a_reachability_fix() {
        assert!(
            unreachable_hint(&RemoteFleetError::Timeout("connect"), false).is_some(),
            "an operator whose machine moved needs to be told what to do"
        );
        assert!(
            unreachable_hint(&RemoteFleetError::IdentityMismatch, false).is_none(),
            "an identity failure is not fixed by another address"
        );
        assert!(
            unreachable_hint(&RemoteFleetError::Timeout("health"), false).is_none(),
            "a reachable machine that answered slowly is not unreachable"
        );
    }

    #[test]
    fn a_relay_machine_is_not_told_to_go_and_check_its_listen_port() {
        // A relay outage and a moved listen port look identical from here, but
        // they are not fixed in the same place — and an outage takes every
        // relay-dependent machine down at once, so pointing its operator at a
        // per-machine port setting sends them to the wrong screen entirely.
        let with_relay = unreachable_hint(&RemoteFleetError::Timeout("connect"), true)
            .unwrap_or_else(|| panic!("a relay machine still needs a recovery step"));
        assert!(
            with_relay.contains("relay"),
            "the relay must be named as a cause, got: {with_relay}"
        );
        assert!(
            with_relay.contains("listen-port"),
            "the address-change cause does not stop applying, got: {with_relay}"
        );

        let without_relay = unreachable_hint(&RemoteFleetError::Timeout("connect"), false)
            .unwrap_or_else(|| panic!("a direct machine still needs a recovery step"));
        assert!(
            !without_relay.contains("relay"),
            "a direct-only machine has no relay to check, got: {without_relay}"
        );
    }
}
