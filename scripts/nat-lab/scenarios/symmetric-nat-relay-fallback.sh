#!/usr/bin/env bash
# symmetric_nat_relay_fallback — a monitored machine behind a symmetric NAT,
# reached through an explicitly configured self-hosted relay.
#
# Proves: when the monitored machine's NAT gives it a different external port
# per destination, no direct path can be established, the session is carried by
# the operator's own relay, and the viewer reports the path as `relayed` rather
# than as a direct path.
#
# The machine is configured exactly as the hole-punching case would be — a
# fixed listen port and an operator-declared external address — so the only
# thing standing between it and a direct path is the NAT's mapping behaviour.
#
# This is also the scenario issue #19 cites for relay payload opacity: the
# relay's own interface is captured with full payloads and scanned for anything
# the viewer read out of the session.
set -euo pipefail

source "$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)/lib/lab.sh"

viewer="lan-e-viewer"
monitored="lan-f-monitored"
monitored_lan_address="192.168.106.10"
monitored_wan_address="192.0.2.7"
viewer_wan_address="192.0.2.6"
listen_port="41641"

lab_scenario_begin "symmetric_nat_relay_fallback" "relayed"
trap 'lab_scenario_finish' EXIT

lab_observe topology "$(jq -n \
  --arg lan "$monitored_lan_address" --arg wan "$monitored_wan_address" \
  --arg viewer_wan "$viewer_wan_address" --arg relay "$LAB_RELAY_ADDRESS" \
  --arg port "$listen_port" \
  '{description: "a symmetric-NAT machine and a cone-NAT viewer meeting only at the operator relay",
    internet: "net_internet 192.0.2.0/24 (RFC 5737 TEST-NET-1)",
    monitored: {service: "lan-f-monitored", lan: "lan_f 192.168.106.0/24",
                lan_address: $lan, default_gateway: "192.168.106.2",
                listen_port: $port, declared_external_address: ($wan + ":" + $port)},
    viewer: {service: "lan-e-viewer", lan: "lan_e 192.168.105.0/24",
             lan_address: "192.168.105.10", default_gateway: "192.168.105.2",
             observed_wan_address: $viewer_wan},
    routers: [
      {service: "router-f", nat_mode: "symmetric", wan_address: $wan,
       port_forward: null,
       behaviour: "iptables MASQUERADE --random-fully: a fresh external port per destination, so an address learned from one peer is useless to another"},
      {service: "router-e", nat_mode: "endpoint_independent",
       wan_address: $viewer_wan, port_forward: null}
    ],
    relay: {service: "relay", address: $relay,
            image: "relay-package/ built unchanged (upstream iroh-relay 1.0.3)"}}')"

lab_capture_start "router-f" "192.0.2.0/24"

lab_daemon_start "$viewer"
lab_daemon_start "$monitored"
lab_wait_for_command "viewer daemon" lab_rackio "$viewer" status
lab_wait_for_command "monitored daemon" lab_rackio "$monitored" status

# Configure the machine the way an operator behind NAT would: a fixed listen
# port, the external address the operator believes it has, and the relay.
lab_rackio "$monitored" listen-port set "$listen_port" >/dev/null
lab_rackio "$monitored" advertise-address add "$monitored_wan_address:$listen_port" >/dev/null
lab_use_relay "$monitored"
lab_use_relay "$viewer"
lab_daemon_stop "$monitored"
lab_daemon_stop "$viewer"
lab_daemon_start "$monitored"
lab_daemon_start "$viewer"
lab_wait_for_command "monitored daemon with a relay" lab_rackio "$monitored" status
lab_wait_for_command "viewer daemon with a relay" lab_rackio "$viewer" status

lab_relay_authorise "$monitored" "$viewer"
relay_before="$(lab_relay_byte_counters)"
lab_relay_capture_start

bundle="$(lab_rackio "$monitored" pairing create | jq -r '.data')"
imported="$(lab_rackio "$viewer" pairing import "$bundle")"
lab_assert_true "pairing_accepted" "the viewer imported the pairing bundle" \
  "$(jq -r '.ok' <<<"$imported")"

fleet="$(lab_wait_for_remote_sample "$viewer")"
endpoint_id="$(jq -r '.data.remotes[0].endpoint_id' <<<"$fleet")"
viewer_endpoint_id="$(lab_endpoint_id "$viewer")"

# Watch the path for half a minute rather than reading it once. iroh retries
# holepunching every five seconds, so this covers several attempts; reporting
# "never direct" from a single reading would only prove the lab was impatient.
observed_paths="$(lab_sample_path_of "$viewer" "$endpoint_id" 15 2)"
lab_settle_path_events

selected_path="$(lab_remote_field "$viewer" 'path')"
rtt_ms="$(lab_remote_field "$viewer" 'rtt_ms')"
state="$(lab_remote_field "$viewer" 'state')"
sequence="$(lab_remote_field "$viewer" 'latest.sequence')"
memory_total="$(lab_remote_field "$viewer" 'latest.memory_total_bytes')"
disk_total="$(lab_remote_field "$viewer" 'latest.disks[0].total_bytes')"
display_name="$(lab_remote_field "$viewer" 'node.display_name')"
node_id="$(lab_remote_field "$viewer" 'node.node_id')"
relay_url="$(lab_rackio "$viewer" status | jq -r '.data.relay_url')"
relay_mode="$(lab_relay_mode "$viewer")"
events="$(lab_observe_path_events "$viewer")"

# The needles are values this viewer really read out of this session. If the
# relay could read the payload, these are what it would see.
needles="$(jq -n \
  --arg name "$display_name" --arg node "$node_id" \
  --arg monitored_id "$endpoint_id" --arg viewer_id "$viewer_endpoint_id" \
  --argjson memory "$memory_total" --argjson disk "$disk_total" \
  '[{label: "monitored machine display name", kind: "utf8", value: $name, expected_visible: false},
    {label: "monitored machine node id", kind: "utf8", value: $node, expected_visible: false},
    {label: "memory_total_bytes the viewer displayed", kind: "u64", value: $memory, expected_visible: false},
    {label: "disk total_bytes the viewer displayed", kind: "u64", value: $disk, expected_visible: false},
    {label: "monitored endpoint id", kind: "hex", value: $monitored_id, expected_visible: true},
    {label: "viewer endpoint id", kind: "hex", value: $viewer_id, expected_visible: true}]')"
opacity="$(lab_relay_capture_opacity "symmetric_nat_relay_fallback" "$needles")"
# Recorded before the assertions run, so the report carries the scan even when
# an assertion on it does not hold.
lab_observe relay_payload_opacity "$opacity"

relay_after="$(lab_relay_byte_counters)"
packet_loss="$(lab_packet_loss "$viewer" "$LAB_RELAY_ADDRESS")"
# The relay's address is declared as an allowed peer because the relay really
# is on this wire; what must be absent is a direct flow between the two
# machines, and that is asserted below rather than assumed.
capture="$(lab_capture_finish "symmetric_nat_relay_fallback" \
  "$monitored_wan_address" "$viewer_wan_address" "$LAB_RELAY_ADDRESS")"

lab_observe_string selected_path "$selected_path"
lab_observe rtt_ms "$([[ "$rtt_ms" == "null" ]] && echo null || echo "$rtt_ms")"
lab_observe_string state "$state"
lab_observe_string endpoint_id "$endpoint_id"
lab_observe reconnect "$(jq -n '{applicable: false,
  note: "no reconnection is exercised here; see relay_outage and path_migration"}')"
lab_observe packet_loss "$packet_loss"
lab_observe observed_paths "$(jq -n --argjson paths "$observed_paths" \
  '{samples: $paths, interval_seconds: 2,
    note: "the path as the viewer reported it, sampled across several of iroh five-second holepunch retries"}')"
lab_observe capture "$capture"
lab_observe direct_path_attempts "$(jq -n --argjson capture "$capture" \
  --arg monitored "$monitored_wan_address" --arg viewer "$viewer_wan_address" \
  '{peer_to_peer_flow_seen_on_the_wire: $capture.expected_direct_flow_present,
    flow: ($monitored + " <-> " + $viewer),
    note: "UDP between the two NAT external addresses does appear here: both sides tried to open a direct path. It never became one, which is what the symmetric mapping is expected to cause, so the attempt is reported rather than hidden."}')"
lab_observe relay "$(jq -n --arg url "$relay_url" --arg mode "$relay_mode" \
  --argjson before "$relay_before" --argjson after "$relay_after" \
  --argjson config "$(lab_relay_rendered_config)" \
  '{configured_relay_url: (if $url == "null" then null else $url end),
    relay_mode: $mode, relays_running_in_lab: 1,
    tls: "none; the lab relay serves plain HTTP. See README.md for why, and for what it does and does not change.",
    byte_counters_before: $before, byte_counters_after: $after,
    bytes_relayed_during_session: (if ($before.available and $after.available)
      then {sent: ($after.bytes_sent - $before.bytes_sent),
            recv: ($after.bytes_recv - $before.bytes_recv)}
      else null end),
    rendered_config: $config}')"

lab_assert_equal "selected_path" \
  "a machine behind a symmetric NAT is reachable only through the relay" \
  "$selected_path" "relayed"
lab_assert_true "rtt_reported" "a live relayed path still reports a round-trip time" \
  "$([[ "$rtt_ms" != "null" ]] && echo true || echo false)"
lab_assert_true "machine_healthy" "the monitored machine reports live metrics over the relay" \
  "$([[ "$state" == "healthy" || "$state" == "warning" || "$state" == "critical" ]] && echo true || echo false)"
lab_assert_true "metrics_advanced_over_the_relay" \
  "the relayed session carried more than one sample" \
  "$([[ "$sequence" != "null" ]] && [[ "$sequence" -gt 1 ]] && echo true || echo false)"
lab_assert_equal "never_reported_a_direct_path" \
  "every path reading taken over thirty seconds was relayed" \
  "$(jq -r '[.[] | select(. != "relayed")] | length' <<<"$observed_paths")" "0"
lab_assert_equal "no_direct_path_event_was_ever_logged" \
  "the viewer's own log records no direct transport for this machine" \
  "$(jq -r '[.[] | select(.current_path == "LanDirect" or .current_path == "WanDirect")] | length' <<<"$events")" "0"
lab_assert_true "the_relay_really_carried_it" \
  "the relay's own byte counters grew while the session ran" \
  "$(jq -n --argjson before "$relay_before" --argjson after "$relay_after" \
    '($before.available and $after.available and ($after.bytes_recv > $before.bytes_recv))')"
lab_assert_relayed_never_reported_as_direct \
  "$selected_path" "$relay_url" "$relay_mode" "$capture" "$events" "$LAB_RELAY_URL"
lab_assert_relay_payload_opacity "$opacity"

lab_scenario_complete
