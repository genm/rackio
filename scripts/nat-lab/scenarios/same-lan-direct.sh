#!/usr/bin/env bash
# same_lan_direct — two machines on one LAN, no router between them.
#
# Proves: a viewer that pairs with a machine on its own LAN selects a direct
# path and reports it as `lan_direct`, with the capture showing the two
# machines' own packets and no third party.
set -euo pipefail

source "$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)/lib/lab.sh"

viewer="lan-a-viewer"
monitored="lan-a-monitored"
viewer_address="192.168.101.10"
monitored_address="192.168.101.11"

lab_scenario_begin "same_lan_direct" "lan_direct"
trap 'lab_scenario_finish' EXIT

lab_observe topology "$(jq -n \
  --arg viewer "$viewer_address" --arg monitored "$monitored_address" \
  '{description: "one flat LAN, no NAT router on the path",
    network: "lan_a 192.168.101.0/24 (docker internal)",
    viewer: {service: "lan-a-viewer", address: $viewer},
    monitored: {service: "lan-a-monitored", address: $monitored},
    routers: []}')"

lab_capture_start "$monitored" "192.168.101.0/24"

lab_daemon_start "$viewer"
lab_daemon_start "$monitored"
lab_wait_for_command "viewer daemon" lab_rackio "$viewer" status
lab_wait_for_command "monitored daemon" lab_rackio "$monitored" status

bundle="$(lab_rackio "$monitored" pairing create | jq -r '.data')"
imported="$(lab_rackio "$viewer" pairing import "$bundle")"
lab_assert_true "pairing_accepted" "the viewer imported the pairing bundle" \
  "$(jq -r '.ok' <<<"$imported")"

fleet="$(lab_wait_for_remote_sample "$viewer")"
lab_settle_path_events

selected_path="$(jq -r '.data.remotes[0].path' <<<"$fleet")"
rtt_ms="$(lab_remote_field "$viewer" 'rtt_ms')"
state="$(lab_remote_field "$viewer" 'state')"
endpoint_id="$(jq -r '.data.remotes[0].endpoint_id' <<<"$fleet")"
relay_url="$(lab_rackio "$viewer" status | jq -r '.data.relay_url')"
relay_mode="$(lab_relay_mode "$viewer")"
events="$(lab_observe_path_events "$viewer")"
packet_loss="$(lab_packet_loss "$viewer" "$monitored_address")"
capture="$(lab_capture_finish "same_lan_direct" "$viewer_address" "$monitored_address")"

lab_observe_string selected_path "$selected_path"
lab_observe rtt_ms "$([[ "$rtt_ms" == "null" ]] && echo null || echo "$rtt_ms")"
lab_observe_string state "$state"
lab_observe_string endpoint_id "$endpoint_id"
lab_observe reconnect "$(jq -n '{applicable: false,
  note: "no reconnection is exercised here; see the address_change scenario"}')"
lab_observe packet_loss "$packet_loss"
lab_observe capture "$capture"
lab_observe relay "$(jq -n --arg url "$relay_url" --arg mode "$relay_mode" \
  '{configured_relay_url: (if $url == "null" then null else $url end),
    relay_mode: $mode, relays_running_in_lab: 0}')"

lab_assert_equal "selected_path" "a peer on the same LAN is a LAN-direct path" \
  "$selected_path" "lan_direct"
lab_assert_true "rtt_reported" "a live direct path reports a round-trip time" \
  "$([[ "$rtt_ms" != "null" ]] && echo true || echo false)"
lab_assert_true "machine_healthy" "the monitored machine reports live metrics" \
  "$([[ "$state" == "healthy" || "$state" == "warning" || "$state" == "critical" ]] && echo true || echo false)"
lab_assert_relayed_never_reported_as_direct \
  "$selected_path" "$relay_url" "$relay_mode" "$capture" "$events"

lab_scenario_complete
