//! Bounded OS-local IPC between the Tauri viewer and its long-lived agent.

use std::time::Duration;

/// A wedged daemon — one that accepts the local connection but never answers —
/// must be reported as unavailable rather than leaving the last healthy
/// snapshot on screen. Both the React dashboard and the tray monitor poll every
/// two seconds and schedule the next poll only after the current request
/// settles, so the bound has to be shorter than two poll intervals (4s) for a
/// stall to surface before a second poll would have been due. Three seconds
/// still leaves room for a slow but live answer on a loaded machine.
const DAEMON_REQUEST_TIMEOUT: Duration = Duration::from_secs(3);

/// Bound every local IPC exchange. Returning an explicit timeout error is what
/// drives the tray monitor into its `daemon_unavailable` branch and lets the
/// React poll reschedule from its `finally`.
pub(crate) async fn request(
    command: serde_json::Value,
) -> Result<serde_json::Value, Box<dyn std::error::Error>> {
    bounded_exchange(exchange(command)).await
}

async fn bounded_exchange<F>(exchange: F) -> Result<serde_json::Value, Box<dyn std::error::Error>>
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
async fn exchange(
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
async fn exchange(
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
async fn exchange(
    _command: serde_json::Value,
) -> Result<serde_json::Value, Box<dyn std::error::Error>> {
    Err("local daemon IPC is unsupported on this platform".into())
}

#[cfg(test)]
mod tests {
    use super::{DAEMON_REQUEST_TIMEOUT, bounded_exchange};

    /// A daemon that accepts the connection and then never answers must not
    /// leave the caller pending, which would freeze both the React poll loop
    /// and the tray monitor on the last healthy snapshot.
    #[tokio::test]
    async fn a_wedged_daemon_fails_with_an_explicit_timeout() {
        tokio::time::pause();
        let result = bounded_exchange(std::future::pending()).await;
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
        let response = bounded_exchange(async { Ok(serde_json::json!({ "ok": true })) })
            .await
            .unwrap_or_else(|error| panic!("{error}"));
        assert_eq!(response, serde_json::json!({ "ok": true }));
    }
}
