#!/usr/bin/env bash
# direct_only_isolation — a direct-only machine on a LAN that is not empty.
#
# The checklist row this exists for, under "Privacy and security":
#
#   direct-only packet capture has no peer-external DNS, HTTP or QUIC
#
# The other scenarios cross-check a *direct claim* against a capture. This one
# asserts the isolation property on its own: of everything the monitored machine
# put on the wire — daemon start, pairing, steady-state monitoring — every
# packet it sent went to its configured peer or was link housekeeping, and none
# of it went to the DNS resolver or the HTTP server sitting on the same LAN.
#
# Those two services are the point. A machine that contacts nothing on an empty
# LAN has proven nothing; here it is pointed at a real resolver in
# /etc/resolv.conf and shares a segment with a real web server, both reachable
# from the machine itself, which the scenario measures on both sides of the
# capture window.
set -euo pipefail

source "$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)/lib/lab.sh"

viewer="lan-g-viewer"
monitored="lan-g-monitored"
viewer_address="192.168.108.10"
monitored_address="192.168.108.11"
resolver_address="192.168.108.20"
http_address="192.168.108.21"
subnet="192.168.108.0/24"

lab_scenario_begin "direct_only_isolation" "lan_direct"
trap 'lab_scenario_finish' EXIT

lab_observe topology "$(jq -n \
  --arg viewer "$viewer_address" --arg monitored "$monitored_address" \
  --arg resolver "$resolver_address" --arg http "$http_address" \
  '{description: "one flat LAN carrying the two machines under test and two off-path services",
    network: "lan_g 192.168.108.0/24 (docker internal)",
    viewer: {service: "lan-g-viewer", address: $viewer},
    monitored: {service: "lan-g-monitored", address: $monitored},
    off_path_services: {
      dns_resolver: {service: "lan-g-resolver", address: $resolver,
                     configured_in: "/etc/resolv.conf on both machines, via LAB_RESOLVER"},
      http_server: {service: "lan-g-http", address: $http}},
    routers: [],
    why: "the isolation claim is only a result if there was something to contact"}')"

# Before the window: are the off-path services actually reachable from the
# machine that is about to ignore them?
reachability_before="$(lab_probe_offpath_services "$monitored" \
  "http.lab.test" "http://$http_address/")"
lab_observe off_path_reachability_before_capture "$reachability_before"
lab_assert_true "resolver_answers_before_the_window" \
  "the DNS resolver the machine is configured with really answers it" \
  "$(jq -r '.dns.answered' <<<"$reachability_before")"
lab_assert_equal "resolver_answer_is_the_lab_http_server" \
  "the resolver is authoritative for the labs reserved .test names" \
  "$(jq -r '.dns.resolved_to' <<<"$reachability_before")" "$http_address"
lab_assert_true "http_server_answers_before_the_window" \
  "the HTTP server on the machines own LAN really answers it" \
  "$(jq -r '.http.answered' <<<"$reachability_before")"

# The capture covers the whole scenario: it is started before the daemons are,
# so daemon start, the pairing exchange and steady-state monitoring are all
# inside one window. A capture that began after pairing could not say anything
# about what the daemon did while pairing.
#
# The filter is widened to everything. A scenario asking whether this machine
# sent an HTTP request cannot answer it from a UDP-only filter.
lab_capture_start "$monitored" "$subnet" ""

lab_daemon_start "$viewer"
lab_daemon_start "$monitored"
lab_wait_for_command "viewer daemon" lab_rackio "$viewer" status
lab_wait_for_command "monitored daemon" lab_rackio "$monitored" status

relay_url="$(lab_rackio "$monitored" status | jq -r '.data.relay_url')"
lab_assert_equal "monitored_machine_is_direct_only" \
  "the isolation claim is about a machine with no relay configured" \
  "$relay_url" "null"

bundle="$(lab_rackio "$monitored" pairing create | jq -r '.data')"
imported="$(lab_rackio "$viewer" pairing import "$bundle")"
lab_assert_true "pairing_accepted" "the viewer imported the pairing bundle" \
  "$(jq -r '.ok' <<<"$imported")"

fleet="$(lab_wait_for_remote_sample "$viewer")"
endpoint_id="$(jq -r '.data.remotes[0].endpoint_id' <<<"$fleet")"

# Steady state, watched rather than assumed: the window has to contain real
# monitoring traffic, not just a handshake, or "it sent nothing elsewhere" would
# be a statement about a session that had barely started.
steady_state_seconds=20
first_sequence="$(lab_remote_field "$viewer" 'latest.sequence // -1')"
sleep "$steady_state_seconds"
lab_wait_for_sequence_beyond "$viewer" "$first_sequence" 30
lab_settle_path_events

selected_path="$(jq -r '.data.remotes[0].path' <<<"$fleet")"
state="$(lab_remote_field "$viewer" 'state')"
relay_mode="$(lab_relay_mode "$monitored")"
events="$(lab_observe_path_events "$viewer")"

self_addresses="$(lab_interface_addresses "$monitored" "$lab_capture_interface_name")"
capture="$(lab_capture_finish "direct_only_isolation" \
  "$viewer_address" "$monitored_address")"
egress="$(lab_capture_egress "$self_addresses" \
  "$(jq -n --arg viewer "$viewer_address" '[$viewer]')")"

# After the window: the services were still up, so their absence from the
# capture is not the absence of a service that had died.
reachability_after="$(lab_probe_offpath_services "$monitored" \
  "http.lab.test" "http://$http_address/")"

lab_observe_string selected_path "$selected_path"
lab_observe_string state "$state"
lab_observe_string endpoint_id "$endpoint_id"
lab_observe steady_state_seconds "$steady_state_seconds"
lab_observe capture "$capture"
lab_observe egress_isolation "$egress"
lab_observe off_path_reachability_after_capture "$reachability_after"
lab_observe relay "$(jq -n --arg url "$relay_url" --arg mode "$relay_mode" \
  '{configured_relay_url: (if $url == "null" then null else $url end),
    relay_mode: $mode, relays_running_in_lab: 0}')"
lab_observe scope "$(jq -n \
  '{proves: "no packet this machine sent during daemon start, pairing and steady-state monitoring went to any unicast address other than its configured peer, on a LAN where a DNS resolver and an HTTP server were reachable throughout",
    does_not_prove: [
      "anything about a machine with a relay configured; relay mode is a different claim and lives in the relay scenarios",
      "anything about protocols the capture filter could not see: it is unfiltered but the snaplen is 128 bytes, so this is an address-level result, not a payload-level one",
      "anything about a machine whose operator pointed it at a hostname; nothing here resolves a name, so a DNS lookup at configuration time is untested",
      "the negative for all time: it is one bounded window on one topology, not a proof that no code path anywhere can reach out",
      "anything about IPv6 routed traffic; the topology is IPv4-only and the only IPv6 on the wire is link-local housekeeping"
    ]}')"

lab_assert_equal "selected_path" "a peer on the same LAN is a LAN-direct path" \
  "$selected_path" "lan_direct"
lab_assert_true "machine_healthy" "the monitored machine reports live metrics" \
  "$([[ "$state" == "healthy" || "$state" == "warning" || "$state" == "critical" ]] && echo true || echo false)"

# An HTTP request is TCP. A capture that filtered TCP out could not have
# recorded one, and the assertion below that none was sent would be true of the
# filter rather than of the daemon. Asserted because it silently regressed once:
# an empty filter argument fell through to the UDP-only default.
lab_assert_equal "the_capture_recorded_every_protocol" \
  "an isolation claim needs a capture that could have seen the traffic it says was absent" \
  "$(jq -r '.bounds.filter' <<<"$capture")" "(none: every packet on the interface)"

# Fail closed on an empty capture: an isolation claim read off a capture that
# recorded nothing is the most flattering possible result and the least true.
lab_assert_true "the_machine_actually_sent_packets" \
  "an isolation result read from an empty capture would prove nothing" \
  "$(jq -r '.packets_sent_by_this_machine > 0' <<<"$egress")"
lab_assert_true "the_peer_received_the_machines_packets" \
  "the session really ran over this interface, so the capture saw the traffic under test" \
  "$(jq -r '(.peer_destinations | length) > 0' <<<"$egress")"

lab_assert_equal "every_packet_sent_went_to_the_peer" \
  "no packet this machine sent went to any unicast address other than its configured peer" \
  "$(jq -r '.unexpected_destinations | length' <<<"$egress")" "0"
lab_assert_equal "nothing_was_sent_to_the_dns_resolver" \
  "the resolver is in /etc/resolv.conf and answers, and the daemon still never queried it" \
  "$(jq -r --arg a "$resolver_address" \
    '[.destinations_sent_to[] | select(.address == $a)] | length' <<<"$egress")" "0"
lab_assert_equal "nothing_was_sent_to_the_http_server" \
  "the HTTP server is on the same LAN and answers, and the daemon still never contacted it" \
  "$(jq -r --arg a "$http_address" \
    '[.destinations_sent_to[] | select(.address == $a)] | length' <<<"$egress")" "0"

# An ARP request for an address is an attempt to reach it, and it carries no IP
# header, so it would not appear among the destinations above. Checked
# separately rather than left as a hole under the IP-level result.
lab_assert_equal "no_address_was_resolved_at_the_link_layer_for_an_off_path_service" \
  "an ARP for the resolver or the HTTP server would be an attempt to reach it even with no IP packet behind it" \
  "$(jq -r --arg dns "$resolver_address" --arg http "$http_address" \
    '[.link_layer.arp_requests_sent_by_this_machine[]
      | select(.address == $dns or .address == $http)] | length' <<<"$egress")" "0"

lab_assert_true "resolver_still_answered_after_the_window" \
  "the services absence from the capture is not the absence of a service that had died" \
  "$(jq -r '.dns.answered' <<<"$reachability_after")"
lab_assert_true "http_server_still_answered_after_the_window" \
  "the same, for the HTTP server" \
  "$(jq -r '.http.answered' <<<"$reachability_after")"

lab_assert_relayed_never_reported_as_direct \
  "$selected_path" "$relay_url" "$relay_mode" "$capture" "$events"

lab_scenario_complete
