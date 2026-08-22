#!/usr/bin/env bash
# Prove that a monitored daemon which restarts does not permanently strand an
# already paired viewer, and that an address it cannot follow stays visibly
# offline instead of looking healthy.
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
source "$repo_root/scripts/reject-matches.sh"
integration_root="$(mktemp -d "${TMPDIR:-/tmp}/rackio-address-change.XXXXXX")"
viewer_pid=""
server_pid=""

cleanup() {
  status="${1:-$?}"
  # A signal trap followed by EXIT must not run cleanup twice or turn an
  # interrupted integration test into a synthetic success.
  trap - EXIT HUP INT TERM
  if [[ -n "$viewer_pid" ]]; then
    kill "$viewer_pid" 2>/dev/null || true
    wait "$viewer_pid" 2>/dev/null || true
  fi
  if [[ -n "$server_pid" ]]; then
    kill "$server_pid" 2>/dev/null || true
    wait "$server_pid" 2>/dev/null || true
  fi
  if [[ "$status" -ne 0 ]]; then
    evidence="$repo_root/test-results/two-daemon-address-change"
    mkdir -p "$evidence"
    cp "$integration_root"/*.log "$evidence/" 2>/dev/null || true
    cp "$integration_root/viewer/data/monitored-machines.json" "$evidence/" 2>/dev/null || true
  fi
  if ! rm -rf "$integration_root"; then
    echo "failed to remove address-change integration root: $integration_root" >&2
    status=1
  fi
  exit "$status"
}
trap 'cleanup $?' EXIT
trap 'cleanup 129' HUP
trap 'cleanup 130' INT
trap 'cleanup 143' TERM

command -v jq >/dev/null 2>&1 || {
  echo "two-daemon address-change test requires jq" >&2
  exit 2
}
command -v node >/dev/null 2>&1 || {
  echo "two-daemon address-change test requires node to reserve free ports" >&2
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
  env \
    RACKIO_CONFIG_DIR="$integration_root/viewer/config" \
    RACKIO_DATA_DIR="$integration_root/viewer/data" \
    RACKIO_STATE_DIR="$integration_root/viewer/state" \
    RACKIO_LOG_DIR="$integration_root/viewer/log" \
    RACKIO_SOCKET="$viewer_socket" \
    "$binary" daemon >>"$integration_root/viewer.log" 2>&1 &
  viewer_pid=$!
}

start_server() {
  env \
    RACKIO_CONFIG_DIR="$integration_root/server/config" \
    RACKIO_DATA_DIR="$integration_root/server/data" \
    RACKIO_STATE_DIR="$integration_root/server/state" \
    RACKIO_LOG_DIR="$integration_root/server/log" \
    RACKIO_SOCKET="$server_socket" \
    "$binary" daemon >>"$integration_root/server.log" 2>&1 &
  server_pid=$!
}

stop_server() {
  [[ -n "$server_pid" ]] || return 0
  kill "$server_pid"
  wait "$server_pid" 2>/dev/null || true
  server_pid=""
}

# Ask the OS for a free UDP port and release it. Hard-coding a port would make
# this test fail on whichever developer machine already uses it.
free_udp_port() {
  node -e '
const socket = require("node:dgram").createSocket("udp4");
socket.bind(0, "127.0.0.1", () => {
  process.stdout.write(String(socket.address().port));
  socket.close();
});
'
}

wait_for_command() {
  description="$1"
  shift
  for _ in {1..40}; do
    if "$@" >/dev/null 2>&1; then
      return 0
    fi
    sleep 0.25
  done
  echo "timed out waiting for $description" >&2
  return 1
}

remote_field() {
  viewer fleet 2>/dev/null | jq -r ".data.remotes[0].$1"
}

# The viewer derives stale/offline from its own last-seen clock, so recovery is
# only proven by a metric sequence that moved past the one seen before the
# restart.
wait_for_sequence_beyond() {
  previous="$1"
  deadline="$2"
  for _ in $(seq 1 "$deadline"); do
    current="$(remote_field 'latest.sequence // -1')"
    state="$(remote_field 'state')"
    if [[ "$current" != "null" ]] && [[ "$current" -gt "$previous" ]] &&
      [[ "$state" == "healthy" || "$state" == "warning" || "$state" == "critical" ]]; then
      return 0
    fi
    sleep 1
  done
  echo "the viewer did not receive a fresh sample beyond sequence $previous" >&2
  viewer fleet >&2 || true
  return 1
}

wait_for_state() {
  expected="$1"
  deadline="$2"
  for _ in $(seq 1 "$deadline"); do
    if [[ "$(remote_field 'state')" == "$expected" ]]; then
      return 0
    fi
    sleep 1
  done
  echo "the viewer never reported $expected" >&2
  viewer fleet >&2 || true
  return 1
}

wait_for_remote_sample() {
  for _ in {1..40}; do
    fleet="$(viewer fleet 2>/dev/null || true)"
    if [[ -n "$fleet" ]] &&
      [[ "$(jq -r '.data.remotes | length' <<<"$fleet")" == "1" ]] &&
      [[ "$(jq -r '.data.remotes[0].latest.cpu_percent != null' <<<"$fleet")" == "true" ]]; then
      printf '%s' "$fleet"
      return 0
    fi
    sleep 0.25
  done
  echo "remote metric sample did not arrive within ten seconds" >&2
  return 1
}

paired_port="$(free_udp_port)"
moved_port="$(free_udp_port)"
[[ "$paired_port" != "$moved_port" ]]

start_viewer
start_server
wait_for_command "viewer daemon" viewer status
wait_for_command "monitored daemon" server status

# A fixed listen port is the supported way to keep a monitored machine's direct
# addresses stable across restarts.
configured="$(server listen-port set "$paired_port")"
[[ "$(jq -r '.data.restart_required' <<<"$configured")" == "true" ]]
stop_server
start_server
wait_for_command "monitored daemon on its configured port" server status
status="$(server status)"
[[ "$(jq -r '.data.bind_port' <<<"$status")" == "$paired_port" ]]
[[ "$(jq -r --arg port "$paired_port" '[.data.direct_addresses[] | endswith(":" + $port)] | all' <<<"$status")" == "true" ]]

bundle="$(server pairing create | jq -r '.data')"
[[ "$(jq -r '.ok' <<<"$(viewer pairing import "$bundle")")" == "true" ]]
fleet="$(wait_for_remote_sample)"
endpoint_id="$(jq -r '.data.remotes[0].endpoint_id' <<<"$fleet")"
paired_sequence="$(jq -r '.data.remotes[0].latest.sequence' <<<"$fleet")"
registry="$integration_root/viewer/data/monitored-machines.json"
paired_registry="$(jq -S . "$registry")"

# 1. A monitored daemon restart on its configured port recovers on its own.
stop_server
start_server
wait_for_command "restarted monitored daemon" server status
wait_for_sequence_beyond "$paired_sequence" 45
recovered_state="$(remote_field 'state')"
recovered_path="$(remote_field 'path')"
recovered_rtt="$(remote_field 'rtt_ms')"
recovered_last_seen="$(remote_field 'last_seen_ms')"
recovered_sequence="$(remote_field 'latest.sequence')"
[[ "$recovered_path" == "lan_direct" ]]
[[ "$recovered_rtt" != "null" ]]
[[ "$recovered_last_seen" != "null" ]]
[[ "$(jq -S . "$registry")" == "$paired_registry" ]]

# 2. An address this viewer cannot follow stays visibly offline, keeps the last
#    known values, and says what to do about it.
# The setting travels over local IPC, so it is changed while the daemon still
# runs and takes effect on the next start.
server listen-port set "$moved_port" >/dev/null
stop_server
start_server
wait_for_command "monitored daemon on its new port" server status
[[ "$(jq -r '.data.bind_port' <<<"$(server status)")" == "$moved_port" ]]
wait_for_state offline 60
offline_details="$(remote_field 'details | join(" ")')"
grep -q "listen-port set" <<<"$offline_details" || {
  echo "an unreachable machine must say how to recover, got: $offline_details" >&2
  exit 1
}
[[ "$(remote_field 'latest.cpu_percent')" != "null" ]]
[[ "$(remote_field 'latest.sequence')" == "$recovered_sequence" ]]
[[ "$(viewer peer list | jq -r '.data | length')" == "0" ]]
[[ "$(server peer list | jq -r '.data | length')" == "1" ]]
[[ "$(jq -r 'length' "$registry")" == "1" ]]
[[ "$(jq -r --arg id "$endpoint_id" 'has($id)' "$registry")" == "true" ]]

# 3. The machine returning to a known address recovers without re-pairing or a
#    hand-edited registry, and survives a viewer restart too.
server listen-port set "$paired_port" >/dev/null
stop_server
start_server
wait_for_command "monitored daemon back on its paired port" server status
wait_for_sequence_beyond "$recovered_sequence" 60
returned_sequence="$(remote_field 'latest.sequence')"

kill "$viewer_pid"
wait "$viewer_pid" 2>/dev/null || true
viewer_pid=""
start_viewer
wait_for_command "restarted viewer daemon" viewer status
[[ "$(remote_field 'latest.cpu_percent')" != "null" ]]
wait_for_sequence_beyond "$returned_sequence" 60
restarted_state="$(remote_field 'state')"
restarted_path="$(remote_field 'path')"
[[ "$(viewer peer list | jq -r '.data | length')" == "0" ]]
[[ "$(jq -r 'length' "$registry")" == "1" ]]

rackio_reject_matches \
  "the viewer persisted a one-time pairing secret" \
  "persisted pairing-secret scan" \
  grep -q -E 'one_time_secret' "$registry"
rackio_reject_matches \
  "daemon logs exposed a pairing bundle, secret, or metric payload" \
  "daemon sensitive-log scan" \
  grep -R -q -E 'rackio-pair:|one_time_secret|"cpu_percent"' \
  "$integration_root/viewer.log" "$integration_root/server.log" \
  "$integration_root/viewer/log" "$integration_root/server/log"

printf '%s\n' "$(jq -n \
  --arg recovered_state "$recovered_state" \
  --arg recovered_path "$recovered_path" \
  --argjson recovered_rtt_ms "$recovered_rtt" \
  --argjson recovered_last_seen_ms "$recovered_last_seen" \
  --argjson recovered_sequence "$recovered_sequence" \
  --arg offline_details "$offline_details" \
  --argjson returned_sequence "$returned_sequence" \
  --arg restarted_state "$restarted_state" \
  --arg restarted_path "$restarted_path" \
  '{
    configured_listen_port_is_stable: true,
    monitored_restart_recovered: true,
    recovered_state: $recovered_state,
    recovered_path: $recovered_path,
    recovered_rtt_ms: $recovered_rtt_ms,
    recovered_last_seen_ms: $recovered_last_seen_ms,
    recovered_sequence: $recovered_sequence,
    unreachable_address_reported_offline: true,
    unreachable_details: $offline_details,
    last_known_values_preserved: true,
    peer_allowlist_unchanged: true,
    recovered_without_repairing: true,
    returned_sequence: $returned_sequence,
    viewer_restart_state: $restarted_state,
    viewer_restart_path: $restarted_path
  }')"
