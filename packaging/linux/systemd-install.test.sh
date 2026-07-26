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
    if sudo env RACKIO_SOCKET=/run/rackio/agent.sock /usr/local/bin/rackio status |
      jq -e '.ok == true' >/dev/null 2>&1; then
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

sudo touch /var/lib/rackio/preserve-marker
sudo /usr/local/lib/rackio/uninstall.sh
[[ ! -e /usr/local/bin/rackio ]]
[[ ! -e /etc/systemd/system/rackio.service ]]
[[ -e /var/lib/rackio/preserve-marker ]]
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
[[ -e /var/lib/rackio/preserve-marker ]]

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
    unauthorized_user_rejected: true,
    restart_health: true,
    preserving_uninstall: true,
    reinstall_preserved_state: true,
    purge_removed_state: true
  }' >"$report"
