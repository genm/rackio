mod ssh_bootstrap;

use base64::{
    Engine as _,
    engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD},
};
use qrcode::{QrCode, render::svg};
use serde::Deserialize;
use std::{
    collections::{HashMap, HashSet},
    fs::OpenOptions,
    io::Write,
    path::PathBuf,
    sync::Mutex,
    time::Duration,
};
use tauri::{
    AppHandle, Manager, Runtime,
    image::Image,
    menu::{Menu, MenuBuilder, MenuItem},
    tray::TrayIconBuilder,
};

#[tauri::command]
async fn fleet_snapshot() -> Result<serde_json::Value, String> {
    let response = daemon_request(serde_json::json!({ "command": "fleet_snapshot" }))
        .await
        .map_err(|error| error.to_string())?;
    let data = response
        .get("data")
        .ok_or_else(|| String::from("daemon response did not contain data"))?;
    let local = data
        .get("local")
        .ok_or_else(|| String::from("daemon fleet did not contain the local machine"))?;
    let local_node = machine_json(
        local,
        local
            .get("health")
            .and_then(|health| health.get("state"))
            .and_then(serde_json::Value::as_str)
            .unwrap_or("degraded"),
        "unknown",
        None,
        local
            .get("latest")
            .and_then(|latest| latest.get("timestamp_ms"))
            .and_then(serde_json::Value::as_i64),
        local
            .get("trend")
            .and_then(serde_json::Value::as_array)
            .map_or(&[][..], Vec::as_slice),
        local
            .get("health")
            .and_then(|health| health.get("details"))
            .and_then(serde_json::Value::as_array)
            .and_then(|details| details.first())
            .and_then(serde_json::Value::as_str),
    )?;
    let remotes = data
        .get("remotes")
        .and_then(serde_json::Value::as_array)
        .ok_or_else(|| String::from("daemon fleet did not contain remote machines"))?;
    let mut nodes = Vec::with_capacity(remotes.len().saturating_add(1));
    nodes.push(local_node);
    for remote in remotes {
        let trend = remote
            .get("trend")
            .and_then(serde_json::Value::as_array)
            .map_or(&[][..], Vec::as_slice);
        nodes.push(machine_json(
            remote,
            remote
                .get("state")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("degraded"),
            remote
                .get("path")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("unknown"),
            remote.get("rtt_ms").and_then(serde_json::Value::as_u64),
            remote
                .get("last_seen_ms")
                .and_then(serde_json::Value::as_i64),
            trend,
            remote
                .get("details")
                .and_then(serde_json::Value::as_array)
                .and_then(|details| details.first())
                .and_then(serde_json::Value::as_str),
        )?);
    }
    Ok(serde_json::json!({
        "daemon": "connected",
        "nodes": nodes,
    }))
}

#[tauri::command]
async fn pair_machine(bundle: String) -> Result<serde_json::Value, String> {
    let response = daemon_request(serde_json::json!({
        "command": "pairing_import",
        "bundle": bundle,
    }))
    .await
    .map_err(|error| error.to_string())?;
    response
        .get("data")
        .cloned()
        .ok_or_else(|| String::from("daemon pairing response did not contain data"))
}

#[tauri::command]
async fn create_pairing_share() -> Result<serde_json::Value, String> {
    let response = daemon_request(serde_json::json!({ "command": "pairing_create" }))
        .await
        .map_err(|error| error.to_string())?;
    let bundle = response
        .get("data")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| String::from("daemon pairing response did not contain a bundle"))?;
    // Fail closed: the viewer promises a five-minute window, so a bundle whose
    // expiry cannot be read must not be presented as an open pairing window.
    let expires_at_ms = pairing_bundle_expiry(bundle)?;
    let qr = pairing_qr_data_url(bundle);
    let lan_warning = response
        .get("warnings")
        .and_then(serde_json::Value::as_array)
        .and_then(|warnings| warnings.first())
        .and_then(serde_json::Value::as_str);
    Ok(match qr {
        Ok(qr_data_url) => serde_json::json!({
            "bundle": bundle,
            "expiresAtMs": expires_at_ms,
            "qrDataUrl": qr_data_url,
            "lanWarning": lan_warning,
        }),
        Err(error) => serde_json::json!({
            "bundle": bundle,
            "expiresAtMs": expires_at_ms,
            "qrError": error,
            "lanWarning": lan_warning,
        }),
    })
}

/// Read the pairing window's expiry out of the locally generated bundle.
///
/// The bundle is `rackio-pair:` plus URL-safe base64 JSON produced by
/// `rackio-iroh`; only `expires_at_ms` is read here so the desktop does not
/// take a dependency on the transport crate or touch the one-time secret.
fn pairing_bundle_expiry(bundle: &str) -> Result<i64, String> {
    let payload = bundle
        .strip_prefix("rackio-pair:")
        .ok_or_else(|| String::from("daemon returned a bundle without a pairing prefix"))?;
    let decoded = URL_SAFE_NO_PAD
        .decode(payload)
        .map_err(|_| String::from("daemon pairing bundle is not valid base64"))?;
    serde_json::from_slice::<serde_json::Value>(&decoded)
        .ok()
        .as_ref()
        .and_then(|value| value.get("expires_at_ms"))
        .and_then(serde_json::Value::as_i64)
        .ok_or_else(|| String::from("daemon pairing bundle did not declare an expiry"))
}

#[tauri::command]
fn save_pairing_bundle(path: PathBuf, bundle: String) -> Result<(), String> {
    if !bundle.starts_with("rackio-pair:") || bundle.len() > 16 * 1024 {
        return Err(String::from("refusing to save an invalid pairing bundle"));
    }
    if path.as_os_str().is_empty() {
        return Err(String::from("pairing bundle path cannot be empty"));
    }

    let mut options = OpenOptions::new();
    options.create(true).truncate(true).write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options
        .open(path)
        .map_err(|error| format!("failed to open the pairing bundle file: {error}"))?;
    let mut contents = bundle.into_bytes();
    contents.push(b'\n');
    file.write_all(&contents)
        .and_then(|()| file.sync_all())
        .map_err(|error| format!("failed to save the pairing bundle: {error}"))
}

fn pairing_qr_data_url(bundle: &str) -> Result<String, String> {
    let code = QrCode::new(bundle.as_bytes())
        .map_err(|error| format!("Pairing bundle is too large for a QR code: {error}"))?;
    let svg = code
        .render::<svg::Color>()
        .min_dimensions(240, 240)
        .dark_color(svg::Color("#0b0f0d"))
        .light_color(svg::Color("#f3f6f1"))
        .build();
    Ok(format!(
        "data:image/svg+xml;base64,{}",
        STANDARD.encode(svg)
    ))
}

#[tauri::command]
async fn machine_history(endpoint_id: String, hours: u16) -> Result<serde_json::Value, String> {
    if endpoint_id.is_empty() || !(1..=168).contains(&hours) {
        return Err(String::from(
            "History requires a paired endpoint and a range between 1 and 168 hours.",
        ));
    }
    let to_ms = chrono::Utc::now().timestamp_millis();
    let from_ms = to_ms.saturating_sub(i64::from(hours) * 60 * 60 * 1_000);
    let response = daemon_request(serde_json::json!({
        "command": "query_history",
        "endpoint_id": endpoint_id,
        "from_ms": from_ms,
        "to_ms": to_ms,
        "resolution": "minute",
    }))
    .await
    .map_err(|error| error.to_string())?;
    let samples = response
        .get("data")
        .and_then(serde_json::Value::as_array)
        .ok_or_else(|| String::from("daemon history response did not contain samples"))?;
    Ok(serde_json::Value::Array(
        samples.iter().map(history_point_from_sample).collect(),
    ))
}

/// Project one daemon-reported `MetricSample` (`snake_case`) into the
/// camelCase `HistoryPoint` shape the frontend expects.
///
/// The peer's minute buckets report the fullest disk and the hottest sensor
/// as a single averaged reading, carried as a one-element `disks` array so
/// the minute resolution can reuse the raw resolution's wire shape — see
/// `MetricStore::query_minute`.
fn history_point_from_sample(sample: &serde_json::Value) -> serde_json::Value {
    let worst_disk = sample
        .get("disks")
        .and_then(serde_json::Value::as_array)
        .and_then(|disks| disks.first());
    serde_json::json!({
        "timestampMs": sample.get("timestamp_ms"),
        "cpuPercent": sample.get("cpu_percent"),
        "memoryUsedBytes": sample.get("memory_used_bytes"),
        "memoryTotalBytes": sample.get("memory_total_bytes"),
        "diskUsedBytes": worst_disk.and_then(|disk| disk.get("used_bytes")),
        "diskTotalBytes": worst_disk.and_then(|disk| disk.get("total_bytes")),
        "temperatureCelsius": sample
            .get("temperature")
            .and_then(|temperature| temperature.get("celsius")),
        "networkReceivedBytesPerSecond": sample
            .get("network")
            .and_then(|network| network.get("received_bytes_per_second")),
        "networkSentBytesPerSecond": sample
            .get("network")
            .and_then(|network| network.get("sent_bytes_per_second")),
    })
}

struct TrayRegistry<R: Runtime = tauri::Wry> {
    machine_ids: Mutex<HashSet<String>>,
    menus: Mutex<HashMap<String, TrayMenuState<R>>>,
}

impl<R: Runtime> Default for TrayRegistry<R> {
    fn default() -> Self {
        Self {
            machine_ids: Mutex::default(),
            menus: Mutex::default(),
        }
    }
}

struct TrayMenuState<R: Runtime> {
    detail_items: Vec<MenuItem<R>>,
}

impl<R: Runtime> Clone for TrayMenuState<R> {
    fn clone(&self) -> Self {
        Self {
            detail_items: self.detail_items.clone(),
        }
    }
}

#[derive(Debug, Deserialize)]
struct TrayFleetSnapshot {
    daemon: String,
    nodes: Vec<TrayNodeSnapshot>,
}

#[derive(Debug, Deserialize)]
struct TrayNodeSnapshot {
    id: String,
    name: String,
    state: String,
    path: String,
    #[serde(rename = "cpuPercent")]
    cpu_percent: Option<f64>,
    #[serde(rename = "memoryUsedBytes")]
    memory_used_bytes: Option<u64>,
    #[serde(rename = "memoryTotalBytes")]
    memory_total_bytes: Option<u64>,
    #[serde(rename = "diskUsedBytes")]
    disk_used_bytes: Option<u64>,
    #[serde(rename = "diskTotalBytes")]
    disk_total_bytes: Option<u64>,
    temperature: Option<TrayTemperature>,
    #[serde(rename = "rttMs")]
    rtt_ms: Option<u64>,
}

#[derive(Debug, Deserialize)]
struct TrayTemperature {
    label: String,
    celsius: f64,
}

#[derive(Debug, PartialEq, Eq)]
struct TrayMachineMenu {
    title: String,
    details: Vec<String>,
}

fn machine_tray_id(node: &TrayNodeSnapshot) -> String {
    format!("machine-{}", node.id)
}

fn tray_state_color(state: &str) -> Result<[u8; 4], String> {
    match state {
        "healthy" => Ok([84, 217, 139, 255]),
        "warning" | "degraded" => Ok([230, 189, 89, 255]),
        "stale" => Ok([164, 173, 168, 255]),
        "critical" | "offline" | "auth_error" | "incompatible" | "daemon_unavailable" => {
            Ok([255, 111, 103, 255])
        }
        _ => Err(String::from("Unknown fleet state for tray icon.")),
    }
}

fn tray_event_handler<R: Runtime>(app: &AppHandle<R>, event: &tauri::menu::MenuEvent) {
    match event.id().as_ref() {
        "show" => {
            if let Some(window) = app.get_webview_window("main") {
                let _ = window.show();
                let _ = window.set_focus();
            }
        }
        "quit" => app.exit(0),
        _ => {}
    }
}

fn tray_menu<R: Runtime, M: Manager<R>>(
    app: &M,
    details: &[String],
) -> Result<(Menu<R>, TrayMenuState<R>), tauri::Error> {
    let detail_items = details
        .iter()
        .enumerate()
        .map(|(index, text)| {
            MenuItem::with_id(
                app,
                format!("machine-detail-{index}"),
                text,
                false,
                None::<&str>,
            )
        })
        .collect::<Result<Vec<_>, _>>()?;
    let show = MenuItem::with_id(app, "show", "Open dashboard", true, None::<&str>)?;
    let quit = MenuItem::with_id(app, "quit", "Quit", true, None::<&str>)?;
    let mut builder = MenuBuilder::new(app);
    for item in &detail_items {
        builder = builder.item(item);
    }
    let menu = builder.separator().item(&show).item(&quit).build()?;
    Ok((menu, TrayMenuState { detail_items }))
}

fn status_details(message: &str) -> Vec<String> {
    vec![
        message.to_owned(),
        String::from("State · —"),
        String::from("CPU · —"),
        String::from("Memory · —"),
        String::from("Disk · —"),
        String::from("Temperature · —"),
        String::from("Path · — · RTT · —"),
    ]
}

/// Refresh the existing menu items in place. Returns `false` when the new
/// details do not fit the menu that was built, so the caller rebuilds instead
/// of silently dropping them.
///
/// Zipping the two slices would leave surplus items showing their pre-outage
/// text, so an agent outage or an unpaired machine would keep advertising the
/// last healthy CPU, memory and path values.
fn update_tray_menu<R: Runtime>(menu: &TrayMenuState<R>, details: &[String]) -> bool {
    if details.len() > menu.detail_items.len() {
        return false;
    }
    for (index, item) in menu.detail_items.iter().enumerate() {
        let text = details.get(index).map_or("—", String::as_str);
        let _ = item.set_text(text);
    }
    true
}

fn upsert_tray<R: Runtime, F>(
    app: &AppHandle<R>,
    id: &str,
    state: &str,
    title: &str,
    tooltip: &str,
    details: &[String],
    menu_builder: F,
) -> Result<(), String>
where
    F: FnOnce() -> Result<(Menu<R>, TrayMenuState<R>), String>,
{
    let color = tray_state_color(state)?;
    if let Some(tray) = app.tray_by_id(id) {
        tray.set_icon(Some(state_icon(color)))
            .map_err(|error| format!("Could not update the tray icon: {error}"))?;
        tray.set_title(Some(title.to_owned()))
            .map_err(|error| format!("Could not update the tray title: {error}"))?;
        tray.set_tooltip(Some(tooltip.to_owned()))
            .map_err(|error| format!("Could not update the tray tooltip: {error}"))?;
        #[cfg(target_os = "macos")]
        tray.set_icon_as_template(false)
            .map_err(|error| format!("Could not configure the tray icon: {error}"))?;
        let registry = app.state::<TrayRegistry<R>>();
        let existing_menu = registry
            .menus
            .lock()
            .ok()
            .and_then(|menus| menus.get(id).cloned());
        // Updating menu items in place keeps an open macOS NSMenu alive.
        // Replacing the menu causes AppKit to dismiss it on the next poll, so
        // only rebuild when the detail count no longer fits.
        let updated_in_place = existing_menu.is_some_and(|menu| update_tray_menu(&menu, details));
        if !updated_in_place {
            let (menu, menu_state) = menu_builder()?;
            tray.set_menu(Some(menu))
                .map_err(|error| format!("Could not update the tray menu: {error}"))?;
            if let Ok(mut menus) = registry.menus.lock() {
                menus.insert(id.to_owned(), menu_state);
            }
        }
        return Ok(());
    }

    let (menu, menu_state) = menu_builder()?;
    let builder = TrayIconBuilder::with_id(id)
        .icon(state_icon(color))
        .icon_as_template(false)
        .title(title)
        .tooltip(tooltip)
        .menu(&menu)
        .show_menu_on_left_click(true)
        .on_menu_event(|app, event| tray_event_handler(app, &event));
    builder
        .build(app)
        .map(|_| {
            if let Ok(mut menus) = app.state::<TrayRegistry<R>>().menus.lock() {
                menus.insert(id.to_owned(), menu_state);
            }
        })
        .map_err(|error| format!("Could not create tray icon {id}: {error}"))
}

fn update_status_tray(app: &AppHandle, state: &str, message: &str) {
    let registry = app.state::<TrayRegistry>();
    let ids = registry
        .machine_ids
        .lock()
        .map(|ids| ids.iter().cloned().collect::<Vec<_>>())
        .unwrap_or_default();
    if ids.is_empty() {
        let details = status_details(message);
        if upsert_tray(
            app,
            "rackio-status",
            state,
            message,
            message,
            &details,
            || {
                tray_menu(app, &details)
                    .map_err(|error| format!("Could not build tray menu: {error}"))
            },
        )
        .is_ok()
            && let Ok(mut ids) = registry.machine_ids.lock()
        {
            ids.insert(String::from("rackio-status"));
        }
        return;
    }

    // Keep existing status items alive on macOS. Removing an NSStatusItem while
    // AppKit is processing a daemon transition can block the event loop, so a
    // degraded state is rendered in-place and remains observable.
    for id in ids {
        let details = status_details(message);
        let _ = upsert_tray(app, &id, state, message, message, &details, || {
            tray_menu(app, &details).map_err(|error| format!("Could not build tray menu: {error}"))
        });
    }
}

fn update_machine_trays(app: &AppHandle, snapshot: &TrayFleetSnapshot) {
    let registry = app.state::<TrayRegistry>();
    let registered_ids = registry
        .machine_ids
        .lock()
        .map(|ids| ids.clone())
        .unwrap_or_default();
    let status_slot =
        registered_ids.contains("rackio-status") && app.tray_by_id("rackio-status").is_some();
    let fleet_title = fleet_tray_status(&snapshot.nodes);
    let fleet_details = fleet_tray_details(&snapshot.nodes);
    let mut active_ids = HashSet::new();
    for (index, node) in snapshot.nodes.iter().enumerate() {
        let id = if index == 0 && status_slot {
            String::from("rackio-status")
        } else {
            machine_tray_id(node)
        };
        let machine = tray_machine_menu(node);
        let is_primary_machine = index == 0;
        let title = if is_primary_machine {
            fleet_title.clone()
        } else {
            tray_node_status(node)
        };
        let details = if is_primary_machine {
            fleet_details.clone()
        } else {
            machine.details.clone()
        };
        let _ = upsert_tray(
            app,
            &id,
            &node.state,
            &title,
            if is_primary_machine {
                &fleet_title
            } else {
                &machine.title
            },
            &details,
            || {
                tray_menu(app, &details)
                    .map_err(|error| format!("Could not build tray menu: {error}"))
            },
        );
        active_ids.insert(id);
    }

    // This runs on Tauri's main thread (see `dispatch_tray_update`), which is
    // where AppKit requires status-item mutation. Retiring an item is a
    // response to a deliberate fleet change, not to a poll-to-poll wobble, so
    // it does not churn an open NSMenu the way a periodic rebuild would.
    for id in retired_tray_ids(&registered_ids, &active_ids) {
        app.remove_tray_by_id(&id);
        if let Ok(mut menus) = registry.menus.lock() {
            menus.remove(&id);
        }
    }
    if let Ok(mut ids) = registry.machine_ids.lock() {
        *ids = active_ids;
    }
}

/// Tray ids to drop because the machine left the fleet.
///
/// A connected agent reports every paired machine, including unreachable ones,
/// which keep their tray item and degrade in place through their own `state`
/// (and, when the agent itself is gone, through `update_status_tray`). An id
/// that disappears from a connected agent's snapshot therefore means explicit
/// removal — an unpaired machine. Relabelling it "Unavailable" and re-adding it
/// to the registry, as this once did, made a removal indistinguishable from an
/// outage and kept the item for the process lifetime.
fn retired_tray_ids(registered: &HashSet<String>, active: &HashSet<String>) -> Vec<String> {
    registered.difference(active).cloned().collect()
}

fn fleet_tray_status(nodes: &[TrayNodeSnapshot]) -> String {
    nodes
        .iter()
        .map(tray_node_status)
        .collect::<Vec<_>>()
        .join(" · ")
}

fn fleet_tray_details(nodes: &[TrayNodeSnapshot]) -> Vec<String> {
    nodes
        .iter()
        .flat_map(|node| {
            let machine = tray_machine_menu(node);
            std::iter::once(machine.title).chain(machine.details)
        })
        .collect()
}

fn tray_node_status(node: &TrayNodeSnapshot) -> String {
    // Keep the status item narrow enough that multiple machines remain visible
    // beside other menu-bar extras; the click menu carries the full metrics.
    format!("{} {}", node.name, tray_state_symbol(&node.state))
}

fn tray_machine_menu(node: &TrayNodeSnapshot) -> TrayMachineMenu {
    TrayMachineMenu {
        title: format!(
            "{} · {} · CPU {} · Memory {}",
            node.name,
            tray_state_label(&node.state),
            percentage_label(node.cpu_percent),
            memory_percentage_label(node)
        ),
        details: vec![
            format!("State · {}", tray_state_label(&node.state)),
            format!("CPU · {}", percentage_label(node.cpu_percent)),
            format!("Memory · {}", memory_percentage_label(node)),
            format!("Disk · {}", disk_percentage_label(node)),
            format!("Temperature · {}", temperature_label(node)),
            format!("Path · {}", tray_path_label(&node.path)),
            format!("RTT · {}", rtt_label(node.rtt_ms)),
        ],
    }
}

fn disk_percentage_label(node: &TrayNodeSnapshot) -> String {
    match (node.disk_used_bytes, node.disk_total_bytes) {
        (Some(used), Some(total)) if total > 0 => {
            let basis_points = used
                .saturating_mul(10_000)
                .checked_div(total)
                .unwrap_or_default()
                .min(10_000);
            let percentage = f64::from(u32::try_from(basis_points).unwrap_or(10_000)) / 100.0;
            format!("{percentage:.0}%")
        }
        _ => String::from("—"),
    }
}

/// The hottest sensor, named: an unattributed number would leave the operator
/// unable to tell a battery reading from a CPU package one. A machine with no
/// readable sensor shows an em dash rather than a plausible zero.
fn temperature_label(node: &TrayNodeSnapshot) -> String {
    node.temperature.as_ref().map_or_else(
        || String::from("—"),
        |temperature| format!("{:.0} °C · {}", temperature.celsius, temperature.label),
    )
}

fn tray_path_label(path: &str) -> &str {
    match path {
        "lan_direct" => "LAN direct",
        "wan_direct" => "WAN direct",
        "relayed" => "Relayed",
        _ => "Unknown path",
    }
}

fn rtt_label(rtt_ms: Option<u64>) -> String {
    rtt_ms.map_or_else(|| String::from("—"), |value| format!("{value} ms"))
}

fn tray_state_symbol(state: &str) -> &str {
    match state {
        "healthy" => "●",
        "warning" | "degraded" => "▲",
        "stale" => "◌",
        "critical" | "offline" | "auth_error" | "incompatible" => "✕",
        _ => "?",
    }
}

fn percentage_label(value: Option<f64>) -> String {
    value.map_or_else(|| String::from("—"), |value| format!("{value:.0}%"))
}

fn memory_percentage_label(node: &TrayNodeSnapshot) -> String {
    match (node.memory_used_bytes, node.memory_total_bytes) {
        (Some(used), Some(total)) if total > 0 => {
            let basis_points = used
                .saturating_mul(10_000)
                .checked_div(total)
                .unwrap_or_default()
                .min(10_000);
            let percentage = f64::from(u32::try_from(basis_points).unwrap_or(10_000)) / 100.0;
            format!("{percentage:.0}%")
        }
        _ => String::from("—"),
    }
}

fn tray_state_label(state: &str) -> &str {
    match state {
        "healthy" => "Healthy",
        "warning" => "Warning",
        "stale" => "Stale",
        "critical" => "Critical",
        "offline" => "Offline",
        "auth_error" => "Authentication error",
        "incompatible" => "Incompatible agent",
        "daemon_unavailable" => "Agent unavailable",
        _ => "Degraded",
    }
}

async fn update_tray_from_daemon(app: &AppHandle) {
    let Ok(value) = fleet_snapshot().await else {
        dispatch_tray_update(app, |app| {
            update_status_tray(app, "daemon_unavailable", "Agent unavailable");
        });
        return;
    };
    let Ok(snapshot) = serde_json::from_value::<TrayFleetSnapshot>(value) else {
        dispatch_tray_update(app, |app| {
            update_status_tray(app, "degraded", "Invalid agent snapshot");
        });
        return;
    };
    if snapshot.daemon == "connected" && !snapshot.nodes.is_empty() {
        dispatch_tray_update(app, move |app| {
            // AppKit status items must be mutated on Tauri's main thread;
            // keeping the IPC poll off-thread prevents multi-machine tray updates from deadlocking.
            update_machine_trays(app, &snapshot);
        });
    } else if snapshot.daemon == "connected" {
        dispatch_tray_update(app, |app| {
            update_status_tray(app, "warning", "No paired machines");
        });
    } else {
        dispatch_tray_update(app, |app| {
            update_status_tray(app, "degraded", "Invalid agent snapshot");
        });
    }
}

fn dispatch_tray_update<F>(app: &AppHandle, update: F)
where
    F: FnOnce(&AppHandle) + Send + 'static,
{
    let app_handle = app.clone();
    if let Err(error) = app.run_on_main_thread(move || update(&app_handle)) {
        tracing::error!(%error, "Could not dispatch tray update to the main thread");
    }
}

async fn run_tray_monitor(app: AppHandle) {
    let mut ticker = tokio::time::interval(Duration::from_secs(2));
    loop {
        ticker.tick().await;
        update_tray_from_daemon(&app).await;
    }
}

fn state_icon(color: [u8; 4]) -> Image<'static> {
    const SIZE: usize = 22;
    const SIZE_U32: u32 = 22;
    const CENTER: isize = 10;
    const RADIUS_SQUARED: isize = 49;
    let mut rgba = vec![0_u8; SIZE * SIZE * 4];
    for y in 0..SIZE {
        for x in 0..SIZE {
            let dx = isize::try_from(x).unwrap_or_default() - CENTER;
            let dy = isize::try_from(y).unwrap_or_default() - CENTER;
            if dx * dx + dy * dy <= RADIUS_SQUARED {
                let offset = (y * SIZE + x) * 4;
                rgba[offset..offset + 4].copy_from_slice(&color);
            }
        }
    }
    Image::new_owned(rgba, SIZE_U32, SIZE_U32)
}

/// Convert a daemon-side trend sample to the camelCase point shape the
/// dashboard shares with its 24-hour history query.
fn trend_point_json(sample: &serde_json::Value) -> serde_json::Value {
    serde_json::json!({
        "timestampMs": sample.get("timestamp_ms"),
        "cpuPercent": sample.get("cpu_percent"),
        "memoryUsedBytes": sample.get("memory_used_bytes"),
        "memoryTotalBytes": sample.get("memory_total_bytes"),
        "diskUsedBytes": sample.get("disk_used_bytes"),
        "diskTotalBytes": sample.get("disk_total_bytes"),
        "temperatureCelsius": sample.get("temperature_celsius"),
        "networkReceivedBytesPerSecond": sample.get("network_received_bytes_per_second"),
        "networkSentBytesPerSecond": sample.get("network_sent_bytes_per_second"),
        "rttMs": sample.get("rtt_ms"),
    })
}

fn machine_json(
    source: &serde_json::Value,
    node_state: &str,
    path: &str,
    rtt_ms: Option<u64>,
    last_seen_ms: Option<i64>,
    trend: &[serde_json::Value],
    detail: Option<&str>,
) -> Result<serde_json::Value, String> {
    let node = source
        .get("node")
        .ok_or_else(|| String::from("machine response did not contain node info"))?;
    let latest = source.get("latest").unwrap_or(&serde_json::Value::Null);
    let node_id = node
        .get("node_id")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| String::from("machine node ID is missing"))?;
    let node_name = node
        .get("display_name")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| String::from("machine node name is missing"))?;
    let disks = latest
        .get("disks")
        .and_then(serde_json::Value::as_array)
        .cloned()
        .unwrap_or_default();
    let worst_disk = disks
        .iter()
        .filter_map(|disk| {
            let used = disk.get("used_bytes").and_then(serde_json::Value::as_u64)?;
            let total = disk
                .get("total_bytes")
                .and_then(serde_json::Value::as_u64)?;
            (total > 0).then_some((used, total))
        })
        .max_by(|(left_used, left_total), (right_used, right_total)| {
            u128::from(*left_used)
                .saturating_mul(u128::from(*right_total))
                .cmp(&u128::from(*right_used).saturating_mul(u128::from(*left_total)))
        });
    let (disk_used, disk_total) = worst_disk.unzip();
    let cpu = latest
        .get("cpu_percent")
        .and_then(serde_json::Value::as_f64);
    // Absent on a host with no readable sensor. Carried through as an absent
    // field rather than as a zero so the viewer renders "—" instead of a
    // frozen machine.
    let temperature = latest
        .get("temperature")
        .filter(|value| !value.is_null())
        .map(|temperature| {
            serde_json::json!({
                "label": temperature.get("label"),
                "celsius": temperature.get("celsius"),
                "criticalCelsius": temperature.get("critical_celsius"),
                "sensorCount": temperature.get("sensor_count"),
            })
        });
    Ok(serde_json::json!({
        "id": node_id,
        "endpointId": source.get("endpoint_id"),
        "name": node_name,
        "os": format!(
            "{} · {}",
            node.get("os").and_then(serde_json::Value::as_str).unwrap_or("unknown"),
            node.get("architecture").and_then(serde_json::Value::as_str).unwrap_or("unknown")
        ),
        "state": node_state,
        "path": path,
        "cpuPercent": cpu,
        "memoryUsedBytes": latest.get("memory_used_bytes"),
        "memoryTotalBytes": latest.get("memory_total_bytes"),
        "diskUsedBytes": disk_used,
        "diskTotalBytes": disk_total,
        "temperature": temperature,
        "networkReceivedBytesPerSecond": latest
            .get("network")
            .and_then(|network| network.get("received_bytes_per_second")),
        "networkSentBytesPerSecond": latest
            .get("network")
            .and_then(|network| network.get("sent_bytes_per_second")),
        "rttMs": rtt_ms,
        "lastSeenMs": last_seen_ms,
        "trend": trend.iter().map(trend_point_json).collect::<Vec<_>>(),
        "detail": detail,
    }))
}

/// A wedged daemon — one that accepts the local connection but never answers —
/// must be reported as unavailable rather than leaving the last healthy
/// snapshot on screen. Both the React dashboard and the tray monitor poll every
/// two seconds and schedule the next poll only after the current request
/// settles, so the bound has to be shorter than two poll intervals (4s) for a
/// stall to surface before a second poll would have been due. Three seconds
/// still leaves room for a slow but live answer on a loaded machine.
const DAEMON_REQUEST_TIMEOUT: Duration = Duration::from_secs(3);

/// Bound every local IPC exchange. Returning an explicit timeout error is what
/// drives `update_tray_from_daemon` into its `daemon_unavailable` branch and
/// lets the React poll reschedule from its `finally`.
async fn daemon_request(
    command: serde_json::Value,
) -> Result<serde_json::Value, Box<dyn std::error::Error>> {
    bounded_daemon_exchange(daemon_exchange(command)).await
}

async fn bounded_daemon_exchange<F>(
    exchange: F,
) -> Result<serde_json::Value, Box<dyn std::error::Error>>
where
    F: Future<Output = Result<serde_json::Value, Box<dyn std::error::Error>>>,
{
    match tokio::time::timeout(DAEMON_REQUEST_TIMEOUT, exchange).await {
        Ok(response) => response,
        Err(_) => Err(format!(
            "daemon did not respond within {} seconds",
            DAEMON_REQUEST_TIMEOUT.as_secs()
        )
        .into()),
    }
}

#[cfg(unix)]
async fn daemon_exchange(
    command: serde_json::Value,
) -> Result<serde_json::Value, Box<dyn std::error::Error>> {
    use directories::ProjectDirs;
    use tokio::{
        io::{AsyncBufReadExt as _, AsyncWriteExt as _, BufReader},
        net::UnixStream,
    };

    let dirs = ProjectDirs::from("dev", "rackio", "rackio")
        .ok_or("OS application directories are unavailable")?;
    let state_dir = dirs.state_dir().unwrap_or_else(|| dirs.data_local_dir());
    let mut candidates = Vec::new();
    if let Some(path) = std::env::var_os("RACKIO_SOCKET") {
        candidates.push(std::path::PathBuf::from(path));
    }
    #[cfg(target_os = "linux")]
    candidates.push(std::path::PathBuf::from("/run/rackio/agent.sock"));
    #[cfg(target_os = "macos")]
    candidates.push(std::path::PathBuf::from(
        "/Library/Application Support/Rackio/run/agent.sock",
    ));
    candidates.push(state_dir.join("agent.sock"));
    let mut last_error = None;
    let mut connected = None;
    for candidate in candidates {
        match UnixStream::connect(&candidate).await {
            Ok(stream) => {
                connected = Some(stream);
                break;
            }
            Err(error) => last_error = Some((candidate, error)),
        }
    }
    let stream = connected.ok_or_else(|| match last_error {
        Some((path, error)) => format!("daemon is not reachable at {}: {error}", path.display()),
        None => String::from("no local daemon socket was configured"),
    })?;
    let (read, mut write) = stream.into_split();
    let mut request = serde_json::to_vec(&command)?;
    request.push(b'\n');
    write.write_all(&request).await?;
    let mut lines = BufReader::new(read).lines();
    let line = lines
        .next_line()
        .await?
        .ok_or("daemon closed the local connection without a response")?;
    let response: serde_json::Value = serde_json::from_str(&line)?;
    if !response
        .get("ok")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false)
    {
        return Err(response
            .get("error")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("daemon request failed")
            .to_owned()
            .into());
    }
    Ok(response)
}

#[cfg(windows)]
async fn daemon_exchange(
    command: serde_json::Value,
) -> Result<serde_json::Value, Box<dyn std::error::Error>> {
    use tokio::io::{AsyncBufReadExt as _, AsyncWriteExt as _, BufReader};

    let pipe_name = rackio_windows_ipc::configured_pipe_name()?;
    let stream = rackio_windows_ipc::connect_client(&pipe_name).await?;
    let (read, mut write) = tokio::io::split(stream);
    let mut request = serde_json::to_vec(&command)?;
    request.push(b'\n');
    write.write_all(&request).await?;
    let mut lines = BufReader::new(read).lines();
    let line = lines
        .next_line()
        .await?
        .ok_or("daemon closed the local connection without a response")?;
    let response: serde_json::Value = serde_json::from_str(&line)?;
    if !response
        .get("ok")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false)
    {
        return Err(response
            .get("error")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("daemon request failed")
            .to_owned()
            .into());
    }
    Ok(response)
}

#[cfg(not(any(unix, windows)))]
async fn daemon_exchange(
    _command: serde_json::Value,
) -> Result<serde_json::Value, Box<dyn std::error::Error>> {
    Err("local daemon IPC is unsupported on this platform".into())
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_notification::init())
        .plugin(tauri_plugin_dialog::init())
        .setup(|app| {
            app.manage(TrayRegistry::<tauri::Wry>::default());
            #[cfg(target_os = "macos")]
            {
                // The desktop window is a secondary viewer; the primary surface
                // is the set of status items, so keep the app in the menu bar.
                app.set_activation_policy(tauri::ActivationPolicy::Accessory);
            }
            // The first poll chooses either the machine tabs or a degraded
            // status item. Avoid creating a temporary status item here: on
            // macOS replacing it immediately after startup can block AppKit's
            // status bar before the machine tabs become visible.
            tauri::async_runtime::spawn(run_tray_monitor(app.handle().clone()));
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            fleet_snapshot,
            pair_machine,
            create_pairing_share,
            save_pairing_bundle,
            machine_history,
            ssh_bootstrap::ssh_inspect_host,
            ssh_bootstrap::ssh_bootstrap
        ])
        .run(tauri::generate_context!())
        .unwrap_or_else(|error| panic!("failed to run Rackio desktop: {error}"));
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use super::{
        DAEMON_REQUEST_TIMEOUT, TrayMachineMenu, TrayNodeSnapshot, TrayTemperature,
        bounded_daemon_exchange, fleet_tray_status, history_point_from_sample, machine_json,
        machine_tray_id, pairing_bundle_expiry, pairing_qr_data_url, retired_tray_ids,
        save_pairing_bundle, temperature_label, tray_machine_menu, tray_node_status,
        tray_state_color,
    };

    fn ids(values: &[&str]) -> HashSet<String> {
        values.iter().map(|value| (*value).to_owned()).collect()
    }

    /// A daemon that accepts the connection and then never answers must not
    /// leave the caller pending, which would freeze both the React poll loop
    /// and the tray monitor on the last healthy snapshot.
    #[tokio::test]
    async fn a_wedged_daemon_fails_with_an_explicit_timeout() {
        tokio::time::pause();
        let result = bounded_daemon_exchange(std::future::pending()).await;
        let message = result
            .err()
            .map(|error| error.to_string())
            .unwrap_or_default();
        assert_eq!(message, "daemon did not respond within 3 seconds");
    }

    #[test]
    fn the_ipc_timeout_stays_below_two_dashboard_poll_intervals() {
        assert!(DAEMON_REQUEST_TIMEOUT < std::time::Duration::from_secs(4));
    }

    #[tokio::test]
    async fn a_responsive_daemon_is_not_cut_short() {
        let response = bounded_daemon_exchange(async { Ok(serde_json::json!({ "ok": true })) })
            .await
            .unwrap_or_else(|error| panic!("{error}"));
        assert_eq!(response, serde_json::json!({ "ok": true }));
    }

    #[test]
    fn a_machine_removed_from_the_fleet_loses_its_tray_item() {
        let registered = ids(&["rackio-status", "machine-a", "machine-b"]);
        let active = ids(&["rackio-status", "machine-a"]);
        assert_eq!(
            retired_tray_ids(&registered, &active),
            vec![String::from("machine-b")]
        );
        // An unreachable machine still appears in the snapshot, so it keeps its
        // item and degrades in place rather than disappearing silently.
        assert!(retired_tray_ids(&registered, &registered).is_empty());
    }

    #[test]
    fn pairing_expiry_is_read_from_the_bundle_and_fails_closed() {
        let payload = base64::Engine::encode(
            &base64::engine::general_purpose::URL_SAFE_NO_PAD,
            serde_json::to_vec(&serde_json::json!({
                "format_version": 1,
                "expires_at_ms": 1_750_000_300_000_i64,
            }))
            .unwrap_or_else(|error| panic!("{error}")),
        );
        assert_eq!(
            pairing_bundle_expiry(&format!("rackio-pair:{payload}")),
            Ok(1_750_000_300_000)
        );
        assert!(pairing_bundle_expiry("not-a-bundle").is_err());
        assert!(pairing_bundle_expiry("rackio-pair:!!!").is_err());
        let without_expiry = base64::Engine::encode(
            &base64::engine::general_purpose::URL_SAFE_NO_PAD,
            b"{\"format_version\":1}",
        );
        assert!(pairing_bundle_expiry(&format!("rackio-pair:{without_expiry}")).is_err());
    }

    #[test]
    fn pairing_qr_is_generated_locally_as_an_svg_data_url() {
        let result = pairing_qr_data_url("rackio-pair:test-bundle")
            .unwrap_or_else(|error| panic!("{error}"));
        assert!(result.starts_with("data:image/svg+xml;base64,"));
    }

    #[test]
    fn an_oversized_bundle_does_not_produce_a_misleading_qr_code() {
        let result = pairing_qr_data_url(&"x".repeat(10_000));
        assert!(result.is_err());
    }

    #[test]
    fn pairing_bundle_export_is_private_and_round_trips() {
        let directory = tempfile::tempdir().unwrap_or_else(|error| panic!("{error}"));
        let path = directory.path().join("pairing.txt");
        save_pairing_bundle(path.clone(), String::from("rackio-pair:test"))
            .unwrap_or_else(|error| panic!("{error}"));
        let saved = std::fs::read_to_string(&path).unwrap_or_else(|error| panic!("{error}"));
        assert_eq!(saved, "rackio-pair:test\n");

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(path)
                .unwrap_or_else(|error| panic!("{error}"))
                .permissions()
                .mode();
            assert_eq!(mode & 0o777, 0o600);
        }
    }

    #[test]
    fn invalid_pairing_bundle_is_not_written() {
        let directory = tempfile::tempdir().unwrap_or_else(|error| panic!("{error}"));
        let path = directory.path().join("pairing.txt");
        let result = save_pairing_bundle(path.clone(), String::from("not-a-pairing-bundle"));
        assert!(result.is_err());
        assert!(!path.exists());
    }

    /// The daemon speaks `snake_case` and the viewer `camelCase`, so this boundary
    /// is where a temperature would silently disappear — or, worse, arrive as a
    /// zero on a machine that never reported one.
    #[test]
    fn a_machine_carries_its_hottest_sensor_to_the_viewer_or_nothing_at_all() {
        let source = serde_json::json!({
            "node": {
                "node_id": "id",
                "display_name": "Server",
                "os": "linux",
                "architecture": "x86_64",
            },
            "latest": {
                "cpu_percent": 12.0,
                "temperature": {
                    "label": "Package id 0",
                    "celsius": 61.5,
                    "critical_celsius": 100.0,
                    "sensor_count": 7,
                },
            },
        });

        let machine = machine_json(&source, "healthy", "lan_direct", None, None, &[], None)
            .unwrap_or_else(|error| panic!("{error}"));
        assert_eq!(
            machine.get("temperature"),
            Some(&serde_json::json!({
                "label": "Package id 0",
                "celsius": 61.5,
                "criticalCelsius": 100.0,
                "sensorCount": 7,
            }))
        );

        // A host with no readable sensor must not acquire one here.
        let sensorless = serde_json::json!({
            "node": { "node_id": "id", "display_name": "Server" },
            "latest": { "cpu_percent": 12.0 },
        });
        let machine = machine_json(&sensorless, "healthy", "lan_direct", None, None, &[], None)
            .unwrap_or_else(|error| panic!("{error}"));
        assert_eq!(machine.get("temperature"), Some(&serde_json::Value::Null));
    }

    #[test]
    fn trend_samples_reach_the_viewer_as_timestamped_points() {
        let source = serde_json::json!({
            "node": { "node_id": "id", "display_name": "Server" },
            "latest": { "cpu_percent": 12.0 },
        });
        let trend = [serde_json::json!({
            "timestamp_ms": 1_750_000_000_000_i64,
            "cpu_percent": 12.5,
            "memory_used_bytes": 3_000,
            "memory_total_bytes": 4_000,
            "disk_used_bytes": 90,
            "disk_total_bytes": 100,
            "temperature_celsius": 61.5,
            "network_received_bytes_per_second": 2_048,
            "network_sent_bytes_per_second": 512,
            "rtt_ms": 8,
        })];

        let machine = machine_json(&source, "healthy", "lan_direct", None, None, &trend, None)
            .unwrap_or_else(|error| panic!("{error}"));
        assert_eq!(
            machine.get("trend"),
            Some(&serde_json::json!([{
                "timestampMs": 1_750_000_000_000_i64,
                "cpuPercent": 12.5,
                "memoryUsedBytes": 3_000,
                "memoryTotalBytes": 4_000,
                "diskUsedBytes": 90,
                "diskTotalBytes": 100,
                "temperatureCelsius": 61.5,
                "networkReceivedBytesPerSecond": 2_048,
                "networkSentBytesPerSecond": 512,
                "rttMs": 8,
            }]))
        );
    }

    #[test]
    fn each_machine_gets_a_distinct_tray_tab_and_detail_menu() {
        let node = TrayNodeSnapshot {
            id: String::from("server-id"),
            name: String::from("Server"),
            state: String::from("warning"),
            path: String::from("relayed"),
            cpu_percent: Some(60.0),
            memory_used_bytes: Some(30),
            memory_total_bytes: Some(100),
            disk_used_bytes: Some(45),
            disk_total_bytes: Some(100),
            temperature: Some(TrayTemperature {
                label: String::from("CPU die"),
                celsius: 72.4,
            }),
            rtt_ms: None,
        };

        assert_eq!(machine_tray_id(&node), "machine-server-id");
        assert_eq!(tray_node_status(&node), "Server ▲");
        assert_eq!(
            tray_machine_menu(&node),
            TrayMachineMenu {
                title: String::from("Server · Warning · CPU 60% · Memory 30%"),
                details: vec![
                    String::from("State · Warning"),
                    String::from("CPU · 60%"),
                    String::from("Memory · 30%"),
                    String::from("Disk · 45%"),
                    String::from("Temperature · 72 °C · CPU die"),
                    String::from("Path · Relayed"),
                    String::from("RTT · —"),
                ],
            }
        );

        let steamdeck = TrayNodeSnapshot {
            id: String::from("steamdeck-id"),
            name: String::from("steamdeck"),
            state: String::from("healthy"),
            path: String::from("lan_direct"),
            cpu_percent: None,
            memory_used_bytes: None,
            memory_total_bytes: None,
            disk_used_bytes: None,
            disk_total_bytes: None,
            // A machine with no readable sensor: the menu must show an em dash
            // rather than a plausible zero.
            temperature: None,
            rtt_ms: Some(8),
        };
        assert_eq!(temperature_label(&steamdeck), "—");
        assert_eq!(
            fleet_tray_status(&[node, steamdeck]),
            "Server ▲ · steamdeck ●"
        );
    }

    #[test]
    fn tray_state_color_fails_closed_for_unknown_states() {
        assert!(tray_state_color("unknown").is_err());
        assert_eq!(
            tray_state_color("offline").unwrap_or_default(),
            [255, 111, 103, 255]
        );
    }

    #[test]
    fn history_point_carries_the_minute_averaged_disk_and_temperature() {
        let sample = serde_json::json!({
            "timestamp_ms": 60_000,
            "cpu_percent": 20.0,
            "memory_used_bytes": 100,
            "memory_total_bytes": 200,
            "disks": [{"mount": "(minute average)", "used_bytes": 25, "total_bytes": 100}],
            "temperature": {"label": "(minute average)", "celsius": 50.0, "sensor_count": 0},
            "network": {"received_bytes_per_second": 10, "sent_bytes_per_second": 20},
        });

        let point = history_point_from_sample(&sample);
        assert_eq!(point["timestampMs"], 60_000);
        assert_eq!(point["diskUsedBytes"], 25);
        assert_eq!(point["diskTotalBytes"], 100);
        assert_eq!(point["temperatureCelsius"], 50.0);
        assert_eq!(point["networkReceivedBytesPerSecond"], 10);
    }

    #[test]
    fn history_point_reports_no_disk_or_temperature_when_the_minute_had_neither() {
        let sample = serde_json::json!({
            "timestamp_ms": 60_000,
            "cpu_percent": 20.0,
            "memory_used_bytes": 100,
            "memory_total_bytes": 200,
            "disks": [],
        });

        let point = history_point_from_sample(&sample);
        assert!(point["diskUsedBytes"].is_null());
        assert!(point["diskTotalBytes"].is_null());
        assert!(point["temperatureCelsius"].is_null());
    }
}
