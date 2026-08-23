//! Agent lifecycle composition and supervision.

mod config;
mod local_ipc;
mod sampling;

use std::{sync::Arc, time::Duration};

use anyhow::anyhow;
use rackio_core::{
    HealthSnapshot, HistoryResolution, MetricCapability, MetricStore, NodeInfo, NodeState,
    ProtocolVersion, SystemCollector,
};
use rackio_iroh::{
    EndpointConfig, NodeRuntime, PairingManager, PairingMdnsState, PeerRegistry, RemoteServer,
    load_or_create_secret_key,
};
use tokio::sync::{RwLock, watch};
use uuid::Uuid;

use crate::remote::RemoteFleet;
use config::{create_directories, init_logging, load_config, load_or_create_node_id};
use local_ipc::run_local_server;
use sampling::sample_loop;

pub use config::{AppPaths, app_paths};
pub use local_ipc::{LocalCommand, LocalResponse, request_local};

const SHUTDOWN_FLUSH_TIMEOUT: Duration = Duration::from_secs(5);
/// How far back a restart reads to refill the local trend. Generous enough to
/// cover a full `TrendWindow` at the two-second cadence; the window itself
/// caps how many of those samples are kept.
const LOCAL_TREND_SEED_MS: i64 = 15 * 60 * 1_000;

/// Resolve on an operator-initiated stop. `systemctl stop`, container runtimes
/// and package upgrades all send SIGTERM, so waiting only on Ctrl-C would skip
/// the shutdown path on every real service stop.
async fn shutdown_signal() -> anyhow::Result<()> {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{SignalKind, signal};
        let mut terminate = signal(SignalKind::terminate())?;
        tokio::select! {
            result = tokio::signal::ctrl_c() => result.map_err(anyhow::Error::from),
            _ = terminate.recv() => Ok(()),
        }
    }
    #[cfg(not(unix))]
    {
        tokio::signal::ctrl_c().await.map_err(anyhow::Error::from)
    }
}

pub async fn run_daemon(paths: AppPaths) -> anyhow::Result<()> {
    create_directories(&paths)?;
    init_logging(&paths)?;

    let config = load_config(&paths)?;
    let secret = load_or_create_secret_key(&paths.data.join("identity.key"))?;
    let endpoint = rackio_iroh::bind_endpoint(
        secret,
        &EndpointConfig {
            relay_urls: config.relay_url.clone().into_iter().collect(),
            bind_port: config.bind_port,
        },
    )
    .await?;
    let node_id = load_or_create_node_id(&paths.data.join("node-id"))?;
    // Probe the host before advertising what this machine can collect, and
    // hand the same collector to the sampler so the advertised capabilities
    // and the published samples come from one source.
    let collector = SystemCollector::new();
    let info = node_info(node_id, collector.capabilities());
    let (latest_tx, latest_rx) = watch::channel(None);
    let store = MetricStore::open(paths.data.join("metrics.sqlite3"))?;
    let trend = resume_local_trend(&store);
    let runtime = Arc::new(NodeRuntime {
        info,
        health: RwLock::new(healthy()),
        latest: latest_rx,
        trend: RwLock::new(trend),
        store: tokio::sync::Mutex::new(store),
        pairing: std::sync::Mutex::new(PairingManager::default()),
        pairing_mdns: Arc::new(PairingMdnsState::default()),
        peers: PeerRegistry::load(paths.data.join("peers.json"))?,
        active_connections: std::sync::Mutex::new(std::collections::BTreeMap::new()),
    });
    let server = RemoteServer::new(endpoint.clone(), Arc::clone(&runtime));
    let remote_fleet =
        RemoteFleet::load(endpoint.clone(), paths.data.join("monitored-machines.json"))?;
    remote_fleet.start()?;

    tracing::info!(
        endpoint_id = %endpoint.id(),
        relay_mode = if config.relay_url.is_some() { "self_hosted" } else { "direct_only" },
        listen_port = listen_port_label(config.bind_port),
        "agent started"
    );

    let (shutdown_tx, shutdown_rx) = watch::channel(false);
    // Logged because "why did this machine never warn me?" is answerable only
    // if the rules the daemon actually runs are on the record.
    let alert_rules = config.alert_rules();
    tracing::info!(
        rules = ?alert_rules
            .iter()
            .map(|rule| rule.id.as_str())
            .collect::<Vec<_>>(),
        source = if config.alerts.is_some() { "configured" } else { "built_in_defaults" },
        "local health thresholds loaded"
    );
    let mut sampler = tokio::spawn(sample_loop(
        Arc::clone(&runtime),
        collector,
        alert_rules,
        latest_tx,
        shutdown_rx,
    ));
    let mut remote = tokio::spawn(server.run());
    let mut local = tokio::spawn(run_local_server(
        paths.clone(),
        endpoint.clone(),
        runtime,
        remote_fleet,
    ));

    // Polling a `JoinHandle` that already resolved panics, so remember whether
    // the sampler is the branch that ended the select.
    let mut sampler_finished = false;
    let result = tokio::select! {
        signal = shutdown_signal() => signal,
        stopped = &mut sampler => {
            sampler_finished = true;
            Err(anyhow!("metric sampler stopped unexpectedly: {stopped:?}"))
        }
        stopped = &mut remote => Err(anyhow!("remote listener stopped unexpectedly: {stopped:?}")),
        stopped = &mut local => match stopped {
            Ok(Ok(())) => Err(anyhow!("local IPC listener stopped unexpectedly")),
            Ok(Err(error)) => Err(error.context("local IPC listener failed")),
            Err(error) => Err(error.into()),
        },
    };
    // Let the sampler commit its buffered batch before the process exits.
    // Aborting it outright would discard up to ten seconds of history on every
    // service stop, restart and upgrade.
    let _ = shutdown_tx.send(true);
    if !sampler_finished {
        match tokio::time::timeout(SHUTDOWN_FLUSH_TIMEOUT, &mut sampler).await {
            Ok(Ok(())) => {}
            Ok(Err(error)) => tracing::warn!(error = %error, "metric sampler ended abnormally"),
            Err(_) => {
                tracing::warn!("metric sampler did not flush within the shutdown timeout");
                sampler.abort();
            }
        }
    }
    endpoint.close().await;
    remote.abort();
    local.abort();
    if let Err(error) = &result {
        tracing::error!(error = %error, "agent stopped because a required task failed");
    } else {
        tracing::info!("agent stopped");
    }
    result
}

/// Resume this machine's own trend from storage. Without it a restart shows a
/// blank chart for the local machine while every remote keeps the window its
/// registry persisted. A failed read degrades to an empty window — the chart
/// then says it is collecting, which is true.
fn resume_local_trend(store: &MetricStore) -> rackio_core::TrendWindow {
    let seed_now_ms = rackio_core::Clock::new().now_ms();
    match store.query(
        seed_now_ms.saturating_sub(LOCAL_TREND_SEED_MS),
        seed_now_ms,
        HistoryResolution::Raw,
    ) {
        Ok(samples) => rackio_core::TrendWindow::from_samples(&samples),
        Err(error) => {
            tracing::warn!(error = %error, "local trend could not be resumed from storage");
            rackio_core::TrendWindow::default()
        }
    }
}

/// Describe the configured listen port for the startup log. An ephemeral port
/// is named as such: it is the difference between a restart that already paired
/// viewers survive and one that strands them.
fn listen_port_label(bind_port: Option<u16>) -> String {
    bind_port.map_or_else(|| String::from("ephemeral"), |port| port.to_string())
}

/// Build the advertised node information from what this host can actually
/// collect. Declaring a fixed list of `Supported` capabilities made a viewer
/// trust metrics the collector cannot read on a sandboxed or containerised
/// host.
fn node_info(node_id: Uuid, capabilities: Vec<MetricCapability>) -> NodeInfo {
    NodeInfo {
        node_id,
        display_name: sysinfo::System::host_name().unwrap_or_else(|| String::from("Unnamed node")),
        os: std::env::consts::OS.to_owned(),
        architecture: std::env::consts::ARCH.to_owned(),
        agent_version: env!("CARGO_PKG_VERSION").to_owned(),
        protocol: ProtocolVersion::V1,
        capabilities,
    }
}

fn healthy() -> HealthSnapshot {
    HealthSnapshot {
        state: NodeState::Healthy,
        collector_degraded: false,
        storage_degraded: false,
        remote_listener_degraded: false,
        details: Vec::new(),
    }
}
