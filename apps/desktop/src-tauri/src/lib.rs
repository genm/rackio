use tauri::{
    Manager,
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
    candidates.push(std::path::PathBuf::from("/var/run/rackio/agent.sock"));
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

#[cfg(not(unix))]
async fn daemon_request(
    _command: serde_json::Value,
) -> Result<serde_json::Value, Box<dyn std::error::Error>> {
    Err("Windows named-pipe local IPC is not available in this development build".into())
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_notification::init())
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
            builder.build(app)?;
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![fleet_snapshot, pair_machine])
        .run(tauri::generate_context!())
        .unwrap_or_else(|error| panic!("failed to run Rackio desktop: {error}"));
}
