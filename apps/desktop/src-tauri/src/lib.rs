mod ssh_bootstrap;

use base64::{Engine as _, engine::general_purpose::STANDARD};
use qrcode::{QrCode, render::svg};
use std::{fs::OpenOptions, io::Write, path::PathBuf};
use tauri::{
    AppHandle, Manager,
    image::Image,
    menu::{Menu, MenuItem},
    tray::TrayIconBuilder,
};
use tauri_plugin_autostart::MacosLauncher;

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
        Vec::new(),
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
        let history = remote
            .get("history")
            .and_then(serde_json::Value::as_array)
            .cloned()
            .unwrap_or_default();
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
            history,
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
    let qr = pairing_qr_data_url(bundle);
    let lan_warning = response
        .get("warnings")
        .and_then(serde_json::Value::as_array)
        .and_then(|warnings| warnings.first())
        .and_then(serde_json::Value::as_str);
    Ok(match qr {
        Ok(qr_data_url) => serde_json::json!({
            "bundle": bundle,
            "qrDataUrl": qr_data_url,
            "lanWarning": lan_warning,
        }),
        Err(error) => serde_json::json!({
            "bundle": bundle,
            "qrError": error,
            "lanWarning": lan_warning,
        }),
    })
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
        samples
            .iter()
            .map(|sample| {
                serde_json::json!({
                    "timestampMs": sample.get("timestamp_ms"),
                    "cpuPercent": sample.get("cpu_percent"),
                    "memoryUsedBytes": sample.get("memory_used_bytes"),
                    "memoryTotalBytes": sample.get("memory_total_bytes"),
                    "networkReceivedBytesPerSecond": sample
                        .get("network")
                        .and_then(|network| network.get("received_bytes_per_second")),
                    "networkSentBytesPerSecond": sample
                        .get("network")
                        .and_then(|network| network.get("sent_bytes_per_second")),
                })
            })
            .collect(),
    ))
}

#[tauri::command]
fn set_tray_state(app: AppHandle, state: String) -> Result<(), String> {
    let result = apply_tray_state(&app, &state);
    drop((app, state));
    result
}

fn apply_tray_state(app: &AppHandle, state: &str) -> Result<(), String> {
    let (label, color) = match state {
        "healthy" => ("Healthy", [84, 217, 139, 255]),
        "warning" => ("Warning", [230, 189, 89, 255]),
        "degraded" => ("Degraded", [230, 189, 89, 255]),
        "stale" => ("Stale", [164, 173, 168, 255]),
        "critical" => ("Critical", [255, 111, 103, 255]),
        "offline" => ("Offline", [255, 111, 103, 255]),
        "auth_error" => ("Authentication error", [255, 111, 103, 255]),
        "incompatible" => ("Incompatible agent", [255, 111, 103, 255]),
        _ => return Err(String::from("Unknown fleet state for tray icon.")),
    };
    let tray = app
        .tray_by_id("main")
        .ok_or_else(|| String::from("Rackio tray icon is unavailable."))?;
    tray.set_icon(Some(state_icon(color)))
        .map_err(|error| format!("Could not update the Rackio tray icon: {error}"))?;
    tray.set_tooltip(Some(format!("Rackio · {label}")))
        .map_err(|error| format!("Could not update the Rackio tray tooltip: {error}"))?;
    #[cfg(target_os = "macos")]
    tray.set_icon_as_template(false)
        .map_err(|error| format!("Could not configure the Rackio tray icon: {error}"))?;
    Ok(())
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

fn machine_json(
    source: &serde_json::Value,
    node_state: &str,
    path: &str,
    rtt_ms: Option<u64>,
    last_seen_ms: Option<i64>,
    history: Vec<serde_json::Value>,
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
        "rttMs": rtt_ms,
        "lastSeenMs": last_seen_ms,
        "history": if history.is_empty() {
            cpu.into_iter().map(serde_json::Value::from).collect()
        } else {
            history
        },
        "detail": detail,
    }))
}

#[cfg(unix)]
async fn daemon_request(
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
async fn daemon_request(
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
async fn daemon_request(
    _command: serde_json::Value,
) -> Result<serde_json::Value, Box<dyn std::error::Error>> {
    Err("local daemon IPC is unsupported on this platform".into())
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_notification::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_autostart::init(
            MacosLauncher::LaunchAgent,
            None,
        ))
        .setup(|app| {
            let show = MenuItem::with_id(app, "show", "Open Rackio", true, None::<&str>)?;
            let quit = MenuItem::with_id(app, "quit", "Quit tray", true, None::<&str>)?;
            let menu = Menu::with_items(app, &[&show, &quit])?;
            let mut builder = TrayIconBuilder::with_id("main")
                .tooltip("Rackio")
                .menu(&menu)
                .show_menu_on_left_click(true)
                .on_menu_event(|app, event| match event.id().as_ref() {
                    "show" => {
                        if let Some(window) = app.get_webview_window("main") {
                            let _ = window.show();
                            let _ = window.set_focus();
                        }
                    }
                    "quit" => app.exit(0),
                    _ => {}
                });
            if let Some(icon) = app.default_window_icon() {
                builder = builder.icon(icon.clone());
            }
            if builder.build(app).is_err()
                && let Some(window) = app.get_webview_window("main")
            {
                // A normal window is the explicit fallback on Linux desktops
                // without a tray implementation.
                let _ = window.show();
                let _ = window.set_focus();
            }
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            fleet_snapshot,
            pair_machine,
            create_pairing_share,
            save_pairing_bundle,
            machine_history,
            set_tray_state,
            ssh_bootstrap::ssh_inspect_host,
            ssh_bootstrap::ssh_bootstrap
        ])
        .run(tauri::generate_context!())
        .unwrap_or_else(|error| panic!("failed to run Rackio desktop: {error}"));
}

#[cfg(test)]
mod tests {
    use super::{pairing_qr_data_url, save_pairing_bundle};

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
}
