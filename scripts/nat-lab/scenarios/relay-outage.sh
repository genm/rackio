#!/usr/bin/env bash
# relay_outage — the relay is stopped and started again while one viewer is
# watching two machines at once: one reached directly, one reachable only
# through the relay.
#
# Both machines are configured identically apart from what their NAT allows, so
# the outage is the only variable between them. That is what makes the result
# say something: a relay outage must be invisible to the direct machine and
# visible, honestly, on the relayed one.
#
# Proves: stopping the relay leaves the direct machine live and unchanged, puts
# the relay-only machine into `offline` with its last known values kept rather
# than zeroed, and starting the relay again recovers it without re-pairing.
set -euo pipefail

source "$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)/lib/lab.sh"

viewer="lan-c-viewer"
direct_machine="lan-b-monitored"
relayed_machine="lan-f-monitored"
direct_wan_address="192.0.2.2"
viewer_wan_address="192.0.2.3"
relayed_wan_address="192.0.2.7"
forwarded_port="41641"

lab_scenario_begin "relay_outage" "wan_direct and relayed"
trap 'lab_scenario_finish' EXIT

lab_observe topology "$(jq -n \
  --arg direct "$direct_wan_address" --arg relayed "$relayed_wan_address" \
  --arg viewer_wan "$viewer_wan_address" --arg relay "$LAB_RELAY_ADDRESS" \
  --arg port "$forwarded_port" \
  '{description: "one viewer, two machines: one behind a port forward, one behind a symmetric NAT, both pointed at the same relay",
    internet: "net_internet 192.0.2.0/24 (RFC 5737 TEST-NET-1)",
    viewer: {service: "lan-c-viewer", lan_address: "192.168.103.10",
             default_gateway: "192.168.103.2", observed_wan_address: $viewer_wan},
    direct_machine: {service: "lan-b-monitored", lan_address: "192.168.102.10",
                     wan_address: $direct, listen_port: $port,
                     reachable_because: ("router-b forwards udp/" + $port)},
    relayed_machine: {service: "lan-f-monitored", lan_address: "192.168.106.10",
                      wan_address: $relayed,
                      reachable_because: "nothing; router-f is symmetric and has no port forward, so only the relay can carry it"},
    relay: {service: "relay", address: $relay,
            outage: "docker compose stop relay, then start relay"}}')"

lab_capture_start "router-c" "192.0.2.0/24"

for machine in "$viewer" "$direct_machine" "$relayed_machine"; do
  lab_daemon_start "$machine"
done
for machine in "$viewer" "$direct_machine" "$relayed_machine"; do
  lab_wait_for_command "$machine daemon" lab_rackio "$machine" status
done

lab_rackio "$direct_machine" listen-port set "$forwarded_port" >/dev/null
lab_rackio "$direct_machine" advertise-address add "$direct_wan_address:$forwarded_port" >/dev/null
lab_use_relay "$direct_machine"
lab_use_relay "$relayed_machine"
lab_use_relay "$viewer"
for machine in "$viewer" "$direct_machine" "$relayed_machine"; do
  lab_daemon_stop "$machine"
  lab_daemon_start "$machine"
done
for machine in "$viewer" "$direct_machine" "$relayed_machine"; do
  lab_wait_for_command "$machine daemon with a relay" lab_rackio "$machine" status
done

lab_relay_authorise "$viewer" "$direct_machine" "$relayed_machine"

direct_id="$(lab_endpoint_id "$direct_machine")"
relayed_id="$(lab_endpoint_id "$relayed_machine")"
for machine in "$direct_machine" "$relayed_machine"; do
  bundle="$(lab_rackio "$machine" pairing create | jq -r '.data')"
  lab_assert_true "pairing_accepted_for_$machine" \
    "the viewer imported the pairing bundle" \
    "$(jq -r '.ok' <<<"$(lab_rackio "$viewer" pairing import "$bundle")")"
done

lab_wait_for_remote_sample_of "$viewer" "$direct_id"
lab_wait_for_remote_sample_of "$viewer" "$relayed_id"
lab_wait_for_path_of "$viewer" "$direct_id" "wan_direct" 180
lab_wait_for_path_of "$viewer" "$relayed_id" "relayed" 180
lab_assert_equal "two_machines_are_being_watched" \
  "the outage is only meaningful with both machines live at the same time" \
  "$(lab_rackio "$viewer" fleet | jq -r '.data.remotes | length')" "2"

# --- The outage ------------------------------------------------------------
direct_sequence_before="$(lab_remote_field_of "$viewer" "$direct_id" 'latest.sequence')"
relayed_sequence_before="$(lab_remote_field_of "$viewer" "$relayed_id" 'latest.sequence')"
relayed_cpu_before="$(lab_remote_field_of "$viewer" "$relayed_id" 'latest.cpu_percent')"
relayed_memory_before="$(lab_remote_field_of "$viewer" "$relayed_id" 'latest.memory_used_bytes')"

lab_relay_stop
lab_assert_true "relay_is_really_stopped" \
  "the outage is a stopped container, not a configuration change" \
  "$(lab_relay_running && echo false || echo true)"

# The direct machine must not notice. Proven by a fresh sample arriving while
# the relay is down, not by the absence of a complaint.
lab_wait_for_sequence_beyond_of "$viewer" "$direct_id" "$direct_sequence_before" 60
direct_state_during="$(lab_remote_field_of "$viewer" "$direct_id" 'state')"
direct_path_during="$(lab_remote_field_of "$viewer" "$direct_id" 'path')"

lab_wait_for_state_of "$viewer" "$relayed_id" offline 180
relayed_state_during="$(lab_remote_field_of "$viewer" "$relayed_id" 'state')"
relayed_cpu_during="$(lab_remote_field_of "$viewer" "$relayed_id" 'latest.cpu_percent')"
relayed_memory_during="$(lab_remote_field_of "$viewer" "$relayed_id" 'latest.memory_used_bytes')"
relayed_sequence_during="$(lab_remote_field_of "$viewer" "$relayed_id" 'latest.sequence')"
relayed_details="$(lab_remote_field_of "$viewer" "$relayed_id" 'details | join(" ")')"

# The direct machine has to still be healthy at the end of the outage too, not
# only at the start of it.
direct_state_after_outage="$(lab_remote_field_of "$viewer" "$direct_id" 'state')"

# --- Recovery --------------------------------------------------------------
lab_relay_start
recovery_started_ms="$(lab_now_ms)"
lab_wait_for_sequence_beyond_of "$viewer" "$relayed_id" "$relayed_sequence_during" 180
recovery_ms=$(($(lab_now_ms) - recovery_started_ms))
lab_wait_for_path_of "$viewer" "$relayed_id" "relayed" 60
relayed_state_after="$(lab_remote_field_of "$viewer" "$relayed_id" 'state')"
relayed_path_after="$(lab_remote_field_of "$viewer" "$relayed_id" 'path')"
direct_path_after="$(lab_remote_field_of "$viewer" "$direct_id" 'path')"
direct_state_after="$(lab_remote_field_of "$viewer" "$direct_id" 'state')"
lab_settle_path_events

relay_url="$(lab_rackio "$viewer" status | jq -r '.data.relay_url')"
relay_mode="$(lab_relay_mode "$viewer")"
events="$(lab_observe_path_events "$viewer")"
relay_counters="$(lab_relay_byte_counters)"
packet_loss="$(lab_packet_loss "$viewer" "$direct_wan_address")"
capture="$(lab_capture_finish "relay_outage" \
  "$viewer_wan_address" "$direct_wan_address" "$LAB_RELAY_ADDRESS" "$relayed_wan_address")"

lab_observe_string selected_path "$direct_path_after and $relayed_path_after"
lab_observe rtt_ms "$(jq -n \
  --arg direct "$(lab_remote_field_of "$viewer" "$direct_id" 'rtt_ms')" \
  --arg relayed "$(lab_remote_field_of "$viewer" "$relayed_id" 'rtt_ms')" \
  '{direct_machine: (if $direct == "null" then null else ($direct | tonumber) end),
    relayed_machine: (if $relayed == "null" then null else ($relayed | tonumber) end)}')"
lab_observe_string state "$direct_state_after and $relayed_state_after"
lab_observe endpoint_id "$(jq -n --arg d "$direct_id" --arg r "$relayed_id" \
  '{direct_machine: $d, relayed_machine: $r}')"
lab_observe outage "$(jq -n \
  --arg direct_state "$direct_state_during" --arg direct_path "$direct_path_during" \
  --arg direct_end "$direct_state_after_outage" \
  --arg relayed_state "$relayed_state_during" --arg details "$relayed_details" \
  --argjson cpu_before "$relayed_cpu_before" --argjson cpu_during "$relayed_cpu_during" \
  --argjson memory_before "$relayed_memory_before" --argjson memory_during "$relayed_memory_during" \
  --argjson sequence_before "$relayed_sequence_before" \
  --argjson sequence_during "$relayed_sequence_during" \
  '{direct_machine: {state_during_outage: $direct_state, path_during_outage: $direct_path,
                     state_at_end_of_outage: $direct_end},
    relayed_machine: {state_during_outage: $relayed_state, details: $details,
                      last_known_cpu_percent: {before: $cpu_before, during: $cpu_during},
                      last_known_memory_used_bytes: {before: $memory_before, during: $memory_during},
                      sequence: {before: $sequence_before, during: $sequence_during}}}')"
lab_observe reconnect "$(jq -n --argjson recovery "$recovery_ms" \
  '{applicable: true, relayed_machine_recovery_ms: $recovery,
    measured_from: "the moment the relay container was healthy again, to the first metric sequence past the one frozen during the outage"}')"
lab_observe packet_loss "$packet_loss"
lab_observe capture "$capture"
lab_observe relay "$(jq -n --arg url "$relay_url" --arg mode "$relay_mode" \
  --argjson counters "$relay_counters" \
  --argjson config "$(lab_relay_rendered_config)" \
  '{configured_relay_url: (if $url == "null" then null else $url end),
    relay_mode: $mode, relays_running_in_lab: 1,
    tls: "none; the lab relay serves plain HTTP. See README.md.",
    byte_counters_at_end: $counters, rendered_config: $config}')"

lab_assert_equal "direct_machine_unaffected_by_the_outage" \
  "a machine on a direct path keeps reporting while the relay is down" \
  "$direct_state_during" "healthy"
lab_assert_equal "direct_machine_stayed_direct" \
  "the direct machine did not silently move onto a relay that was not there" \
  "$direct_path_during" "wan_direct"
lab_assert_equal "direct_machine_still_healthy_at_the_end_of_the_outage" \
  "the direct machine was live for the whole outage, not just its first seconds" \
  "$direct_state_after_outage" "healthy"
lab_assert_equal "relayed_machine_went_offline" \
  "a machine that only the relay could reach is reported offline, not healthy" \
  "$relayed_state_during" "offline"
lab_assert_true "relayed_machine_kept_its_last_known_values" \
  "an offline machine keeps its last sample instead of reading as zero" \
  "$(jq -n --argjson cpu "$relayed_cpu_during" --argjson memory "$relayed_memory_during" \
    '($cpu != null) and ($memory != null) and ($memory > 0)')"
lab_assert_equal "no_sample_was_invented_during_the_outage" \
  "the frozen sample is the one that was last received, not a fresh one" \
  "$relayed_sequence_during" "$relayed_sequence_before"
lab_assert_equal "relayed_machine_recovered" \
  "the machine came back when the relay did" \
  "$relayed_state_after" "healthy"
lab_assert_equal "relayed_machine_is_still_relayed" \
  "recovery did not turn a relayed machine into a direct one" \
  "$relayed_path_after" "relayed"
lab_assert_equal "recovery_needed_no_repairing" \
  "the viewer authorized no new peer to get the machine back" \
  "$(lab_rackio "$viewer" peer list | jq -r '.data | length')" "0"
# Two machines share one event log, so the direct machine's claim is checked
# against its own events. Reading the whole log here would let the relayed
# machine's last transition decide whether the direct one is credible.
lab_assert_relayed_never_reported_as_direct \
  "$direct_path_after" "$relay_url" "$relay_mode" "$capture" \
  "$(jq --arg id "$direct_id" '[.[] | select(.endpoint_id == $id)]' <<<"$events")" \
  "$LAB_RELAY_URL"

lab_scenario_complete
