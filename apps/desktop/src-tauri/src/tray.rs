//! Tray state, menus, main-thread updates, and daemon polling.

use crate::fleet_snapshot;
use serde::Deserialize;
use std::{
    collections::{HashMap, HashSet},
    sync::Mutex,
    time::Duration,
};
use tauri::{
    AppHandle, Manager, Runtime,
    image::Image,
    menu::{Menu, MenuBuilder, MenuItem},
    tray::TrayIconBuilder,
};

pub(crate) struct TrayRegistry<R: Runtime = tauri::Wry> {
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

pub(crate) async fn run_tray_monitor(app: AppHandle) {
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

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use super::{
        TrayMachineMenu, TrayNodeSnapshot, TrayTemperature, fleet_tray_status, machine_tray_id,
        retired_tray_ids, temperature_label, tray_machine_menu, tray_node_status, tray_state_color,
    };

    fn ids(values: &[&str]) -> HashSet<String> {
        values.iter().map(|value| (*value).to_owned()).collect()
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
}
