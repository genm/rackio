#!/usr/bin/env bash
# address_change — a monitored machine rebinds to a different address and port.
#
# The assertions are the ones scripts/test-two-daemon-address-change.sh already
# makes about the documented recovery behaviour; they are not re-invented here.
# What the container lab adds is a real address change: the host script can only
# move the port, while here the machine also loses its IP and takes another one,
# which is the case the "stale address" checklist row is about.
#
# Phase 1  restart on the paired address        -> recovers on its own
# Phase 2  move to an address the viewer cannot follow -> visibly offline, last
#          known values preserved, says how to recover, allowlist untouched
# Phase 3  return to the paired address         -> recovers without re-pairing,
#          and survives a viewer restart
set -euo pipefail

source "$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)/lib/lab.sh"

viewer="lan-a-viewer"
monitored="lan-a-monitored"
viewer_address="192.168.101.10"
paired_address="192.168.101.11"
moved_address="192.168.101.21"
paired_port="41651"
moved_port="41652"

lab_scenario_begin "address_change" "lan_direct"
trap 'lab_scenario_finish' EXIT

lab_observe topology "$(jq -n \
  --arg viewer "$viewer_address" --arg paired "$paired_address" \
  --arg moved "$moved_address" --arg paired_port "$paired_port" \
  --arg moved_port "$moved_port" \
  '{description: "one flat LAN; the monitored machine rebinds to a different address and port",
    network: "lan_a 192.168.101.0/24 (docker internal)",
    viewer: {service: "lan-a-viewer", address: $viewer},
    monitored: {service: "lan-a-monitored",
                paired_socket: ($paired + ":" + $paired_port),
                moved_socket: ($moved + ":" + $moved_port)},
    routers: []}')"

# Move the monitored machine's LAN address inside its own namespace. The docker
# network still owns the subnet; only the machine's address changes, which is
# what an address change looks like to a paired viewer.
move_address() {
  local from="$1" to="$2" interface
  interface="$(lab_capture_interface "$monitored" "192.168.101.0/24")"
  lab_exec "$monitored" ip addr del "$from/24" dev "$interface"
  lab_exec "$monitored" ip addr add "$to/24" dev "$interface"
}

lab_capture_start "$monitored" "192.168.101.0/24"

lab_daemon_start "$viewer"
lab_daemon_start "$monitored"
lab_wait_for_command "viewer daemon" lab_rackio "$viewer" status
lab_wait_for_command "monitored daemon" lab_rackio "$monitored" status

configured="$(lab_rackio "$monitored" listen-port set "$paired_port")"
lab_assert_equal "listen_port_restart_required" \
  "changing the listen port asks for a restart rather than silently drifting" \
  "$(jq -r '.data.restart_required' <<<"$configured")" "true"
lab_daemon_stop "$monitored"
lab_daemon_start "$monitored"
lab_wait_for_command "monitored daemon on its configured port" \
  lab_rackio "$monitored" status
status="$(lab_rackio "$monitored" status)"
lab_assert_equal "bind_port_configured" "the machine listens on its fixed port" \
  "$(jq -r '.data.bind_port' <<<"$status")" "$paired_port"
lab_assert_equal "direct_addresses_use_the_fixed_port" \
  "every advertised address uses the fixed port" \
  "$(jq -r --arg port "$paired_port" \
    '[.data.direct_addresses[] | endswith(":" + $port)] | all' <<<"$status")" "true"

bundle="$(lab_rackio "$monitored" pairing create | jq -r '.data')"
lab_assert_true "pairing_accepted" "the viewer imported the pairing bundle" \
  "$(jq -r '.ok' <<<"$(lab_rackio "$viewer" pairing import "$bundle")")"
fleet="$(lab_wait_for_remote_sample "$viewer")"
endpoint_id="$(jq -r '.data.remotes[0].endpoint_id' <<<"$fleet")"
paired_sequence="$(jq -r '.data.remotes[0].latest.sequence' <<<"$fleet")"
paired_registry="$(lab_registry "$viewer" '.')"

# --- Phase 1: a restart on the paired address recovers on its own -----------
lab_daemon_stop "$monitored"
lab_daemon_start "$monitored"
lab_wait_for_command "restarted monitored daemon" lab_rackio "$monitored" status
restart_started_ms="$(lab_now_ms)"
lab_wait_for_sequence_beyond "$viewer" "$paired_sequence" 60
restart_reconnect_ms=$(($(lab_now_ms) - restart_started_ms))

recovered_state="$(lab_remote_field "$viewer" 'state')"
recovered_path="$(lab_remote_field "$viewer" 'path')"
recovered_rtt="$(lab_remote_field "$viewer" 'rtt_ms')"
recovered_last_seen="$(lab_remote_field "$viewer" 'last_seen_ms')"
recovered_sequence="$(lab_remote_field "$viewer" 'latest.sequence')"
lab_assert_equal "restart_recovered_path" "the recovered path is still LAN direct" \
  "$recovered_path" "lan_direct"
lab_assert_true "restart_recovered_rtt" "the recovered path reports a round-trip time" \
  "$([[ "$recovered_rtt" != "null" ]] && echo true || echo false)"
lab_assert_true "restart_recovered_last_seen" "the viewer recorded a fresh last-seen time" \
  "$([[ "$recovered_last_seen" != "null" ]] && echo true || echo false)"
lab_assert_equal "registry_unchanged_by_restart" \
  "a restart on the paired address does not rewrite the viewer's registry" \
  "$(lab_registry "$viewer" '.')" "$paired_registry"

# --- Phase 2: an address the viewer cannot follow stays visibly offline -----
lab_rackio "$monitored" listen-port set "$moved_port" >/dev/null
lab_daemon_stop "$monitored"
move_address "$paired_address" "$moved_address"
lab_daemon_start "$monitored"
lab_wait_for_command "monitored daemon on its new address" \
  lab_rackio "$monitored" status
lab_assert_equal "moved_bind_port" "the machine really did move" \
  "$(lab_rackio "$monitored" status | jq -r '.data.bind_port')" "$moved_port"
lab_assert_equal "moved_address" "the machine really did change address" \
  "$(lab_rackio "$monitored" status |
    jq -r --arg a "$moved_address" '[.data.direct_addresses[] | startswith($a + ":")] | any')" \
  "true"

lab_wait_for_state "$viewer" offline 90
offline_details="$(lab_remote_field "$viewer" 'details | join(" ")')"
lab_assert_true "unreachable_machine_says_how_to_recover" \
  "an unreachable machine names the listen-port setting that fixes it" \
  "$(grep -q "listen-port set" <<<"$offline_details" && echo true || echo false)"
lab_assert_true "last_known_values_preserved" \
  "an offline machine keeps its last known sample instead of reading as zero" \
  "$([[ "$(lab_remote_field "$viewer" 'latest.cpu_percent')" != "null" ]] && echo true || echo false)"
lab_assert_equal "offline_sequence_frozen" \
  "no fresh sample is invented while the machine is unreachable" \
  "$(lab_remote_field "$viewer" 'latest.sequence')" "$recovered_sequence"
lab_assert_equal "viewer_grants_nothing" "the viewer authorized no peer of its own" \
  "$(lab_rackio "$viewer" peer list | jq -r '.data | length')" "0"
lab_assert_equal "monitored_allowlist_intact" \
  "the monitored machine still authorizes exactly the paired viewer" \
  "$(lab_rackio "$monitored" peer list | jq -r '.data | length')" "1"
lab_assert_equal "registry_still_holds_the_machine" \
  "the unreachable machine is still registered under its endpoint id" \
  "$(lab_registry "$viewer" "has(\"$endpoint_id\") and (length == 1)")" \
  "true"

# --- Phase 3: returning to a known address recovers without re-pairing ------
lab_rackio "$monitored" listen-port set "$paired_port" >/dev/null
lab_daemon_stop "$monitored"
move_address "$moved_address" "$paired_address"
lab_daemon_start "$monitored"
lab_wait_for_command "monitored daemon back on its paired address" \
  lab_rackio "$monitored" status
return_started_ms="$(lab_now_ms)"
lab_wait_for_sequence_beyond "$viewer" "$recovered_sequence" 90
return_reconnect_ms=$(($(lab_now_ms) - return_started_ms))
returned_sequence="$(lab_remote_field "$viewer" 'latest.sequence')"

lab_daemon_stop "$viewer"
lab_daemon_start "$viewer"
lab_wait_for_command "restarted viewer daemon" lab_rackio "$viewer" status
lab_assert_true "viewer_restart_kept_last_sample" \
  "a restarted viewer still shows the last known sample" \
  "$([[ "$(lab_remote_field "$viewer" 'latest.cpu_percent')" != "null" ]] && echo true || echo false)"
viewer_restart_started_ms="$(lab_now_ms)"
lab_wait_for_sequence_beyond "$viewer" "$returned_sequence" 90
viewer_restart_reconnect_ms=$(($(lab_now_ms) - viewer_restart_started_ms))

restarted_state="$(lab_remote_field "$viewer" 'state')"
restarted_path="$(lab_remote_field "$viewer" 'path')"
rtt_ms="$(lab_remote_field "$viewer" 'rtt_ms')"
relay_url="$(lab_rackio "$viewer" status | jq -r '.data.relay_url')"
relay_mode="$(lab_relay_mode "$viewer")"
events="$(lab_observe_path_events "$viewer")"
packet_loss="$(lab_packet_loss "$viewer" "$paired_address")"
capture="$(lab_capture_finish "address_change" \
  "$viewer_address" "$paired_address" "$moved_address")"

lab_observe_string selected_path "$restarted_path"
lab_observe rtt_ms "$([[ "$rtt_ms" == "null" ]] && echo null || echo "$rtt_ms")"
lab_observe_string state "$restarted_state"
lab_observe_string endpoint_id "$endpoint_id"
lab_observe reconnect "$(jq -n \
  --argjson restart "$restart_reconnect_ms" \
  --argjson ret "$return_reconnect_ms" \
  --argjson viewer_restart "$viewer_restart_reconnect_ms" \
  '{applicable: true,
    monitored_restart_ms: $restart,
    return_to_paired_address_ms: $ret,
    viewer_restart_ms: $viewer_restart,
    measured_from: "the moment the daemon accepted local IPC again, to the first metric sequence past the one held before"}')"
lab_observe packet_loss "$packet_loss"
lab_observe capture "$capture"
lab_observe relay "$(jq -n --arg url "$relay_url" --arg mode "$relay_mode" \
  '{configured_relay_url: (if $url == "null" then null else $url end),
    relay_mode: $mode, relays_running_in_lab: 0}')"
lab_observe_string offline_details "$offline_details"

lab_assert_equal "selected_path" "the recovered path is LAN direct" \
  "$restarted_path" "lan_direct"
lab_assert_true "recovered_without_repairing" \
  "the machine came back without a second pairing bundle" \
  "$([[ "$(lab_rackio "$viewer" peer list | jq -r '.data | length')" == "0" ]] && echo true || echo false)"
lab_assert_equal "registry_still_single_entry" \
  "recovery did not duplicate or drop the registry entry" \
  "$(lab_registry "$viewer" 'length')" "1"
# Unlike the steady-state scenarios, this one genuinely changes connection
# state: the session is lost and re-established three times. If the agent emits
# no structured event even here, the checklist's path-transition evidence does
# not exist at all, which the report must fail on rather than note in passing.
lab_assert_true "path_transition_events_emitted" \
  "the agent emitted structured connection events across the changes" \
  "$([[ "$(jq 'length' <<<"$events")" != "0" ]] && echo true || echo false)"
# Every recovery must be legible on its own. The path here stays `lan_direct`
# throughout, so a count of *path changes* would be zero even when recovery
# worked — the resumptions are what carry the evidence, one per recovery, and
# the initial pairing session may add one more.
lab_assert_true "each_recovery_announced_a_resumed_session" \
  "three recoveries produced at least three session-established events" \
  "$([[ "$(jq '[.[] | select(.event == "remote monitoring session established")] | length' <<<"$events")" -ge 3 ]] && echo true || echo false)"
lab_assert_relayed_never_reported_as_direct \
  "$restarted_path" "$relay_url" "$relay_mode" "$capture" "$events"

lab_scenario_complete
