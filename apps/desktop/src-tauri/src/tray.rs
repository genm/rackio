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
    /// The live trend window the daemon already ships with every snapshot for
    /// the dashboard; the tray reuses it for the submenu sparklines instead of
    /// issuing a second history query.
    #[serde(default)]
    trend: Vec<TrayTrendPoint>,
}

#[derive(Debug, Deserialize)]
struct TrayTrendPoint {
    #[serde(rename = "timestampMs")]
    timestamp_ms: i64,
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
    #[serde(rename = "temperatureCelsius")]
    temperature_celsius: Option<f64>,
}

#[derive(Debug, Deserialize)]
struct TrayTemperature {
    label: String,
    celsius: f64,
}

/// One metric row inside a machine's hover submenu: a text label and, when
/// readings exist, a time-series area chart rendered as the row's icon. The
/// chart uses a fixed 0–100 scale, so its right edge doubles as a bar gauge of
/// the current value while the body shows the recent history. Values are
/// modelled in basis points (0..=10 000) so the menu model stays
/// `Eq`-comparable in tests; pixel rendering happens only at menu-build time.
#[derive(Debug, Clone, PartialEq, Eq)]
struct TrayGauge {
    text: String,
    /// Current reading, used as a flat fallback when the trend carries fewer
    /// than two points for this metric.
    fill_basis_points: Option<u16>,
    /// `(timestamp_ms, basis_points)` history, chronological. Positions on the
    /// chart's x-axis come from these timestamps — the sampling cadence is the
    /// data's to declare, not the tray's to assume.
    series: Vec<(i64, u16)>,
}

/// One machine's entry in the click menu: a hover submenu (`▸`) whose title
/// carries the minimal at-a-glance row (`● Mac — Healthy · CPU 51% · Mem 88%`)
/// and whose rows carry the charted metrics. The top-level menu therefore
/// stays one row per machine no matter how large the fleet is, without hiding
/// the numbers an operator glances at most.
#[derive(Debug, Clone, PartialEq, Eq)]
struct TraySection {
    header: String,
    /// Chart fills reuse the machine's state colour rather than inventing
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

fn empty_gauge(label: &str) -> TrayGauge {
    TrayGauge {
        text: format!("{label} —"),
        fill_basis_points: None,
        series: Vec::new(),
    }
}

fn status_section(message: &str) -> TraySection {
    TraySection {
        header: message.to_owned(),
        color: NEUTRAL_COLOR,
        gauges: vec![
            empty_gauge("CPU"),
            empty_gauge("Memory"),
            empty_gauge("Disk"),
            empty_gauge("Temperature"),
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
    let memory_label = memory_percentage_label(node);
    TrayMachineMenu {
        title: format!(
            "{} · {} · CPU {} · Memory {}",
            node.name,
            tray_state_label(&node.state),
            percentage_label(node.cpu_percent),
            memory_label
        ),
        section: TraySection {
            // The top-level row carries the minimal glance metrics itself;
            // hiding every number one hover away made the first level useless.
            header: format!(
                "{} {} — {} · CPU {} · Mem {}",
                tray_state_symbol(&node.state),
                node.name,
                tray_state_label(&node.state),
                percentage_label(node.cpu_percent),
                memory_label
            ),
            // An unknown state fails closed in `upsert_tray` before this
            // section ever renders, so the fallback colour is unreachable in
            // practice and only keeps this constructor infallible.
            color: tray_state_color(&node.state).unwrap_or(NEUTRAL_COLOR),
            gauges: vec![
                TrayGauge {
                    text: format!("CPU {}", percentage_label(node.cpu_percent)),
                    fill_basis_points: percent_basis_points(node.cpu_percent),
                    series: metric_series(&node.trend, |point| {
                        percent_basis_points(point.cpu_percent)
                    }),
                },
                TrayGauge {
                    text: format!("Memory {memory_label}"),
                    fill_basis_points: ratio_basis_points(
                        node.memory_used_bytes,
                        node.memory_total_bytes,
                    ),
                    series: metric_series(&node.trend, |point| {
                        ratio_basis_points(point.memory_used_bytes, point.memory_total_bytes)
                    }),
                },
                TrayGauge {
                    text: format!("Disk {}", disk_percentage_label(node)),
                    fill_basis_points: ratio_basis_points(
                        node.disk_used_bytes,
                        node.disk_total_bytes,
                    ),
                    series: metric_series(&node.trend, |point| {
                        ratio_basis_points(point.disk_used_bytes, point.disk_total_bytes)
                    }),
                },
                temperature_gauge(node),
            ],
            link: link_label(node),
        },
    }
}

/// Basis-point history for one metric, keeping only the trend points where the
/// metric was actually read. A gap in readings shortens the drawn history
/// rather than fabricating zeros.
fn metric_series<F>(trend: &[TrayTrendPoint], value: F) -> Vec<(i64, u16)>
where
    F: Fn(&TrayTrendPoint) -> Option<u16>,
{
    trend
        .iter()
        .filter_map(|point| value(point).map(|basis_points| (point.timestamp_ms, basis_points)))
        .collect()
}

/// The hottest sensor, named: an unattributed number would leave the operator
/// unable to tell a battery reading from a CPU package one. The chart maps
/// 0–100 °C onto the fixed scale; a machine with no readable sensor shows an
/// em dash and no chart rather than a plausible zero.
fn temperature_gauge(node: &TrayNodeSnapshot) -> TrayGauge {
    let series = metric_series(&node.trend, |point| {
        point.temperature_celsius.map(scale_basis_points)
    });
    node.temperature.as_ref().map_or_else(
        || TrayGauge {
            text: String::from("Temperature —"),
            fill_basis_points: None,
            series: Vec::new(),
        },
        |temperature| TrayGauge {
            text: format!(
                "Temperature {:.0} °C {}",
                temperature.celsius, temperature.label
            ),
            fill_basis_points: Some(scale_basis_points(temperature.celsius)),
            series,
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

const GAUGE_WIDTH: usize = 96;
const GAUGE_HEIGHT: usize = 20;

/// Per-column chart values across the icon width, positioned by timestamp and
/// linearly interpolated between samples. A metric with fewer than two history
/// points falls back to a flat line at the current reading, and a metric with
/// no reading at all yields no columns (no icon).
// The casts convert timestamp *differences* bounded by the trend window's
// span (well under 2^53 ms), so they are exact in an f64 mantissa.
#[expect(clippy::cast_precision_loss)]
fn gauge_columns(gauge: &TrayGauge) -> Vec<u16> {
    if let (Some(first), Some(last)) = (gauge.series.first(), gauge.series.last())
        && gauge.series.len() >= 2
        && last.0 > first.0
    {
        let span = last.0 - first.0;
        let mut columns = Vec::with_capacity(GAUGE_WIDTH);
        let mut segment = 0_usize;
        for x in 0..GAUGE_WIDTH {
            // x → timestamp, then interpolate inside the surrounding segment.
            let t = first.0
                + span * i64::try_from(x).unwrap_or_default()
                    / i64::try_from(GAUGE_WIDTH - 1).unwrap_or(1);
            while segment + 2 < gauge.series.len() && gauge.series[segment + 1].0 < t {
                segment += 1;
            }
            let (t0, v0) = gauge.series[segment];
            let (t1, v1) = gauge.series[segment + 1];
            let value = if t1 > t0 {
                let progress = (t.clamp(t0, t1) - t0) as f64 / (t1 - t0) as f64;
                f64::from(v0) + (f64::from(v1) - f64::from(v0)) * progress
            } else {
                f64::from(v1)
            };
            columns.push(scale_basis_points(value / 100.0));
        }
        return columns;
    }
    let flat = gauge
        .fill_basis_points
        .or_else(|| gauge.series.last().map(|(_, value)| *value));
    flat.map_or_else(Vec::new, |value| vec![value; GAUGE_WIDTH])
}

fn gauge_fill_height(basis_points: u16) -> usize {
    usize::from(basis_points.min(10_000)) * GAUGE_HEIGHT / 10_000
}

fn gauge_image(gauge: &TrayGauge, color: [u8; 4]) -> Option<Image<'static>> {
    let columns = gauge_columns(gauge);
    if columns.is_empty() {
        return None;
    }
    Some(gauge_icon(&columns, color))
}

/// A fixed-scale (0–100) time-series area chart rendered as a menu-item icon:
/// a translucent grey plot area, the history filled in a translucent state
/// colour, and a solid cap line tracing the values. The right edge is "now",
/// so the chart also reads as a bar gauge of the current value.
fn gauge_icon(columns: &[u16], color: [u8; 4]) -> Image<'static> {
    const PLOT: [u8; 4] = [127, 127, 127, 40];
    let fill = [color[0], color[1], color[2], 140];
    let mut rgba = vec![0_u8; GAUGE_WIDTH * GAUGE_HEIGHT * 4];
    for (x, basis_points) in columns.iter().enumerate().take(GAUGE_WIDTH) {
        let height = gauge_fill_height(*basis_points);
        // A zero reading still draws its cap so "0%" and "no data" differ.
        let cap_top = GAUGE_HEIGHT - height.max(1);
        let fill_top = GAUGE_HEIGHT - height;
        for y in 0..GAUGE_HEIGHT {
            let pixel = if y >= cap_top && y < cap_top + 2 {
                color
            } else if y >= fill_top {
                fill
            } else {
                PLOT
            };
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
        GAUGE_HEIGHT, GAUGE_WIDTH, TrayGauge, TrayNodeSnapshot, TrayTemperature, TrayTrendPoint,
        fleet_tray_status, gauge_columns, gauge_fill_height, link_label, machine_tray_id,
        retired_tray_ids, status_section, tray_machine_menu, tray_node_status, tray_state_color,
    };

    fn ids(values: &[&str]) -> HashSet<String> {
        values.iter().map(|value| (*value).to_owned()).collect()
    }

    fn trend_point(timestamp_ms: i64, cpu_percent: Option<f64>) -> TrayTrendPoint {
        TrayTrendPoint {
            timestamp_ms,
            cpu_percent,
            memory_used_bytes: None,
            memory_total_bytes: None,
            disk_used_bytes: None,
            disk_total_bytes: None,
            temperature_celsius: None,
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
    fn the_top_level_row_carries_the_minimal_glance_metrics() {
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
            trend: vec![
                trend_point(1_000, Some(40.0)),
                trend_point(2_000, Some(60.0)),
            ],
        };

        assert_eq!(machine_tray_id(&node), "machine-server-id");
        assert_eq!(tray_node_status(&node), "Server ▲");
        let machine = tray_machine_menu(&node);
        assert_eq!(machine.title, "Server · Warning · CPU 60% · Memory 30%");
        assert_eq!(
            machine.section.header,
            "▲ Server — Warning · CPU 60% · Mem 30%"
        );
        // The gauge colour tracks the machine's state (owned by the user's
        // alert rules), not a tray-local threshold set.
        assert_eq!(machine.section.color, [230, 189, 89, 255]);
        assert_eq!(machine.section.link, "Relayed");
        let cpu = &machine.section.gauges[0];
        assert_eq!(cpu.text, "CPU 60%");
        assert_eq!(cpu.fill_basis_points, Some(6_000));
        // The chart is fed from the snapshot's own trend window.
        assert_eq!(cpu.series, vec![(1_000, 4_000), (2_000, 6_000)]);
        assert_eq!(machine.section.gauges[3].text, "Temperature 72 °C CPU die");
    }

    #[test]
    fn missing_readings_render_no_chart_and_no_plausible_zero() {
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
            trend: Vec::new(),
        };
        assert_eq!(link_label(&steamdeck), "LAN direct · RTT 8 ms");
        let section = tray_machine_menu(&steamdeck).section;
        assert_eq!(section.header, "● steamdeck — Healthy · CPU — · Mem —");
        for gauge in &section.gauges {
            assert_eq!(gauge.fill_basis_points, None);
            assert!(gauge.series.is_empty());
            assert!(gauge_columns(gauge).is_empty());
        }

        let server = TrayNodeSnapshot {
            id: String::from("server-id"),
            name: String::from("Server"),
            state: String::from("warning"),
            path: String::from("relayed"),
            cpu_percent: Some(60.0),
            memory_used_bytes: None,
            memory_total_bytes: None,
            disk_used_bytes: None,
            disk_total_bytes: None,
            temperature: None,
            rtt_ms: None,
            trend: Vec::new(),
        };
        assert_eq!(
            fleet_tray_status(&[server, steamdeck]),
            "Server ▲ · steamdeck ●"
        );
    }

    #[test]
    fn the_chart_interpolates_by_timestamp_and_falls_back_to_a_flat_bar() {
        let charted = TrayGauge {
            text: String::from("CPU 100%"),
            fill_basis_points: Some(10_000),
            series: vec![(0, 0), (1_000, 10_000)],
        };
        let columns = gauge_columns(&charted);
        assert_eq!(columns.len(), GAUGE_WIDTH);
        assert_eq!(columns.first(), Some(&0));
        assert_eq!(columns.last(), Some(&10_000));
        // Monotonic input stays monotonic across the interpolated columns.
        assert!(columns.windows(2).all(|pair| pair[0] <= pair[1]));

        // A single sample cannot span a time axis: the chart degrades to a
        // flat bar at the current reading instead of disappearing.
        let flat = TrayGauge {
            text: String::from("CPU 40%"),
            fill_basis_points: Some(4_000),
            series: vec![(0, 4_000)],
        };
        assert_eq!(gauge_columns(&flat), vec![4_000; GAUGE_WIDTH]);
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
                .all(|gauge| gauge.fill_basis_points.is_none() && gauge.series.is_empty())
        );
        assert_eq!(section.link, "—");
    }

    #[test]
    fn gauge_fill_spans_the_chart_proportionally_and_saturates() {
        assert_eq!(gauge_fill_height(0), 0);
        assert_eq!(gauge_fill_height(5_000), GAUGE_HEIGHT / 2);
        assert_eq!(gauge_fill_height(10_000), GAUGE_HEIGHT);
        // An over-range reading must not overrun the icon buffer.
        assert_eq!(gauge_fill_height(u16::MAX), GAUGE_HEIGHT);
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
