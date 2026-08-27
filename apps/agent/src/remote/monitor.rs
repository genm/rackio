//! The per-machine monitoring loop: reconnect with backoff, watch the metric
//! stream, keep the in-memory snapshot fresh, and translate failures into the
//! state and recovery hints an operator reads.

use std::{
    collections::BTreeMap,
    net::SocketAddr,
    sync::Arc,
    time::{Duration, Instant},
};

use chrono::Utc;
use rackio_core::{ConnectionPath, NodeState, TrendSample};
use rackio_iroh::{ClientConnection, TransportError};
use rackio_protocol::{
    current_version,
    v1::{Request, request, response},
};
use tokio::sync::RwLock as AsyncRwLock;

use super::{
    RemoteFleetError,
    client::{
        REQUEST_TIMEOUT, connect_record, get_connection_path, get_health, get_node_info,
        metric_sample,
    },
    registry::{RemoteMachineRecord, RemoteMachineRegistry},
    snapshot::RemoteMachineSnapshot,
};

pub(super) const STREAM_SILENCE_TIMEOUT: Duration = Duration::from_secs(12);
/// How often a monitoring session may rewrite the persisted last-known snapshot.
///
/// Roughly the ten seconds the old `sequence % 5` rule produced at the
/// collector's two-second cadence, but measured on this viewer's clock, so a
/// peer cannot choose it. The registry rewrite is a full serialise plus fsync,
/// which is exactly the work a hostile peer would want to amplify.
const SNAPSHOT_PERSIST_INTERVAL: Duration = Duration::from_secs(10);
const INITIAL_RECONNECT_DELAY: Duration = Duration::from_secs(1);
const MAX_RECONNECT_DELAY: Duration = Duration::from_secs(30);
/// How many candidate direct addresses one paired machine may keep. A peer that
/// takes an ephemeral port publishes a new address on every restart, so an
/// unbounded union would grow this registry for as long as the pairing lives.
/// The most recently observed addresses are kept and older candidates fall off.
const MAX_RECORD_DIRECT_ADDRESSES: usize = 8;

pub(super) async fn monitor_machine(
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
    // Lives across reconnects, not inside a single session. A hostile peer can
    // force a fresh session at will by dropping the connection; if the persist
    // throttle reset with the session, that reconnect would be exactly as good
    // as a crafted sequence number for forcing a registry rewrite.
    let mut last_persisted: Option<Instant> = None;
    loop {
        let started = Instant::now();
        let result = monitor_session(
            endpoint.clone(),
            &mut record,
            &registry,
            Arc::clone(&snapshots),
            &mut last_persisted,
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
    last_persisted: &mut Option<Instant>,
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
    // The cadence of durable writes is this viewer's decision, measured on its
    // own clock. It used to be `sequence % 5`, a number the monitored machine
    // chooses: a peer that stamped every sample with a multiple of five and
    // streamed as fast as the link allowed drove one create-write-fsync-rename
    // of the whole registry per sample, with the file's size under its control
    // too through the node name and detail strings it supplies. The throttle
    // state itself lives in the caller's loop, not here, so it survives a
    // reconnect instead of resetting with every new session.

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
                let should_persist =
                    last_persisted.is_none_or(|last| last.elapsed() >= SNAPSHOT_PERSIST_INTERVAL);
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
                    *last_persisted = Some(Instant::now());
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
pub(super) fn merged_direct_addresses(
    observed: &[SocketAddr],
    known: &[SocketAddr],
) -> Vec<SocketAddr> {
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

#[cfg(test)]
mod tests {
    use std::net::SocketAddr;

    use rackio_core::ConnectionPath;

    use super::{
        MAX_RECORD_DIRECT_ADDRESSES, RemoteMachineSnapshot, apply_connection_path,
        merged_direct_addresses, unreachable_hint,
    };
    use crate::remote::{
        RemoteFleetError,
        test_support::{address, captured_logs, record},
    };

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
