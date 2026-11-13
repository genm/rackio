#!/usr/bin/env bash
# cone_nat_hole_punch — both machines behind endpoint-independent NAT, with no
# port forward on either router, reaching each other over a direct path.
#
# Proves: when each operator declares the NAT-mapped address their machine
# cannot observe, that address becomes a real traversal candidate, and two
# machines that are individually unreachable from the outside open a direct
# path between them and report it as `wan_direct`.
#
# What makes this a hole punch rather than the `port_forwarded_direct` case:
# neither router has a DNAT rule, which the scenario reads back from the
# routers themselves rather than trusting compose.yaml. The only inbound state
# either NAT has is the mapping its own machine created by sending outbound, so
# a direct flow between the two external addresses can only exist because both
# sides punched one open.
#
# The relay is configured on both machines and carries the address exchange.
# That is what a relay is for in a hole punch, and it is not what carries the
# session: the assertions below require the selected path to be direct, the
# capture to show the two external addresses talking to each other, and the
# viewer to have logged the promotion off the relay.
set -euo pipefail

source "$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)/lib/lab.sh"

viewer="lan-e-viewer"
monitored="lan-d-monitored"
monitored_lan_address="192.168.104.10"
monitored_wan_address="192.0.2.5"
viewer_lan_address="192.168.105.10"
viewer_wan_address="192.0.2.6"
# An endpoint-independent MASQUERADE keeps the source port when it is free, so
# a machine that pins its listen port keeps the same external port whoever it
# talks to. That is the property the operator is relying on when they declare
# the mapped address, and the reason both sides pin a port here.
listen_port="41641"

lab_scenario_begin "cone_nat_hole_punch" "wan_direct"
trap 'lab_scenario_finish' EXIT

lab_observe topology "$(jq -n \
  --arg lan "$monitored_lan_address" --arg wan "$monitored_wan_address" \
  --arg viewer_lan "$viewer_lan_address" --arg viewer_wan "$viewer_wan_address" \
  --arg relay "$LAB_RELAY_ADDRESS" --arg port "$listen_port" \
  '{description: "two cone-NAT machines with no port forward on either side, each declaring the address its own NAT maps it to",
    internet: "net_internet 192.0.2.0/24 (RFC 5737 TEST-NET-1, not RFC 1918, so a path across it is wan_direct)",
    monitored: {service: "lan-d-monitored", lan: "lan_d 192.168.104.0/24",
                lan_address: $lan, default_gateway: "192.168.104.2",
                listen_port: $port, declared_external_address: ($wan + ":" + $port)},
    viewer: {service: "lan-e-viewer", lan: "lan_e 192.168.105.0/24",
             lan_address: $viewer_lan, default_gateway: "192.168.105.2",
             listen_port: $port, declared_external_address: ($viewer_wan + ":" + $port)},
    routers: [
      {service: "router-d", nat_mode: "endpoint_independent", wan_address: $wan,
       port_forward: null,
       behaviour: "iptables MASQUERADE with no --random-fully and no DNAT: one external port per internal socket, and nothing inbound that the machine did not itself open"},
      {service: "router-e", nat_mode: "endpoint_independent",
       wan_address: $viewer_wan, port_forward: null,
       behaviour: "identical to router-d"}
    ],
    relay: {service: "relay", address: $relay,
            role: "carries the address exchange only; the session itself is asserted to be direct"}}')"

# Captured on the monitored side WAN interface, which is where a punched path
# has to appear if it exists at all.
lab_capture_start "router-d" "192.0.2.0/24"

lab_daemon_start "$viewer"
lab_daemon_start "$monitored"
lab_wait_for_command "viewer daemon" lab_rackio "$viewer" status
lab_wait_for_command "monitored daemon" lab_rackio "$monitored" status

# Both sides are configured the same way, because in a hole punch both sides
# are the NATed one. A machine with an ephemeral port would get a fresh
# external port on every restart and the operator could not declare it.
lab_rackio "$monitored" listen-port set "$listen_port" >/dev/null
lab_rackio "$viewer" listen-port set "$listen_port" >/dev/null
monitored_advertise="$(lab_rackio "$monitored" advertise-address add \
  "$monitored_wan_address:$listen_port")"
viewer_advertise="$(lab_rackio "$viewer" advertise-address add \
  "$viewer_wan_address:$listen_port")"
lab_assert_true "monitored_advertised_address_saved" \
  "the monitored machine's mapped address was accepted as configuration" \
  "$(jq -r '.ok' <<<"$monitored_advertise")"
lab_assert_true "viewer_advertised_address_saved" \
  "the viewer's mapped address was accepted as configuration" \
  "$(jq -r '.ok' <<<"$viewer_advertise")"
lab_use_relay "$monitored"
lab_use_relay "$viewer"
lab_daemon_stop "$monitored"
lab_daemon_stop "$viewer"
lab_daemon_start "$monitored"
lab_daemon_start "$viewer"
lab_wait_for_command "monitored daemon on its declared address" \
  lab_rackio "$monitored" status
lab_wait_for_command "viewer daemon on its declared address" \
  lab_rackio "$viewer" status

# The routers are read back rather than taken on trust. If either grew a DNAT
# rule, the direct path below would be a port forward and this scenario would
# be `port_forwarded_direct` under another name.
monitored_nat_rules="$(lab_exec "router-d" iptables -t nat -S)"
viewer_nat_rules="$(lab_exec "router-e" iptables -t nat -S)"
monitored_input_rules="$(lab_exec "router-d" iptables -S INPUT)"
viewer_input_rules="$(lab_exec "router-e" iptables -S INPUT)"
lab_observe nat_rules "$(jq -n \
  --arg monitored "$monitored_nat_rules" --arg viewer "$viewer_nat_rules" \
  --arg monitored_input "$monitored_input_rules" --arg viewer_input "$viewer_input_rules" \
  '{"router-d": {nat: ($monitored | split("\n")), input: ($monitored_input | split("\n"))},
    "router-e": {nat: ($viewer | split("\n")), input: ($viewer_input | split("\n"))},
    source: "iptables -t nat -S and iptables -S INPUT, read from each router while the scenario ran",
    note: "no DNAT rule on either side is what separates a hole punch from a port forward. The INPUT rule drops unsolicited inbound UDP addressed to the router itself, which is what a NAT device does; without it the router host stack answers the punch probe and steals the external port its own machine needs. See README.md."}')"
lab_assert_equal "no_port_forward_on_the_monitored_side" \
  "router-d publishes nothing inbound; any inbound state is what the machine itself opened" \
  "$(grep -c -- '-j DNAT' <<<"$monitored_nat_rules" || true)" "0"
lab_assert_equal "no_port_forward_on_the_viewer_side" \
  "router-e publishes nothing inbound either" \
  "$(grep -c -- '-j DNAT' <<<"$viewer_nat_rules" || true)" "0"

# Acceptance criterion: the operator can see from the machine that the setting
# reached the endpoint, without decoding a pairing bundle.
monitored_status="$(lab_rackio "$monitored" status)"
viewer_status="$(lab_rackio "$viewer" status)"
lab_observe declared_addresses "$(jq -n \
  --argjson monitored "$(jq '.data.direct_addresses' <<<"$monitored_status")" \
  --argjson viewer "$(jq '.data.direct_addresses' <<<"$viewer_status")" \
  --arg monitored_declared "$monitored_wan_address:$listen_port" \
  --arg viewer_declared "$viewer_wan_address:$listen_port" \
  '{monitored: {declared: $monitored_declared, status_direct_addresses: $monitored},
    viewer: {declared: $viewer_declared, status_direct_addresses: $viewer},
    note: "the machine cannot observe its NAT-mapped address on any interface, so its presence here is the configured address reaching the endpoint"}')"
lab_assert_true "monitored_status_reports_the_declared_address" \
  "rackio status shows the operator-declared address among the endpoint's direct addresses" \
  "$(jq -r --arg wanted "$monitored_wan_address:$listen_port" \
    '[.data.direct_addresses[] | . == $wanted] | any' <<<"$monitored_status")"
lab_assert_true "viewer_status_reports_the_declared_address" \
  "the viewer's own declared address reached its endpoint too" \
  "$(jq -r --arg wanted "$viewer_wan_address:$listen_port" \
    '[.data.direct_addresses[] | . == $wanted] | any' <<<"$viewer_status")"

lab_relay_authorise "$monitored" "$viewer"
relay_before="$(lab_relay_byte_counters)"

bundle="$(lab_rackio "$monitored" pairing create | jq -r '.data')"
lab_assert_true "pairing_accepted" \
  "the viewer imported the pairing bundle across two NATs" \
  "$(jq -r '.ok' <<<"$(lab_rackio "$viewer" pairing import "$bundle")")"
fleet="$(lab_wait_for_remote_sample "$viewer")"
endpoint_id="$(jq -r '.data.remotes[0].endpoint_id' <<<"$fleet")"
first_observed_path="$(jq -r '.data.remotes[0].path' <<<"$fleet")"

# A session with a relay available may be carried by it while the punch is
# still being attempted, so the direct path is waited for rather than read
# once. The bound is thirty of iroh's five-second holepunch retries, and it is
# deliberately inside the capture window: a punch that landed after the capture
# stopped would be asserted against evidence that could not contain it. If the
# punch never lands this fails here, with the report recording what was seen.
punch_started_ms="$(lab_now_ms)"
lab_wait_for_path_of "$viewer" "$endpoint_id" "wan_direct" 150
punch_ms=$(($(lab_now_ms) - punch_started_ms))

# A path that appears for one reading and collapses back to the relay is not a
# path. Sample it across another half minute to show it held.
held_paths="$(lab_sample_path_of "$viewer" "$endpoint_id" 15 2)"
lab_settle_path_events

selected_path="$(lab_remote_field_of "$viewer" "$endpoint_id" 'path')"
rtt_ms="$(lab_remote_field_of "$viewer" "$endpoint_id" 'rtt_ms')"
state="$(lab_remote_field_of "$viewer" "$endpoint_id" 'state')"
sequence="$(lab_remote_field_of "$viewer" "$endpoint_id" 'latest.sequence')"
relay_url="$(lab_rackio "$viewer" status | jq -r '.data.relay_url')"
relay_mode="$(lab_relay_mode "$viewer")"
events="$(lab_observe_path_events "$viewer")"
relay_after="$(lab_relay_byte_counters)"
packet_loss="$(lab_packet_loss "$viewer" "$monitored_wan_address")"
capture="$(lab_capture_finish "cone_nat_hole_punch" \
  "$monitored_wan_address" "$viewer_wan_address" "$LAB_RELAY_ADDRESS")"

lab_observe_string selected_path "$selected_path"
lab_observe rtt_ms "$([[ "$rtt_ms" == "null" ]] && echo null || echo "$rtt_ms")"
lab_observe_string state "$state"
lab_observe_string endpoint_id "$endpoint_id"
lab_observe reconnect "$(jq -n '{applicable: false,
  note: "nothing restarts or re-pairs after the session is established; the path change measured here happens inside one session"}')"
lab_observe packet_loss "$packet_loss"
lab_observe hole_punch "$(jq -n \
  --arg first "$first_observed_path" --argjson elapsed_ms "$punch_ms" \
  --argjson held "$held_paths" \
  --arg monitored "$monitored_wan_address:$listen_port" \
  --arg viewer "$viewer_wan_address:$listen_port" \
  '{first_path_the_viewer_reported: $first,
    direct_path_established_after_ms: $elapsed_ms,
    measured_from: "the first fleet reading after pairing, to the first reading that reported wan_direct",
    path_held_for_thirty_seconds: $held,
    declared_candidates: {monitored: $monitored, viewer: $viewer},
    note: "neither address is observable on the machine that declared it, and neither router forwards a port. A direct flow between them exists only because both sides sent outbound to the other and their NATs kept the mapping."}')"
lab_observe in_session_promotion "$(jq -n --argjson events "$events" \
  --arg first "$first_observed_path" \
  '{first_reported_path: $first,
    relay_to_direct_events: [$events[] | select(.event == "remote connection path changed"
                                             and .previous_path == "Relayed"
                                             and .current_path == "WanDirect")],
    started_on_the_relay: ($first == "relayed"),
    note: "whether the direct path arrived by promoting a running relayed session, rather than only on a later connect. Recorded from the viewer own log."}')"
lab_observe capture "$capture"
lab_observe relay "$(jq -n --arg url "$relay_url" --arg mode "$relay_mode" \
  --argjson before "$relay_before" --argjson after "$relay_after" \
  --argjson config "$(lab_relay_rendered_config)" \
  '{configured_relay_url: (if $url == "null" then null else $url end),
    relay_mode: $mode, relays_running_in_lab: 1,
    role: "address exchange for the punch; the session is asserted to run directly",
    tls: "none; the lab relay serves plain HTTP. See README.md for why, and for what it does and does not change.",
    byte_counters_before: $before, byte_counters_after: $after,
    rendered_config: $config}')"

lab_assert_equal "selected_path" \
  "a punched path across the non-private internet segment is wan_direct" \
  "$selected_path" "wan_direct"
lab_assert_equal "the_direct_path_held" \
  "every path reading taken over the following thirty seconds was still direct" \
  "$(jq -r '[.[] | select(. != "wan_direct")] | length' <<<"$held_paths")" "0"
lab_assert_true "rtt_reported" "a live direct path reports a round-trip time" \
  "$([[ "$rtt_ms" != "null" ]] && echo true || echo false)"
lab_assert_true "machine_healthy" "the monitored machine reports live metrics over the punched path" \
  "$([[ "$state" == "healthy" || "$state" == "warning" || "$state" == "critical" ]] && echo true || echo false)"
lab_assert_true "metrics_advanced_over_the_punched_path" \
  "the direct session carried more than one sample" \
  "$([[ "$sequence" != "null" ]] && [[ "$sequence" -gt 1 ]] && echo true || echo false)"
# The claim that separates this scenario from every other direct one: the flow
# on the wire is between the two NAT external addresses, neither of which
# publishes anything inbound.
lab_assert_true "the_punched_flow_is_on_the_wire" \
  "the capture shows UDP between the two NAT external addresses" \
  "$(jq -r '.expected_direct_flow_present // false' <<<"$capture")"
lab_assert_relayed_never_reported_as_direct \
  "$selected_path" "$relay_url" "$relay_mode" "$capture" "$events" "$LAB_RELAY_URL"

lab_scenario_complete
