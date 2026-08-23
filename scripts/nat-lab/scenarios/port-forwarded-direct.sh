#!/usr/bin/env bash
# port_forwarded_direct — a monitored machine behind NAT, published through a
# DNAT port forward to its fixed listen port, reached by a viewer behind a
# different NAT across the shared "internet" segment.
#
# Proves: `rackio listen-port set` gives a machine behind NAT a stable,
# forwardable port, and a session that crosses two NATs over a non-RFC-1918
# segment is carried directly and reported as `wan_direct`.
set -euo pipefail

source "$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)/lib/lab.sh"

viewer="lan-c-viewer"
monitored="lan-b-monitored"
monitored_lan_address="192.168.102.10"
monitored_wan_address="192.0.2.2"
viewer_wan_address="192.0.2.3"
# Must match NAT_PORT_FORWARDS for router-b in compose.yaml.
forwarded_port="41641"

lab_scenario_begin "port_forwarded_direct" "wan_direct"
trap 'lab_scenario_finish' EXIT

lab_observe topology "$(jq -n \
  --arg lan "$monitored_lan_address" --arg wan "$monitored_wan_address" \
  --arg viewer_wan "$viewer_wan_address" --arg port "$forwarded_port" \
  '{description: "two NATed LANs meeting on a shared internet segment",
    internet: "net_internet 192.0.2.0/24 (RFC 5737 TEST-NET-1, not RFC 1918, so a path across it is wan_direct)",
    monitored: {service: "lan-b-monitored", lan: "lan_b 192.168.102.0/24",
                lan_address: $lan, default_gateway: "192.168.102.2"},
    viewer: {service: "lan-c-viewer", lan: "lan_c 192.168.103.0/24",
             lan_address: "192.168.103.10", default_gateway: "192.168.103.2",
             observed_wan_address: $viewer_wan},
    routers: [
      {service: "router-b", nat_mode: "endpoint_independent",
       wan_address: $wan,
       port_forward: ("udp " + $wan + ":" + $port + " -> " + $lan + ":" + $port)},
      {service: "router-c", nat_mode: "endpoint_independent",
       wan_address: $viewer_wan, port_forward: null}
    ]}')"

lab_capture_start "router-b" "192.0.2.0/24"

lab_daemon_start "$viewer"
lab_daemon_start "$monitored"
lab_wait_for_command "viewer daemon" lab_rackio "$viewer" status
lab_wait_for_command "monitored daemon" lab_rackio "$monitored" status

# A fixed listen port is the supported way to keep a machine's direct address
# stable, and it is what the router's port forward is aimed at.
configured="$(lab_rackio "$monitored" listen-port set "$forwarded_port")"
lab_assert_equal "listen_port_restart_required" \
  "changing the listen port asks for a restart rather than silently drifting" \
  "$(jq -r '.data.restart_required' <<<"$configured")" "true"
lab_daemon_stop "$monitored"
lab_daemon_start "$monitored"
lab_wait_for_command "monitored daemon on its forwarded port" \
  lab_rackio "$monitored" status

status="$(lab_rackio "$monitored" status)"
lab_assert_equal "bind_port_is_the_forwarded_port" \
  "the machine listens on exactly the port the router forwards" \
  "$(jq -r '.data.bind_port' <<<"$status")" "$forwarded_port"
lab_assert_equal "direct_addresses_use_the_fixed_port" \
  "every advertised address uses the fixed port" \
  "$(jq -r --arg port "$forwarded_port" \
    '[.data.direct_addresses[] | endswith(":" + $port)] | all' <<<"$status")" "true"

bundle="$(lab_rackio "$monitored" pairing create | jq -r '.data')"
advertised="$(node "$lab_dir/lib/rewrite-bundle-addresses.mjs" --read "$bundle")"
# The bundle advertises the machine's own interface addresses, which are behind
# the NAT and unreachable from lan_c. Substitute the forwarded address the
# operator would hand over. See the README: this stands in for a product gap.
forwarded_bundle="$(node "$lab_dir/lib/rewrite-bundle-addresses.mjs" \
  "$bundle" "$monitored_wan_address:$forwarded_port")"

lab_observe bundle_addresses "$(jq -n \
  --argjson advertised "$advertised" \
  --arg substituted "$monitored_wan_address:$forwarded_port" \
  '{advertised_by_pairing_create: $advertised,
    substituted_for_the_viewer: [$substituted],
    substitution_performed: true,
    reason: "rackio fills direct_addresses from local interfaces only; in direct-only mode nothing discovers the router WAN address, and no CLI accepts one. The lab substitutes the forwarded address so this scenario tests the transport, not the missing UX."}')"

imported="$(lab_rackio "$viewer" pairing import "$forwarded_bundle")"
lab_assert_true "pairing_accepted" \
  "the viewer paired across two NATs using the forwarded address" \
  "$(jq -r '.ok' <<<"$imported")"

fleet="$(lab_wait_for_remote_sample "$viewer")"
lab_settle_path_events

selected_path="$(jq -r '.data.remotes[0].path' <<<"$fleet")"
rtt_ms="$(lab_remote_field "$viewer" 'rtt_ms')"
state="$(lab_remote_field "$viewer" 'state')"
relay_url="$(lab_rackio "$viewer" status | jq -r '.data.relay_url')"
relay_mode="$(lab_relay_mode "$viewer")"
events="$(lab_observe_path_events "$viewer")"
packet_loss="$(lab_packet_loss "$viewer" "$monitored_wan_address")"
capture="$(lab_capture_finish "port_forwarded_direct" \
  "$viewer_wan_address" "$monitored_wan_address")"

lab_observe_string selected_path "$selected_path"
lab_observe rtt_ms "$([[ "$rtt_ms" == "null" ]] && echo null || echo "$rtt_ms")"
lab_observe_string state "$state"
lab_observe reconnect "$(jq -n '{applicable: false,
  note: "no reconnection is exercised here; see the address_change scenario"}')"
lab_observe packet_loss "$packet_loss"
lab_observe capture "$capture"
lab_observe relay "$(jq -n --arg url "$relay_url" --arg mode "$relay_mode" \
  '{configured_relay_url: (if $url == "null" then null else $url end),
    relay_mode: $mode, relays_running_in_lab: 0}')"

lab_assert_equal "selected_path" \
  "a forwarded path across the non-private internet segment is wan_direct" \
  "$selected_path" "wan_direct"
lab_assert_true "rtt_reported" "a live direct path reports a round-trip time" \
  "$([[ "$rtt_ms" != "null" ]] && echo true || echo false)"
lab_assert_true "machine_healthy" "the monitored machine reports live metrics" \
  "$([[ "$state" == "healthy" || "$state" == "warning" || "$state" == "critical" ]] && echo true || echo false)"
lab_assert_relayed_never_reported_as_direct \
  "$selected_path" "$relay_url" "$relay_mode" "$capture" "$events"

lab_scenario_complete
