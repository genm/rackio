mod daemon;
mod pairing;
mod presentation;
mod ssh_bootstrap;
mod tray;
mod updater;

use tauri::Manager;

use presentation::{history_point_from_sample, machine_json};

#[tauri::command]
async fn fleet_snapshot() -> Result<serde_json::Value, String> {
    let response = daemon::request(serde_json::json!({ "command": "fleet_snapshot" }))
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
    let response = daemon::request(serde_json::json!({
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
    let response = daemon::request(serde_json::json!({ "command": "pairing_create" }))
        .await
        .map_err(|error| error.to_string())?;
    let bundle = response
        .get("data")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| String::from("daemon pairing response did not contain a bundle"))?;
    // Fail closed: the viewer promises a five-minute window, so a bundle whose
    // expiry cannot be read must not be presented as an open pairing window.
    let expires_at_ms = pairing::bundle_expiry(bundle)?;
    let qr = pairing::qr_data_url(bundle);
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

#[tauri::command]
async fn machine_history(endpoint_id: String, hours: u16) -> Result<serde_json::Value, String> {
    if endpoint_id.is_empty() || !(1..=168).contains(&hours) {
        return Err(String::from(
            "History requires a paired endpoint and a range between 1 and 168 hours.",
        ));
    }
    let to_ms = chrono::Utc::now().timestamp_millis();
    let from_ms = to_ms.saturating_sub(i64::from(hours) * 60 * 60 * 1_000);
    let response = daemon::request(serde_json::json!({
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

/// Structured logs on stderr, filtered like the agent's (`RUST_LOG`, default
/// `info`). Without a subscriber every `tracing` event the shell emits — tray
/// dispatch failures, update-check failures, updates being disabled — was
/// silently dropped.
fn init_logging() {
    use tracing_subscriber::{EnvFilter, layer::SubscriberExt as _, util::SubscriberInitExt as _};
    // `try_init` only fails when a global subscriber is already installed, in
    // which case events already have somewhere to go.
    let _ = tracing_subscriber::registry()
        .with(EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")))
        .with(
            tracing_subscriber::fmt::layer()
                .json()
                .with_writer(std::io::stderr),
        )
        .try_init();
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    init_logging();
    let context = tauri::generate_context!();
    let builder = tauri::Builder::default()
        .plugin(tauri_plugin_notification::init())
        .plugin(tauri_plugin_dialog::init());
    updater::register(builder, context.config())
        .unwrap_or_else(|error| panic!("failed to run Rackio desktop: {error}"))
        .setup(|app| {
            app.manage(tray::TrayRegistry::<tauri::Wry>::default());
            updater::start(app.handle().clone());
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
            tauri::async_runtime::spawn(tray::run_tray_monitor(app.handle().clone()));
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            fleet_snapshot,
            pair_machine,
            create_pairing_share,
            pairing::save_pairing_bundle,
            machine_history,
            ssh_bootstrap::ssh_inspect_host,
            ssh_bootstrap::ssh_bootstrap
        ])
        .run(context)
        .unwrap_or_else(|error| panic!("failed to run Rackio desktop: {error}"));
}
