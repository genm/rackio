#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
archive="${1:-}"
checksum="${2:-}"
report="${3:-test-results/linux-systemd-install.json}"
report_dir="$(dirname "$report")"
viewer_user="${RACKIO_VIEWER_USER:-$(id -un)}"
denied_user="rackio-ci-denied"
cleanup_done=0

fail() {
  printf 'rackio system install test: %s\n' "$*" >&2
  exit 1
}

if [[ "${GITHUB_ACTIONS:-}" != "true" || "${RACKIO_SYSTEM_INSTALL_TEST:-}" != "1" ]]; then
  fail "this destructive system test is restricted to an explicitly opted-in GitHub-hosted runner"
fi
[[ "$(uname -s)" == "Linux" ]] || fail "Linux is required"
[[ -n "$archive" && -f "$archive" ]] || fail "release archive is missing"
[[ -n "$checksum" && -f "$checksum" ]] || fail "release checksum is missing"
command -v systemctl >/dev/null || fail "systemd is required"
command -v jq >/dev/null || fail "jq is required"
command -v sudo >/dev/null || fail "sudo is required"
command -v runuser >/dev/null || fail "runuser is required"

# This test owns the machine-wide Rackio lifecycle, so refuse to touch a host
# with any pre-existing Rackio installation or service identity.
[[ ! -e /usr/local/bin/rackio ]] || fail "a Rackio binary already exists"
[[ ! -e /etc/systemd/system/rackio.service ]] || fail "a Rackio service already exists"
for rackio_path in \
  /etc/rackio \
  /var/lib/rackio \
  /var/log/rackio \
  /run/rackio \
  /usr/local/lib/rackio \
  /usr/local/share/doc/rackio; do
  [[ ! -e "$rackio_path" ]] || fail "pre-existing Rackio state exists at $rackio_path"
done
! id rackio >/dev/null 2>&1 || fail "the rackio service user already exists"
! getent group rackio-viewers >/dev/null 2>&1 || fail "the rackio-viewers group already exists"
! id "$denied_user" >/dev/null 2>&1 || fail "the denied test user already exists"

cleanup() {
  if [[ "$cleanup_done" -eq 0 ]]; then
    sudo sh "$repo_root/packaging/linux/uninstall.sh" --purge >/dev/null 2>&1 || true
    sudo userdel "$denied_user" >/dev/null 2>&1 || true
  fi
}
trap cleanup EXIT HUP INT TERM

wait_for_root_health() {
  local _
  for _ in {1..30}; do
    if sudo env RACKIO_SOCKET=/run/rackio/agent.sock /usr/local/bin/rackio status 2>/dev/null |
      jq -e '.ok == true' >/dev/null; then
      return 0
    fi
    sleep 1
  done
  return 1
}

mkdir -p "$report_dir"
sudo env RACKIO_VIEWER_USER="$viewer_user" \
  sh "$repo_root/install.sh" --archive "$archive" --checksum "$checksum"

systemctl is-enabled --quiet rackio.service
systemctl is-active --quiet rackio.service
wait_for_root_health

# The current CI shell does not inherit a group added after login. Inspect
# metadata as root, then prove viewer access below in a fresh group context.
[[ "$(sudo stat -c '%a' /run/rackio/agent.sock)" == "660" ]]
[[ "$(sudo stat -c '%G' /run/rackio/agent.sock)" == "rackio-viewers" ]]
id -nG "$viewer_user" | tr ' ' '\n' | grep -Fxq rackio-viewers
sudo -u "$viewer_user" -g rackio-viewers \
  env RACKIO_SOCKET=/run/rackio/agent.sock /usr/local/bin/rackio status |
  jq -e '.ok == true' >/dev/null

# The threat model guarantees the private key is a 0600 file inside a
# daemon-owned 0700 directory. Prove both halves: a rackio-viewers member
# (who legitimately reaches the IPC socket above) must still be denied
# traversal into /var/lib/rackio and read access to identity.key.
[[ "$(sudo stat -c '%a' /var/lib/rackio)" == "700" ]]
sudo test -e /var/lib/rackio/identity.key
[[ "$(sudo stat -c '%a' /var/lib/rackio/identity.key)" == "600" ]]
if sudo -u "$viewer_user" -g rackio-viewers cat /var/lib/rackio/identity.key >/dev/null 2>&1; then
  fail "a rackio-viewers member could read /var/lib/rackio/identity.key"
fi

# A viewer reaches the socket; it must not be able to replace it. Directory
# write permission is what governs unlinking and re-creating entries, so a
# group-writable runtime directory would let any viewer bind their own socket at
# the daemon's path and answer for every later client, including root.
[[ "$(sudo stat -c '%a' /run/rackio)" == "750" ]]
if sudo -u "$viewer_user" -g rackio-viewers touch /run/rackio/squatted 2>/dev/null; then
  fail "a rackio-viewers member could create an entry in the runtime directory"
fi

sudo useradd --system --no-create-home --shell /usr/sbin/nologin "$denied_user"
denied_output=""
if denied_output="$(
  sudo runuser -u "$denied_user" -- \
    env RACKIO_SOCKET=/run/rackio/agent.sock /usr/local/bin/rackio status 2>&1
)"; then
  fail "a user outside rackio-viewers reached the local daemon"
fi
printf '%s\n' "$denied_output" >"$report_dir/linux-systemd-denied.txt"

sudo systemctl restart rackio.service
systemctl is-active --quiet rackio.service
wait_for_root_health

# An upgrade installs over a running daemon. `systemctl enable --now` does not
# restart an already-active unit, so without an explicit restart the installer
# reports success while the previous binary image keeps running.
upgrade_pid_before="$(systemctl show -p MainPID --value rackio.service)"
sudo env RACKIO_VIEWER_USER="$viewer_user" \
  sh "$repo_root/install.sh" --archive "$archive" --checksum "$checksum" >/dev/null
systemctl is-active --quiet rackio.service
wait_for_root_health
upgrade_pid_after="$(systemctl show -p MainPID --value rackio.service)"
if [[ "$upgrade_pid_after" == "$upgrade_pid_before" ]]; then
  fail "installing over a running service left the previous process running"
fi
if sudo readlink "/proc/$upgrade_pid_after/exe" | grep -q '(deleted)'; then
  fail "the running daemon still executes a replaced binary image"
fi

sudo touch /var/lib/rackio/preserve-marker
sudo /usr/local/lib/rackio/uninstall.sh
[[ ! -e /usr/local/bin/rackio ]]
[[ ! -e /etc/systemd/system/rackio.service ]]
sudo test -e /var/lib/rackio/preserve-marker
if systemctl is-active --quiet rackio.service; then
  fail "preserving uninstall left the service active"
fi
if systemctl is-enabled --quiet rackio.service; then
  fail "preserving uninstall left the service enabled"
fi
id rackio >/dev/null
getent group rackio-viewers >/dev/null

sudo env RACKIO_VIEWER_USER="$viewer_user" \
  sh "$repo_root/install.sh" --archive "$archive" --checksum "$checksum" >/dev/null
systemctl is-active --quiet rackio.service
wait_for_root_health
sudo test -e /var/lib/rackio/preserve-marker

sudo /usr/local/lib/rackio/uninstall.sh --purge
[[ ! -e /usr/local/bin/rackio ]]
[[ ! -e /etc/systemd/system/rackio.service ]]
[[ ! -e /etc/rackio ]]
[[ ! -e /var/lib/rackio ]]
[[ ! -e /var/log/rackio ]]
if id rackio >/dev/null 2>&1; then
  fail "purge left the rackio service user behind"
fi
if getent group rackio-viewers >/dev/null 2>&1; then
  fail "purge left the rackio-viewers group behind"
fi

sudo userdel "$denied_user"
cleanup_done=1

jq --null-input \
  --arg architecture "$(uname -m)" \
  '{
    ok: true,
    architecture: $architecture,
    service_enabled: true,
    service_active: true,
    root_health: true,
    viewer_group_health: true,
    state_directory_isolated_from_viewers: true,
    runtime_directory_not_viewer_writable: true,
    unauthorized_user_rejected: true,
    restart_health: true,
    in_place_upgrade_replaced_process: true,
    preserving_uninstall: true,
    reinstall_preserved_state: true,
    purge_removed_state: true
  }' >"$report"
