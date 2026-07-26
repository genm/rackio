#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
integration_root="$(mktemp -d "${TMPDIR:-/tmp}/rackio-two-daemon.XXXXXX")"
viewer_pid=""
server_pid=""

cleanup() {
  status=$?
  if [[ -n "$viewer_pid" ]]; then
    kill "$viewer_pid" 2>/dev/null || true
    wait "$viewer_pid" 2>/dev/null || true
  fi
  if [[ -n "$server_pid" ]]; then
    kill "$server_pid" 2>/dev/null || true
    wait "$server_pid" 2>/dev/null || true
  fi
  if [[ "$status" -ne 0 ]]; then
    evidence="$repo_root/test-results/two-daemon"
    mkdir -p "$evidence"
    cp "$integration_root"/*.log "$evidence/" 2>/dev/null || true
  fi
  rm -rf "$integration_root"
  exit "$status"
}
trap cleanup EXIT HUP INT TERM

command -v jq >/dev/null 2>&1 || {
  echo "two-daemon pairing test requires jq" >&2
  exit 2
}

mkdir -p \
  "$integration_root/viewer/config" \
  "$integration_root/viewer/data" \
  "$integration_root/viewer/state" \
  "$integration_root/viewer/log" \
  "$integration_root/server/config" \
  "$integration_root/server/data" \
  "$integration_root/server/state" \
  "$integration_root/server/log"

cargo build -p rackio-agent >/dev/null
# Cargo may use a shared or caller-selected target directory. Resolve the
# authoritative path instead of silently waiting on a binary that was not built.
target_dir="$(cargo metadata --format-version=1 --no-deps | jq -er '.target_directory')"
binary="$target_dir/debug/rackio"
viewer_socket="$integration_root/viewer.sock"
server_socket="$integration_root/server.sock"

viewer() {
  env \
    RACKIO_CONFIG_DIR="$integration_root/viewer/config" \
    RACKIO_DATA_DIR="$integration_root/viewer/data" \
    RACKIO_STATE_DIR="$integration_root/viewer/state" \
    RACKIO_LOG_DIR="$integration_root/viewer/log" \
    RACKIO_SOCKET="$viewer_socket" \
    "$binary" "$@"
}

server() {
  env \
    RACKIO_CONFIG_DIR="$integration_root/server/config" \
    RACKIO_DATA_DIR="$integration_root/server/data" \
    RACKIO_STATE_DIR="$integration_root/server/state" \
    RACKIO_LOG_DIR="$integration_root/server/log" \
    RACKIO_SOCKET="$server_socket" \
    "$binary" "$@"
}

start_viewer() {
  viewer daemon >>"$integration_root/viewer.log" 2>&1 &
  viewer_pid=$!
}

start_server() {
  server daemon >>"$integration_root/server.log" 2>&1 &
  server_pid=$!
}

wait_for_command() {
  description="$1"
  shift
  for _ in {1..20}; do
    if "$@" >/dev/null 2>&1; then
      return 0
    fi
    sleep 0.25
  done
  echo "timed out waiting for $description" >&2
  return 1
}

wait_for_remote_sample() {
  for _ in {1..24}; do
    fleet="$(viewer fleet 2>/dev/null || true)"
    if [[ -n "$fleet" ]] &&
      [[ "$(jq -r '.data.remotes | length' <<<"$fleet")" == "1" ]] &&
      [[ "$(jq -r '.data.remotes[0].latest.cpu_percent != null' <<<"$fleet")" == "true" ]]; then
      printf '%s' "$fleet"
      return 0
    fi
    sleep 0.25
  done
  echo "remote metric sample did not arrive within six seconds" >&2
  return 1
}

wait_for_remote_history() {
  endpoint_id="$1"
  for _ in {1..60}; do
    history="$(viewer history "$endpoint_id" --hours 24 2>/dev/null || true)"
    if [[ -n "$history" ]] && [[ "$(jq -r '.data | length > 0' <<<"$history")" == "true" ]]; then
      printf '%s' "$history"
      return 0
    fi
    sleep 0.25
  done
  echo "remote history did not become queryable within fifteen seconds" >&2
  return 1
}

start_viewer
start_server
wait_for_command "viewer daemon" viewer status
wait_for_command "monitored daemon" server status

bundle="$(server pairing create | jq -r '.data')"
imported="$(viewer pairing import "$bundle")"
[[ "$(jq -r '.ok' <<<"$imported")" == "true" ]]
fleet="$(wait_for_remote_sample)"
[[ "$(jq -r '.data.remotes[0].path' <<<"$fleet")" == "lan_direct" ]]
endpoint_id="$(jq -r '.data.remotes[0].endpoint_id' <<<"$fleet")"
history="$(wait_for_remote_history "$endpoint_id")"
[[ "$(jq -r '.data[0].timestamp_ms != null' <<<"$history")" == "true" ]]
server_endpoint_id="$(server status | jq -r '.data.endpoint_id')"
local_history="$(server history "$server_endpoint_id" --hours 24)"
[[ "$(jq -r '.data | length > 0' <<<"$local_history")" == "true" ]]

if viewer pairing import "$bundle" >/dev/null 2>&1; then
  echo "the same pairing bundle was accepted twice" >&2
  exit 1
fi
if rg -q 'one_time_secret' "$integration_root/viewer/data/monitored-machines.json"; then
  echo "the viewer persisted a one-time pairing secret" >&2
  exit 1
fi
if rg -q 'rackio-pair:|one_time_secret|"cpu_percent"' \
  "$integration_root/viewer.log" "$integration_root/server.log" \
  "$integration_root/viewer/log" "$integration_root/server/log"; then
  echo "daemon logs exposed a pairing bundle, secret, or metric payload" >&2
  exit 1
fi

kill "$viewer_pid"
wait "$viewer_pid" 2>/dev/null || true
viewer_pid=""
kill "$server_pid"
wait "$server_pid" 2>/dev/null || true
server_pid=""
start_viewer
wait_for_command "restarted viewer daemon" viewer status
restored_fleet="$(viewer fleet)"
[[ "$(jq -r '.data.remotes[0].latest.cpu_percent != null' <<<"$restored_fleet")" == "true" ]]
[[ "$(jq -r '.data.remotes[0].last_seen_ms != null' <<<"$restored_fleet")" == "true" ]]
start_server
wait_for_command "restarted monitored daemon" server status
restarted_fleet="$(wait_for_remote_sample)"

printf '%s\n' "$(jq -n \
  --arg path "$(jq -r '.data.remotes[0].path' <<<"$restarted_fleet")" \
  '{
    pairing: true,
    remote_metric: true,
    remote_history: true,
    local_history: true,
    sensitive_logs_absent: true,
    reused_bundle_rejected: true,
    last_snapshot_restored_offline: true,
    viewer_restart_reconnected: true,
    path: $path
  }')"
