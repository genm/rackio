use std::{
    collections::BTreeSet,
    ffi::{OsStr, OsString},
    fs,
    io::Write as _,
    path::{Path, PathBuf},
    process::{Command, Output, Stdio},
};

use directories::ProjectDirs;
use serde::{Deserialize, Serialize};
use tauri::ipc::Channel;

const LOCAL_INSTALLER: &str = include_str!("../../../../install.sh");

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SshTarget {
    host: String,
    user: String,
    port: u16,
    identity_file: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SshHostIdentity {
    host_keys: Vec<String>,
    fingerprints: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SshBootstrapRequest {
    target: SshTarget,
    accepted_host_keys: Vec<String>,
    archive_path: String,
    checksum_path: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SshBootstrapProgress {
    stage: &'static str,
    detail: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SshBootstrapResult {
    pairing_bundle: String,
    remote_platform: String,
}

#[tauri::command]
pub async fn ssh_inspect_host(target: SshTarget) -> Result<SshHostIdentity, String> {
    tauri::async_runtime::spawn_blocking(move || inspect_host(&target))
        .await
        .map_err(|error| format!("SSH host inspection task failed: {error}"))?
}

#[tauri::command]
pub async fn ssh_bootstrap(
    request: SshBootstrapRequest,
    on_progress: Channel<SshBootstrapProgress>,
) -> Result<SshBootstrapResult, String> {
    tauri::async_runtime::spawn_blocking(move || bootstrap(&request, &on_progress))
        .await
        .map_err(|error| format!("SSH bootstrap task failed: {error}"))?
}

fn bootstrap(
    request: &SshBootstrapRequest,
    progress: &Channel<SshBootstrapProgress>,
) -> Result<SshBootstrapResult, String> {
    validate_target(&request.target)?;
    let archive = validate_local_file(&request.archive_path, "release archive")?;
    let checksum = validate_local_file(&request.checksum_path, "release checksum")?;
    if request.accepted_host_keys.is_empty() {
        return Err(String::from(
            "Confirm the SSH host fingerprint before installing.",
        ));
    }

    send_progress(progress, "checking_host_key", "Rechecking the SSH host key")?;
    let current_identity = inspect_host(&request.target)?;
    let accepted = normalized_key_set(&request.accepted_host_keys);
    let current = normalized_key_set(&current_identity.host_keys);
    if accepted != current {
        return Err(String::from(
            "SSH host key changed after confirmation. Installation was stopped.",
        ));
    }
    let known_hosts = persist_known_hosts(&current_identity.host_keys)?;

    send_progress(
        progress,
        "checking_access",
        "Checking Linux, systemd, and non-interactive root access",
    )?;
    let preflight = run_ssh(
        &request.target,
        &known_hosts,
        "set -eu; test \"$(uname -s)\" = Linux; command -v systemctl >/dev/null; \
         if test \"$(id -u)\" -ne 0; then command -v sudo >/dev/null; sudo -n true; fi; \
         printf '%s %s' \"$(uname -s)\" \"$(uname -m)\"",
    )?;
    let remote_platform = output_text(&preflight, "SSH preflight")?;

    send_progress(
        progress,
        "uploading",
        "Uploading the verified release archive",
    )?;
    let remote_dir_output = run_ssh(
        &request.target,
        &known_hosts,
        "set -eu; umask 077; mktemp -d /tmp/rackio-bootstrap.XXXXXX",
    )?;
    let remote_dir = output_text(&remote_dir_output, "remote temporary directory")?;
    validate_remote_temp_dir(&remote_dir)?;

    let installer_file = tempfile::Builder::new()
        .prefix("rackio-install-local-")
        .suffix(".sh")
        .tempfile()
        .map_err(|error| format!("Could not prepare the local installer: {error}"))?;
    fs::write(installer_file.path(), LOCAL_INSTALLER)
        .map_err(|error| format!("Could not write the local installer: {error}"))?;

    let result = perform_remote_install(
        request,
        progress,
        &known_hosts,
        &remote_dir,
        &archive,
        &checksum,
        installer_file.path(),
    );

    let cleanup_command = format!("rm -rf -- {remote_dir}");
    let _ = run_ssh(&request.target, &known_hosts, &cleanup_command);

    let pairing_bundle = result?;
    send_progress(
        progress,
        "connecting_p2p",
        "Switching from SSH installation to the encrypted P2P connection",
    )?;
    Ok(SshBootstrapResult {
        pairing_bundle,
        remote_platform,
    })
}

fn perform_remote_install(
    request: &SshBootstrapRequest,
    progress: &Channel<SshBootstrapProgress>,
    known_hosts: &Path,
    remote_dir: &str,
    archive: &Path,
    checksum: &Path,
    installer: &Path,
) -> Result<String, String> {
    run_scp(
        &request.target,
        known_hosts,
        &[archive, checksum, installer],
        remote_dir,
    )?;
    send_progress(
        progress,
        "installing",
        "Verifying the archive and installing the systemd service",
    )?;
    let privilege = if request.target.user == "root" {
        ""
    } else {
        "sudo -n "
    };
    let install_command = format!(
        "set -eu; {privilege}sh {remote_dir}/{} --archive {remote_dir}/{} --checksum {remote_dir}/{}",
        shell_filename(installer)?,
        shell_filename(archive)?,
        shell_filename(checksum)?,
    );
    output_text(
        &run_ssh(&request.target, known_hosts, &install_command)?,
        "remote Rackio install",
    )?;

    send_progress(
        progress,
        "pairing",
        "Opening a one-time pairing window on the new agent",
    )?;
    let pairing_command = format!(
        "{privilege}env RACKIO_SOCKET=/run/rackio/agent.sock /usr/local/bin/rackio pairing create"
    );
    let pairing_output = output_text(
        &run_ssh(&request.target, known_hosts, &pairing_command)?,
        "remote pairing",
    )?;
    extract_pairing_bundle(&pairing_output)
}

fn inspect_host(target: &SshTarget) -> Result<SshHostIdentity, String> {
    validate_target(target)?;
    require_command("ssh-keyscan")?;
    require_command("ssh-keygen")?;
    let output = Command::new("ssh-keyscan")
        .args([
            OsStr::new("-T"),
            OsStr::new("10"),
            OsStr::new("-p"),
            OsStr::new(&target.port.to_string()),
            OsStr::new(&target.host),
        ])
        .output()
        .map_err(|error| format!("Could not start ssh-keyscan: {error}"))?;
    let stdout = output_text(&output, "SSH host-key scan")?;
    let host_keys: Vec<String> = stdout
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .map(str::to_owned)
        .collect();
    if host_keys.is_empty() {
        return Err(String::from("The SSH server did not return a host key."));
    }

    let fingerprints_output = run_with_stdin(
        "ssh-keygen",
        &[OsString::from("-lf"), OsString::from("-")],
        &host_keys.join("\n"),
    )?;
    let fingerprints = output_text(&fingerprints_output, "SSH fingerprint")?
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(str::to_owned)
        .collect();
    Ok(SshHostIdentity {
        host_keys,
        fingerprints,
    })
}

fn run_ssh(target: &SshTarget, known_hosts: &Path, remote_command: &str) -> Result<Output, String> {
    require_command("ssh")?;
    let mut command = Command::new("ssh");
    command.args(ssh_options(target, known_hosts));
    command.arg(remote_target(target));
    command.arg(remote_command);
    command
        .output()
        .map_err(|error| format!("Could not start SSH: {error}"))
}

fn run_scp(
    target: &SshTarget,
    known_hosts: &Path,
    local_files: &[&Path],
    remote_dir: &str,
) -> Result<(), String> {
    require_command("scp")?;
    let mut command = Command::new("scp");
    command.args([
        OsString::from("-P"),
        OsString::from(target.port.to_string()),
        OsString::from("-o"),
        OsString::from("BatchMode=yes"),
        OsString::from("-o"),
        OsString::from("ConnectTimeout=10"),
        OsString::from("-o"),
        OsString::from("StrictHostKeyChecking=yes"),
        OsString::from("-o"),
        OsString::from(known_hosts_option(known_hosts)),
    ]);
    if let Some(identity_file) = target.identity_file.as_deref() {
        command.arg("-i").arg(identity_file);
    }
    command.args(local_files);
    command.arg(format!("{}:{remote_dir}/", remote_target(target)));
    let output = command
        .output()
        .map_err(|error| format!("Could not start SCP: {error}"))?;
    output_text(&output, "release upload").map(|_| ())
}

/// `UserKnownHostsFile` takes a whitespace-separated *list* of files, so an
/// unquoted path is split on spaces. The macOS config directory is
/// `~/Library/Application Support/...`, which would otherwise be read as two
/// nonexistent files and fail every bootstrap with "Host key verification
/// failed". `ssh_config(5)` groups an argument containing spaces with double
/// quotes.
fn known_hosts_option(known_hosts: &Path) -> String {
    format!("UserKnownHostsFile=\"{}\"", known_hosts.display())
}

fn ssh_options(target: &SshTarget, known_hosts: &Path) -> Vec<OsString> {
    let mut args = vec![
        OsString::from("-p"),
        OsString::from(target.port.to_string()),
        OsString::from("-o"),
        OsString::from("BatchMode=yes"),
        OsString::from("-o"),
        OsString::from("ConnectTimeout=10"),
        OsString::from("-o"),
        OsString::from("StrictHostKeyChecking=yes"),
        OsString::from("-o"),
        OsString::from(known_hosts_option(known_hosts)),
    ];
    if let Some(identity_file) = target.identity_file.as_deref() {
        args.push(OsString::from("-i"));
        args.push(OsString::from(identity_file));
    }
    args
}

fn remote_target(target: &SshTarget) -> String {
    if target.host.contains(':') {
        format!("{}@[{}]", target.user, target.host)
    } else {
        format!("{}@{}", target.user, target.host)
    }
}

fn validate_target(target: &SshTarget) -> Result<(), String> {
    if target.host.is_empty()
        || target.host.len() > 255
        || !target
            .host
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || ".:-".contains(character))
    {
        return Err(String::from("SSH host is invalid."));
    }
    if target.user.is_empty()
        || target.user.len() > 64
        || !target.user.chars().all(|character| {
            character.is_ascii_alphanumeric() || matches!(character, '_' | '-' | '.')
        })
    {
        return Err(String::from("SSH user is invalid."));
    }
    if target.port == 0 {
        return Err(String::from("SSH port must be between 1 and 65535."));
    }
    if let Some(identity_file) = target.identity_file.as_deref() {
        validate_local_file(identity_file, "SSH identity file")?;
    }
    Ok(())
}

fn validate_local_file(path: &str, label: &str) -> Result<PathBuf, String> {
    let path = PathBuf::from(path);
    let metadata = fs::metadata(&path)
        .map_err(|error| format!("{label} is not readable at {}: {error}", path.display()))?;
    if !metadata.is_file() {
        return Err(format!("{label} must be a regular file."));
    }
    Ok(path)
}

fn validate_remote_temp_dir(path: &str) -> Result<(), String> {
    if !path.starts_with("/tmp/rackio-bootstrap.")
        || path
            .chars()
            .any(|character| !(character.is_ascii_alphanumeric() || "/._-".contains(character)))
    {
        return Err(String::from(
            "SSH server returned an unsafe temporary directory path.",
        ));
    }
    Ok(())
}

fn persist_known_hosts(keys: &[String]) -> Result<PathBuf, String> {
    let dirs = ProjectDirs::from("dev", "rackio", "rackio")
        .ok_or_else(|| String::from("OS application directories are unavailable."))?;
    let directory = dirs.config_dir().join("ssh");
    fs::create_dir_all(&directory)
        .map_err(|error| format!("Could not create the SSH configuration directory: {error}"))?;
    let path = directory.join("known_hosts");
    let existing = match fs::read_to_string(&path) {
        Ok(value) => value,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(error) => return Err(format!("Could not read Rackio known_hosts: {error}")),
    };
    let mut merged: BTreeSet<String> = existing
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(str::to_owned)
        .collect();
    merged.extend(normalized_key_set(keys));
    let mut file = tempfile::Builder::new()
        .prefix(".known-hosts-")
        .tempfile_in(&directory)
        .map_err(|error| format!("Could not create Rackio known_hosts: {error}"))?;
    for key in merged {
        writeln!(file, "{key}")
            .map_err(|error| format!("Could not write Rackio known_hosts: {error}"))?;
    }
    file.as_file()
        .sync_all()
        .map_err(|error| format!("Could not sync Rackio known_hosts: {error}"))?;
    file.persist(&path)
        .map_err(|error| format!("Could not persist Rackio known_hosts: {}", error.error))?;
    Ok(path)
}

fn normalized_key_set(keys: &[String]) -> BTreeSet<String> {
    keys.iter()
        .map(|key| key.trim())
        .filter(|key| !key.is_empty())
        .map(str::to_owned)
        .collect()
}

fn extract_pairing_bundle(output: &str) -> Result<String, String> {
    let response: serde_json::Value = serde_json::from_str(output)
        .map_err(|error| format!("Remote pairing returned invalid JSON: {error}"))?;
    if !response
        .get("ok")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false)
    {
        return Err(response
            .get("error")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("Remote agent rejected pairing.")
            .to_owned());
    }
    response
        .get("data")
        .and_then(serde_json::Value::as_str)
        .filter(|bundle| bundle.starts_with("rackio-pair:"))
        .map(str::to_owned)
        .ok_or_else(|| String::from("Remote pairing response did not contain a pairing bundle."))
}

fn shell_filename(path: &Path) -> Result<String, String> {
    path.file_name()
        .and_then(OsStr::to_str)
        .filter(|name| {
            !name.is_empty()
                && name
                    .chars()
                    .all(|character| character.is_ascii_alphanumeric() || "._-".contains(character))
        })
        .map(str::to_owned)
        .ok_or_else(|| String::from("A local release file name is unsafe for SSH transfer."))
}

fn require_command(name: &str) -> Result<(), String> {
    Command::new(name)
        .arg("-V")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|_| ())
        .map_err(|_| format!("{name} is required but was not found."))
}

fn run_with_stdin(program: &str, args: &[OsString], stdin: &str) -> Result<Output, String> {
    let mut child = Command::new(program)
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| format!("Could not start {program}: {error}"))?;
    child
        .stdin
        .take()
        .ok_or_else(|| format!("{program} stdin is unavailable"))?
        .write_all(stdin.as_bytes())
        .map_err(|error| format!("Could not write to {program}: {error}"))?;
    child
        .wait_with_output()
        .map_err(|error| format!("Could not wait for {program}: {error}"))
}

fn output_text(output: &Output, operation: &str) -> Result<String, String> {
    if !output.status.success() {
        let error = String::from_utf8_lossy(&output.stderr).trim().to_owned();
        return Err(if error.is_empty() {
            format!("{operation} failed with status {}", output.status)
        } else {
            format!("{operation} failed: {error}")
        });
    }
    String::from_utf8(output.stdout.clone())
        .map(|value| value.trim().to_owned())
        .map_err(|_| format!("{operation} returned non-UTF-8 output"))
}

fn send_progress(
    channel: &Channel<SshBootstrapProgress>,
    stage: &'static str,
    detail: &str,
) -> Result<(), String> {
    channel
        .send(SshBootstrapProgress {
            stage,
            detail: detail.to_owned(),
        })
        .map_err(|error| format!("Could not report SSH bootstrap progress: {error}"))
}

#[cfg(test)]
mod tests {
    use super::{
        SshTarget, extract_pairing_bundle, known_hosts_option, remote_target,
        validate_remote_temp_dir, validate_target,
    };

    #[test]
    fn known_hosts_path_with_spaces_stays_one_file() {
        let option = known_hosts_option(std::path::Path::new(
            "/Users/rack/Library/Application Support/dev.rackio.rackio/ssh/known_hosts",
        ));
        assert_eq!(
            option,
            "UserKnownHostsFile=\"/Users/rack/Library/Application Support/dev.rackio.rackio/ssh/known_hosts\""
        );
    }

    #[test]
    fn rejects_shell_metacharacters_in_ssh_target() {
        let target = SshTarget {
            host: String::from("server.test;touch /tmp/pwned"),
            user: String::from("operator"),
            port: 22,
            identity_file: None,
        };
        assert!(validate_target(&target).is_err());
    }

    #[test]
    fn accepts_ipv6_and_reserved_test_hostnames() {
        for host in ["server.test", "192.0.2.10", "2001:db8::10"] {
            let target = SshTarget {
                host: String::from(host),
                user: String::from("rackio-admin"),
                port: 22,
                identity_file: None,
            };
            assert!(validate_target(&target).is_ok());
        }
    }

    #[test]
    fn brackets_ipv6_for_scp_destination_parsing() {
        let target = SshTarget {
            host: String::from("2001:db8::10"),
            user: String::from("operator"),
            port: 22,
            identity_file: None,
        };
        assert_eq!(remote_target(&target), "operator@[2001:db8::10]");
    }

    #[test]
    fn remote_temp_directory_fails_closed() {
        assert!(validate_remote_temp_dir("/tmp/rackio-bootstrap.A1b2").is_ok());
        assert!(validate_remote_temp_dir("/tmp/rackio-bootstrap.A1b2;id").is_err());
        assert!(validate_remote_temp_dir("/var/tmp/rackio-bootstrap.A1b2").is_err());
    }

    #[test]
    fn pairing_output_must_be_successful_and_contain_a_bundle() {
        let success = r#"{"ok":true,"data":"rackio-pair:abc","error":null}"#;
        assert_eq!(
            extract_pairing_bundle(success).as_deref(),
            Ok("rackio-pair:abc")
        );
        assert!(extract_pairing_bundle(r#"{"ok":false,"error":"denied"}"#).is_err());
        assert!(extract_pairing_bundle(r#"{"ok":true,"data":"not-a-bundle"}"#).is_err());
    }
}
