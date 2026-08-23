#!/usr/bin/env bash
# udp_blocked — UDP is dropped between the two peers.
#
# The checklist row does not say what the answer must be, and the answer
# depends on what is left reachable, so this scenario measures both cases
# rather than picking the flattering one.
#
# Phase 1  UDP allowed, relay running        -> wan_direct
# Phase 2  UDP dropped, relay running        -> relayed: the relay is reached
#          over TCP, so the session survives the block
# Phase 3  UDP dropped, relay stopped        -> offline, with the last known
#          values kept and the machine still registered. Nothing is reachable,
#          and the honest report of that is `offline`, not a stale `healthy`.
# Phase 4  UDP allowed, relay running        -> recovers
set -euo pipefail

source "$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)/lib/lab.sh"

viewer="lan-c-viewer"
monitored="lan-b-monitored"
monitored_wan_address="192.0.2.2"
viewer_wan_address="192.0.2.3"
forwarded_port="41641"

lab_scenario_begin "udp_blocked" "relayed while the relay is reachable, offline when it is not"
trap 'lab_scenario_finish' EXIT

lab_observe topology "$(jq -n \
  --arg wan "$monitored_wan_address" --arg viewer_wan "$viewer_wan_address" \
  --arg relay "$LAB_RELAY_ADDRESS" --arg port "$forwarded_port" \
  '{description: "a port-forwarded machine with UDP dropped at its router, first with the relay reachable and then without it",
    internet: "net_internet 192.0.2.0/24 (RFC 5737 TEST-NET-1)",
    monitored: {service: "lan-b-monitored", lan_address: "192.168.102.10",
                wan_address: $wan, listen_port: $port},
    viewer: {service: "lan-c-viewer", lan_address: "192.168.103.10",
             observed_wan_address: $viewer_wan},
    impairment: {where: "router-b FORWARD chain",
                 rule: "iptables -I FORWARD 1 -p udp -j DROP",
                 effect: "every UDP datagram across router-b is dropped in both directions; TCP is untouched"},
    relay: {service: "relay", address: $relay,
            transport: "TCP/80, so it is unaffected by the UDP block until it is stopped"}}')"

lab_capture_start "router-b" "192.0.2.0/24"

lab_daemon_start "$viewer"
lab_daemon_start "$monitored"
lab_wait_for_command "viewer daemon" lab_rackio "$viewer" status
lab_wait_for_command "monitored daemon" lab_rackio "$monitored" status

lab_rackio "$monitored" listen-port set "$forwarded_port" >/dev/null
lab_rackio "$monitored" advertise-address add "$monitored_wan_address:$forwarded_port" >/dev/null
lab_use_relay "$monitored"
lab_use_relay "$viewer"
lab_daemon_stop "$monitored"
lab_daemon_stop "$viewer"
lab_daemon_start "$monitored"
lab_daemon_start "$viewer"
lab_wait_for_command "monitored daemon with a relay" lab_rackio "$monitored" status
lab_wait_for_command "viewer daemon with a relay" lab_rackio "$viewer" status

lab_relay_authorise "$monitored" "$viewer"

bundle="$(lab_rackio "$monitored" pairing create | jq -r '.data')"
lab_assert_true "pairing_accepted" "the viewer imported the pairing bundle" \
  "$(jq -r '.ok' <<<"$(lab_rackio "$viewer" pairing import "$bundle")")"
fleet="$(lab_wait_for_remote_sample "$viewer")"
endpoint_id="$(jq -r '.data.remotes[0].endpoint_id' <<<"$fleet")"

# --- Phase 1: unimpaired ---------------------------------------------------
lab_wait_for_path_of "$viewer" "$endpoint_id" "wan_direct" 180
sequence_before_block="$(lab_remote_field_of "$viewer" "$endpoint_id" 'latest.sequence')"

# --- Phase 2: UDP blocked, relay still reachable over TCP -------------------
lab_block_udp "router-b"
blocked_started_ms="$(lab_now_ms)"
lab_wait_for_path_of "$viewer" "$endpoint_id" "relayed" 180
blocked_to_relay_ms=$(($(lab_now_ms) - blocked_started_ms))
lab_wait_for_sequence_beyond_of "$viewer" "$endpoint_id" "$sequence_before_block" 120
blocked_state="$(lab_remote_field_of "$viewer" "$endpoint_id" 'state')"
blocked_path="$(lab_remote_field_of "$viewer" "$endpoint_id" 'path')"
blocked_rtt="$(lab_remote_field_of "$viewer" "$endpoint_id" 'rtt_ms')"
blocked_sequence="$(lab_remote_field_of "$viewer" "$endpoint_id" 'latest.sequence')"
blocked_cpu="$(lab_remote_field_of "$viewer" "$endpoint_id" 'latest.cpu_percent')"
blocked_memory="$(lab_remote_field_of "$viewer" "$endpoint_id" 'latest.memory_used_bytes')"

# --- Phase 3: UDP blocked and the relay stopped: nothing is reachable -------
lab_relay_stop
lab_wait_for_state_of "$viewer" "$endpoint_id" offline 240
isolated_state="$(lab_remote_field_of "$viewer" "$endpoint_id" 'state')"
isolated_sequence="$(lab_remote_field_of "$viewer" "$endpoint_id" 'latest.sequence')"
isolated_cpu="$(lab_remote_field_of "$viewer" "$endpoint_id" 'latest.cpu_percent')"
isolated_memory="$(lab_remote_field_of "$viewer" "$endpoint_id" 'latest.memory_used_bytes')"
isolated_details="$(lab_remote_field_of "$viewer" "$endpoint_id" 'details | join(" ")')"
isolated_registry_entries="$(lab_registry "$viewer" 'length')"

# --- Phase 4: everything restored ------------------------------------------
lab_unblock_udp "router-b"
lab_relay_start
recovery_started_ms="$(lab_now_ms)"
# The machine has been unreachable on every path, so it reconnects rather than
# resuming; the viewer still holds the operator-declared address it was paired
# with, which is what it comes back on.
lab_daemon_stop "$monitored"
lab_daemon_start "$monitored"
lab_wait_for_command "restored monitored daemon" lab_rackio "$monitored" status
lab_wait_for_sequence_beyond_of "$viewer" "$endpoint_id" "$isolated_sequence" 180
recovery_ms=$(($(lab_now_ms) - recovery_started_ms))
lab_settle_path_events

selected_path="$(lab_remote_field_of "$viewer" "$endpoint_id" 'path')"
rtt_ms="$(lab_remote_field_of "$viewer" "$endpoint_id" 'rtt_ms')"
state="$(lab_remote_field_of "$viewer" "$endpoint_id" 'state')"
relay_url="$(lab_rackio "$viewer" status | jq -r '.data.relay_url')"
relay_mode="$(lab_relay_mode "$viewer")"
events="$(lab_observe_path_events "$viewer")"
relay_counters="$(lab_relay_byte_counters)"
packet_loss="$(lab_packet_loss "$viewer" "$monitored_wan_address")"
capture="$(lab_capture_finish "udp_blocked" \
  "$viewer_wan_address" "$monitored_wan_address" "$LAB_RELAY_ADDRESS")"

lab_observe_string selected_path "$selected_path"
lab_observe rtt_ms "$([[ "$rtt_ms" == "null" ]] && echo null || echo "$rtt_ms")"
lab_observe_string state "$state"
lab_observe_string endpoint_id "$endpoint_id"
lab_observe udp_blocked "$(jq -n \
  --arg path "$blocked_path" --arg state "$blocked_state" \
  --argjson rtt "$([[ "$blocked_rtt" == "null" ]] && echo null || echo "$blocked_rtt")" \
  --argjson sequence_before "$sequence_before_block" --argjson sequence_during "$blocked_sequence" \
  --argjson cpu "$blocked_cpu" --argjson memory "$blocked_memory" \
  --argjson elapsed "$blocked_to_relay_ms" \
  '{with_the_relay_reachable: {path: $path, state: $state, rtt_ms: $rtt,
     sequence: {before_the_block: $sequence_before, during_the_block: $sequence_during},
     last_sample: {cpu_percent: $cpu, memory_used_bytes: $memory},
     migration_ms: $elapsed,
     outcome: "the relay is reached over TCP, so the block moved the session onto it rather than ending it"}}')"
lab_observe isolated "$(jq -n \
  --arg state "$isolated_state" --arg details "$isolated_details" \
  --argjson sequence "$isolated_sequence" --argjson cpu "$isolated_cpu" \
  --argjson memory "$isolated_memory" --argjson registry "$isolated_registry_entries" \
  '{with_nothing_reachable: {state: $state, details: $details,
     frozen_sequence: $sequence,
     last_known_sample: {cpu_percent: $cpu, memory_used_bytes: $memory},
     registry_entries: $registry,
     outcome: "UDP dropped and the relay stopped leaves no path at all; the viewer reports offline and keeps what it last saw"}}')"
lab_observe reconnect "$(jq -n --argjson recovery "$recovery_ms" \
  '{applicable: true, recovery_ms: $recovery,
    measured_from: "the moment the UDP rule was removed and the relay was healthy again, to the first metric sequence past the frozen one"}')"
lab_observe packet_loss "$packet_loss"
lab_observe capture "$capture"
lab_observe relay "$(jq -n --arg url "$relay_url" --arg mode "$relay_mode" \
  --argjson counters "$relay_counters" \
  --argjson config "$(lab_relay_rendered_config)" \
  '{configured_relay_url: (if $url == "null" then null else $url end),
    relay_mode: $mode, relays_running_in_lab: 1,
    tls: "none; the lab relay serves plain HTTP. See README.md.",
    byte_counters_at_end: $counters, rendered_config: $config}')"

lab_assert_equal "blocked_udp_falls_back_to_the_relay" \
  "with UDP dropped and the relay reachable over TCP, the path is relayed" \
  "$blocked_path" "relayed"
lab_assert_true "monitoring_survived_the_block" \
  "fresh samples kept arriving over the relay while UDP was dropped" \
  "$([[ "$blocked_sequence" -gt "$sequence_before_block" ]] && echo true || echo false)"
lab_assert_true "blocked_machine_is_not_reported_offline" \
  "a machine still reachable through the relay is not reported as unreachable" \
  "$([[ "$blocked_state" == "healthy" || "$blocked_state" == "warning" || "$blocked_state" == "critical" ]] && echo true || echo false)"
lab_assert_equal "no_path_left_is_reported_offline" \
  "with UDP dropped and the relay stopped there is no path, and the viewer says so" \
  "$isolated_state" "offline"
lab_assert_true "isolated_machine_kept_its_last_known_values" \
  "an unreachable machine keeps its last sample instead of reading as zero" \
  "$(jq -n --argjson cpu "$isolated_cpu" --argjson memory "$isolated_memory" \
    '($cpu != null) and ($memory != null) and ($memory > 0)')"
lab_assert_equal "no_sample_was_invented_while_isolated" \
  "the frozen sample is the last one received over the relay" \
  "$isolated_sequence" "$blocked_sequence"
lab_assert_equal "isolated_machine_stayed_registered" \
  "losing every path does not unregister the machine" \
  "$isolated_registry_entries" "1"
lab_assert_true "recovered_after_the_block_was_lifted" \
  "the machine reports live metrics again once a path exists" \
  "$([[ "$state" == "healthy" || "$state" == "warning" || "$state" == "critical" ]] && echo true || echo false)"
lab_assert_relayed_never_reported_as_direct \
  "$selected_path" "$relay_url" "$relay_mode" "$capture" "$events" "$LAB_RELAY_URL"

lab_scenario_complete
