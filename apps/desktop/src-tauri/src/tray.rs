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
    menu::{IconMenuItem, Menu, MenuBuilder, MenuItem, Submenu},
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

struct TraySectionItems<R: Runtime> {
    submenu: Submenu<R>,
    gauges: Vec<IconMenuItem<R>>,
    link: MenuItem<R>,
}

impl<R: Runtime> Clone for TraySectionItems<R> {
    fn clone(&self) -> Self {
        Self {
            submenu: self.submenu.clone(),
            gauges: self.gauges.clone(),
            link: self.link.clone(),
        }
    }
}

struct TrayMenuState<R: Runtime> {
    sections: Vec<TraySectionItems<R>>,
}

impl<R: Runtime> Clone for TrayMenuState<R> {
    fn clone(&self) -> Self {
        Self {
            sections: self.sections.clone(),
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

/// One metric row inside a machine's hover submenu: a text label and, when the
/// reading exists, a horizontal bar gauge rendered as the row's icon. The fill
/// is stored in basis points (0..=10 000) so the model stays `Eq`-comparable in
/// tests; the pixel rendering happens only at menu-build time.
#[derive(Debug, Clone, PartialEq, Eq)]
struct TrayGauge {
    text: String,
    fill_basis_points: Option<u16>,
}

/// One machine's entry in the click menu: a hover submenu (`▸`) whose title
/// carries the at-a-glance identity (`● Mac — Healthy`) and whose rows carry
/// the bar-gauge metrics. The top-level menu therefore stays one row per
/// machine no matter how large the fleet is.
#[derive(Debug, Clone, PartialEq, Eq)]
struct TraySection {
    header: String,
    /// Gauge fills reuse the machine's state colour rather than inventing
    /// per-metric warning thresholds here: severity is owned by the
    /// user-configurable alert rules (rackio-core), which already drive
    /// `state`. A second, hard-coded threshold set in the tray would drift
    /// from — and contradict — what the user configured.
    color: [u8; 4],
    gauges: Vec<TrayGauge>,
    link: String,
}

#[derive(Debug, PartialEq, Eq)]
struct TrayMachineMenu {
    title: String,
    section: TraySection,
}

fn machine_tray_id(node: &TrayNodeSnapshot) -> String {
    format!("machine-{}", node.id)
}

/// Neutral fill for sections that have no live machine behind them (daemon
/// fallback, unknown state). Matches the `stale` grey.
const NEUTRAL_COLOR: [u8; 4] = [164, 173, 168, 255];

fn tray_state_color(state: &str) -> Result<[u8; 4], String> {
    match state {
        "healthy" => Ok([84, 217, 139, 255]),
        "warning" | "degraded" => Ok([230, 189, 89, 255]),
        "stale" => Ok(NEUTRAL_COLOR),
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
    sections: &[TraySection],
) -> Result<(Menu<R>, TrayMenuState<R>), tauri::Error> {
    let mut builder = MenuBuilder::new(app);
    let mut section_items = Vec::with_capacity(sections.len());
    for (section_index, section) in sections.iter().enumerate() {
        let submenu = Submenu::with_id(
            app,
            format!("machine-section-{section_index}"),
            &section.header,
            true,
        )?;
        let mut gauge_items = Vec::with_capacity(section.gauges.len());
        for (gauge_index, gauge) in section.gauges.iter().enumerate() {
            let item = IconMenuItem::with_id(
                app,
                format!("machine-gauge-{section_index}-{gauge_index}"),
                &gauge.text,
                false,
                gauge_image(gauge, section.color),
                None::<&str>,
            )?;
            submenu.append(&item)?;
            gauge_items.push(item);
        }
        let link = MenuItem::with_id(
            app,
            format!("machine-link-{section_index}"),
            &section.link,
            false,
            None::<&str>,
        )?;
        submenu.append(&link)?;
        builder = builder.item(&submenu);
        section_items.push(TraySectionItems {
            submenu,
            gauges: gauge_items,
            link,
        });
    }
    let show = MenuItem::with_id(app, "show", "Open dashboard", true, None::<&str>)?;
    let quit = MenuItem::with_id(app, "quit", "Quit", true, None::<&str>)?;
    let menu = builder.separator().item(&show).item(&quit).build()?;
    Ok((
        menu,
        TrayMenuState {
            sections: section_items,
        },
    ))
}

fn status_section(message: &str) -> TraySection {
    TraySection {
        header: message.to_owned(),
        color: NEUTRAL_COLOR,
        gauges: vec![
            TrayGauge {
                text: String::from("CPU —"),
                fill_basis_points: None,
            },
            TrayGauge {
                text: String::from("Memory —"),
                fill_basis_points: None,
            },
            TrayGauge {
                text: String::from("Disk —"),
                fill_basis_points: None,
            },
            TrayGauge {
                text: String::from("Temperature —"),
                fill_basis_points: None,
            },
        ],
        link: String::from("—"),
    }
}

/// Refresh the existing menu items in place. Returns `false` when the new
/// sections do not match the shape of the menu that was built, so the caller
/// rebuilds instead of writing one machine's metrics onto another's rows.
fn update_tray_menu<R: Runtime>(menu: &TrayMenuState<R>, sections: &[TraySection]) -> bool {
    if menu.sections.len() != sections.len()
        || menu
            .sections
            .iter()
            .zip(sections)
            .any(|(items, section)| items.gauges.len() != section.gauges.len())
    {
        return false;
    }
    for (items, section) in menu.sections.iter().zip(sections) {
        let _ = items.submenu.set_text(&section.header);
        for (item, gauge) in items.gauges.iter().zip(&section.gauges) {
            let _ = item.set_text(&gauge.text);
            let _ = item.set_icon(gauge_image(gauge, section.color));
        }
        let _ = items.link.set_text(&section.link);
    }
    true
}

fn upsert_tray<R: Runtime, F>(
    app: &AppHandle<R>,
    id: &str,
    state: &str,
    title: &str,
    tooltip: &str,
    sections: &[TraySection],
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
        // only rebuild when the section shape no longer matches.
        let updated_in_place = existing_menu.is_some_and(|menu| update_tray_menu(&menu, sections));
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
        let sections = vec![status_section(message)];
        if upsert_tray(
            app,
            "rackio-status",
            state,
            message,
            message,
            &sections,
            || {
                tray_menu(app, &sections)
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
        let sections = vec![status_section(message)];
        let _ = upsert_tray(app, &id, state, message, message, &sections, || {
            tray_menu(app, &sections).map_err(|error| format!("Could not build tray menu: {error}"))
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
    let fleet_sections = fleet_tray_sections(&snapshot.nodes);
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
        let sections = if is_primary_machine {
            fleet_sections.clone()
        } else {
            vec![machine.section.clone()]
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
            &sections,
            || {
                tray_menu(app, &sections)
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

fn fleet_tray_sections(nodes: &[TrayNodeSnapshot]) -> Vec<TraySection> {
    nodes
        .iter()
        .map(|node| tray_machine_menu(node).section)
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
        section: TraySection {
            header: format!(
                "{} {} — {}",
                tray_state_symbol(&node.state),
                node.name,
                tray_state_label(&node.state)
            ),
            // An unknown state fails closed in `upsert_tray` before this
            // section ever renders, so the fallback colour is unreachable in
            // practice and only keeps this constructor infallible.
            color: tray_state_color(&node.state).unwrap_or(NEUTRAL_COLOR),
            gauges: vec![
                TrayGauge {
                    text: format!("CPU {}", percentage_label(node.cpu_percent)),
                    fill_basis_points: percent_basis_points(node.cpu_percent),
                },
                TrayGauge {
                    text: format!("Memory {}", memory_percentage_label(node)),
                    fill_basis_points: ratio_basis_points(
                        node.memory_used_bytes,
                        node.memory_total_bytes,
                    ),
                },
                TrayGauge {
                    text: format!("Disk {}", disk_percentage_label(node)),
                    fill_basis_points: ratio_basis_points(
                        node.disk_used_bytes,
                        node.disk_total_bytes,
                    ),
                },
                temperature_gauge(node),
            ],
            link: link_label(node),
        },
    }
}

/// The hottest sensor, named: an unattributed number would leave the operator
/// unable to tell a battery reading from a CPU package one. The gauge fill
/// maps 0–100 °C onto the bar, matching the dashboard's temperature axis; a
/// machine with no readable sensor shows an em dash and no bar rather than a
/// plausible zero.
fn temperature_gauge(node: &TrayNodeSnapshot) -> TrayGauge {
    node.temperature.as_ref().map_or_else(
        || TrayGauge {
            text: String::from("Temperature —"),
            fill_basis_points: None,
        },
        |temperature| TrayGauge {
            text: format!(
                "Temperature {:.0} °C {}",
                temperature.celsius, temperature.label
            ),
            fill_basis_points: Some(scale_basis_points(temperature.celsius)),
        },
    )
}

/// Network path and RTT for the submenu's connectivity row, omitting the
/// readings a machine simply does not have; a machine with nothing to report
/// keeps an em-dash row so the section shape stays stable across polls (a
/// shape change forces a menu rebuild, which dismisses an open `NSMenu`).
fn link_label(node: &TrayNodeSnapshot) -> String {
    let mut parts = Vec::new();
    if let Some(path) = known_path_label(&node.path) {
        parts.push(path.to_owned());
    }
    if let Some(rtt) = node.rtt_ms {
        parts.push(format!("RTT {rtt} ms"));
    }
    if parts.is_empty() {
        String::from("—")
    } else {
        parts.join(" · ")
    }
}

/// An unknown path (typically the local machine, which has no network path to
/// itself) is omitted rather than labelled: health problems surface through
/// `state`, not through the path field.
fn known_path_label(path: &str) -> Option<&'static str> {
    match path {
        "lan_direct" => Some("LAN direct"),
        "wan_direct" => Some("WAN direct"),
        "relayed" => Some("Relayed"),
        _ => None,
    }
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

fn percent_basis_points(value: Option<f64>) -> Option<u16> {
    value.map(scale_basis_points)
}

/// Maps a 0–100 scale reading (a percentage, or degrees Celsius for the
/// temperature gauge) onto gauge basis points.
// The clamp bounds the value to 0..=10 000 before the cast, so it can neither
// truncate nor go negative; a NaN clamps to the lower bound and yields 0.
#[expect(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
fn scale_basis_points(value: f64) -> u16 {
    (value.clamp(0.0, 100.0) * 100.0).round() as u16
}

fn disk_percentage_label(node: &TrayNodeSnapshot) -> String {
    basis_points_label(ratio_basis_points(
        node.disk_used_bytes,
        node.disk_total_bytes,
    ))
}

fn memory_percentage_label(node: &TrayNodeSnapshot) -> String {
    basis_points_label(ratio_basis_points(
        node.memory_used_bytes,
        node.memory_total_bytes,
    ))
}

fn ratio_basis_points(used: Option<u64>, total: Option<u64>) -> Option<u16> {
    match (used, total) {
        (Some(used), Some(total)) if total > 0 => {
            let basis_points = used
                .saturating_mul(10_000)
                .checked_div(total)
                .unwrap_or_default()
                .min(10_000);
            Some(u16::try_from(basis_points).unwrap_or(10_000))
        }
        _ => None,
    }
}

fn basis_points_label(basis_points: Option<u16>) -> String {
    basis_points.map_or_else(
        || String::from("—"),
        |basis_points| format!("{:.0}%", f64::from(basis_points) / 100.0),
    )
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

const GAUGE_WIDTH: usize = 56;
const GAUGE_HEIGHT: usize = 10;

fn gauge_fill_width(basis_points: u16) -> usize {
    usize::from(basis_points.min(10_000)) * GAUGE_WIDTH / 10_000
}

fn gauge_image(gauge: &TrayGauge, color: [u8; 4]) -> Option<Image<'static>> {
    gauge
        .fill_basis_points
        .map(|basis_points| gauge_icon(basis_points, color))
}

/// A horizontal bar gauge rendered as a menu-item icon: a translucent grey
/// track with the filled portion in the machine's state colour, so the metric
/// magnitudes can be compared at a glance across rows and machines.
fn gauge_icon(basis_points: u16, color: [u8; 4]) -> Image<'static> {
    const TRACK: [u8; 4] = [127, 127, 127, 56];
    let fill_width = gauge_fill_width(basis_points);
    let mut rgba = vec![0_u8; GAUGE_WIDTH * GAUGE_HEIGHT * 4];
    for y in 0..GAUGE_HEIGHT {
        for x in 0..GAUGE_WIDTH {
            let pixel = if x < fill_width { color } else { TRACK };
            let offset = (y * GAUGE_WIDTH + x) * 4;
            rgba[offset..offset + 4].copy_from_slice(&pixel);
        }
    }
    Image::new_owned(
        rgba,
        u32::try_from(GAUGE_WIDTH).unwrap_or_default(),
        u32::try_from(GAUGE_HEIGHT).unwrap_or_default(),
    )
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use super::{
        GAUGE_WIDTH, TrayGauge, TrayMachineMenu, TrayNodeSnapshot, TraySection, TrayTemperature,
        fleet_tray_status, gauge_fill_width, link_label, machine_tray_id, retired_tray_ids,
        status_section, tray_machine_menu, tray_node_status, tray_state_color,
    };

    fn ids(values: &[&str]) -> HashSet<String> {
        values.iter().map(|value| (*value).to_owned()).collect()
    }

    fn gauge(text: &str, fill_basis_points: Option<u16>) -> TrayGauge {
        TrayGauge {
            text: text.to_owned(),
            fill_basis_points,
        }
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
    fn each_machine_gets_a_distinct_tray_tab_and_a_gauge_submenu() {
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
                section: TraySection {
                    header: String::from("▲ Server — Warning"),
                    // The gauge colour tracks the machine's state (owned by the
                    // user's alert rules), not a tray-local threshold set.
                    color: [230, 189, 89, 255],
                    gauges: vec![
                        gauge("CPU 60%", Some(6_000)),
                        gauge("Memory 30%", Some(3_000)),
                        gauge("Disk 45%", Some(4_500)),
                        gauge("Temperature 72 °C CPU die", Some(7_240)),
                    ],
                    link: String::from("Relayed"),
                },
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
            temperature: None,
            rtt_ms: Some(8),
        };
        assert_eq!(link_label(&steamdeck), "LAN direct · RTT 8 ms");
        // Missing readings render an em dash and no bar, never a plausible
        // zero-length gauge.
        assert_eq!(
            tray_machine_menu(&steamdeck).section.gauges,
            vec![
                gauge("CPU —", None),
                gauge("Memory —", None),
                gauge("Disk —", None),
                gauge("Temperature —", None),
            ]
        );
        assert_eq!(
            fleet_tray_status(&[node, steamdeck]),
            "Server ▲ · steamdeck ●"
        );
    }

    #[test]
    fn a_machine_with_no_connectivity_readings_keeps_a_stable_placeholder_row() {
        // The local machine has no network path to itself; the link row must
        // stay present (section shape stability keeps an open NSMenu alive)
        // but collapse to a single em dash.
        let local = TrayNodeSnapshot {
            id: String::from("local-id"),
            name: String::from("Mac"),
            state: String::from("healthy"),
            path: String::from("unknown"),
            cpu_percent: Some(51.0),
            memory_used_bytes: Some(88),
            memory_total_bytes: Some(100),
            disk_used_bytes: Some(94),
            disk_total_bytes: Some(100),
            temperature: None,
            rtt_ms: None,
        };
        assert_eq!(link_label(&local), "—");
        assert_eq!(tray_machine_menu(&local).section.header, "● Mac — Healthy");
    }

    #[test]
    fn a_degraded_daemon_renders_a_status_section_with_the_same_shape() {
        // The status fallback keeps the four-gauge shape of a machine section
        // so a daemon outage updates the open menu in place instead of
        // rebuilding (and dismissing) it.
        let section = status_section("Agent unavailable");
        assert_eq!(section.header, "Agent unavailable");
        assert_eq!(section.gauges.len(), 4);
        assert!(
            section
                .gauges
                .iter()
                .all(|gauge| gauge.fill_basis_points.is_none())
        );
        assert_eq!(section.link, "—");
    }

    #[test]
    fn gauge_fill_spans_the_bar_proportionally_and_saturates() {
        assert_eq!(gauge_fill_width(0), 0);
        assert_eq!(gauge_fill_width(5_000), GAUGE_WIDTH / 2);
        assert_eq!(gauge_fill_width(10_000), GAUGE_WIDTH);
        // An over-range reading must not overrun the icon buffer.
        assert_eq!(gauge_fill_width(u16::MAX), GAUGE_WIDTH);
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
