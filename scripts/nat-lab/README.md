# Rackio NAT laboratory

A container topology and runner that produce machine-readable evidence for the
"NAT matrix" section of [`docs/release-checklist.md`](../../docs/release-checklist.md)
(issue [#18](https://github.com/genm/rackio/issues/18), `docs/backlog.md` REL-06).

`compose.yaml` is the evidence artefact the checklist asks for. Every address,
route and NAT rule a scenario depends on is declared there, so any report under
`test-results/nat-matrix/` can be reproduced from the checked-in topology alone.

## Running it

```sh
mise run test:nat-lab              # every scenario
scripts/nat-lab/run.sh             # the same thing
scripts/nat-lab/run.sh address_change   # one scenario
```

Requires Docker with the compose plugin, plus `jq` and `node` on the host. The
runner builds the images, runs each scenario in a freshly created topology,
writes the reports, tears the topology down, and exits non-zero if any scenario
failed.

It is **not** part of `mise run check`. It builds a container image and runs
real daemons for minutes; it is opt-in release evidence, not a per-change gate.

Output, all gitignored build artefacts under the existing `/test-results/` rule:

| File                                            | Contents                                                                                                                                   |
| ----------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------ |
| `test-results/nat-matrix/<scenario>.json`       | scenario id, result, selected path, RTT, reconnect durations, packet loss, relay byte counts, path-transition events, per-assertion detail |
| `test-results/nat-matrix/<scenario>.pcap`       | bounded header-only capture of the path under test                                                                                         |
| `test-results/nat-matrix/<scenario>-relay.pcap` | bounded full-payload capture taken inside the relay's own network namespace, for the opacity scan                                          |
| `test-results/nat-matrix/relay/config.toml`     | the relay configuration the runner rendered; every relay report embeds a copy                                                              |
| `test-results/nat-matrix/summary.json`          | pass/fail roll-up and the agent build provenance                                                                                           |

## Topology

```
              lan_a  192.168.101.0/24  (flat, no router)
              ├── lan-a-viewer      .10
              └── lan-a-monitored   .11

  lan_b 192.168.102.0/24        net_internet 192.0.2.0/24        lan_c 192.168.103.0/24
  lan-b-monitored .10 ── router-b .2 ─────┬───── router-c .3 ── lan-c-viewer .10
                        (192.0.2.2)       │      (192.0.2.3)
                        DNAT udp/41641    │      MASQUERADE only
                                          │
                                    relay 192.0.2.4
                                    (relay-package, plain HTTP)
                                          │
  lan_d 192.168.104.0/24                  │                lan_e 192.168.105.0/24
  lan-d-monitored .10 ── router-d .2 ──────┼───── router-e .2 ── lan-e-viewer .10
                        (192.0.2.5)       │      (192.0.2.6)
                        cone, no forward  │      cone, no forward
                                          │
  lan_f 192.168.106.0/24                  │
  lan-f-monitored .10 ── router-f .2 ──────┘
                        (192.0.2.7)
                        symmetric, no forward

              lan_g  192.168.108.0/24  (flat, no router)
              ├── lan-g-viewer      .10
              ├── lan-g-monitored   .11   resolv.conf -> .20
              ├── lan-g-resolver    .20   dnsmasq, authoritative for lab.test
              └── lan-g-http        .21   busybox httpd
```

- Every network is `internal`, so a lab machine physically cannot reach anything
  outside the topology. Direct-only operation is enforced by the topology, not
  only by configuration. The relay at `192.0.2.4` is the one thing on the wire
  that is neither of the two machines under test, and only the relay scenarios
  point a machine at it.
- `net_internet` is `192.0.2.0/24`, RFC 5737 TEST-NET-1. It is deliberately not
  an RFC 1918 range, so the agent classifies a path across it as `wan_direct`
  rather than `lan_direct` — matching what a real WAN path must report.
- Container hostnames use the reserved `.test` domain.
- The routers share one image. NAT behaviour is chosen by `NAT_MODE`:
  `endpoint_independent` keeps one external port per internal socket, while
  `symmetric` adds `--random-fully` so the external port changes per
  destination. `router-f` is `router-d` with that one setting flipped, which is
  what lets `symmetric_nat_relay_fallback` differ from a direct scenario in the
  NAT's mapping behaviour and in nothing else.
- Every router drops unsolicited inbound UDP addressed to itself on its WAN
  interface, which is what a NAT device does. Without that rule the router's own
  host stack accepts a hole-punch probe aimed at its WAN address, conntrack
  confirms an entry whose reply tuple is the one its internal machine needs, and
  `nf_nat` then hands that machine a different external port — so a punch both
  sides attempted correctly fails on the container's host stack rather than on
  any NAT property. It was measured that way while `cone_nat_hole_punch` was
  built: the monitored machine left `router-d` as `192.0.2.5:19522` while
  listening on `41641`, and the viewer's router discarded every reply. ICMP is
  left alone so the packet-loss probes still reach the routers, and a DNAT'd port
  forward is unaffected because it is translated in `PREROUTING` and traverses
  `FORWARD`, never `INPUT`.
- `lan_g` is the one segment that carries something other than the machines
  under test and no relay: a DNS resolver and an HTTP server, both built from
  `services.Dockerfile` and both reachable. They exist so `direct_only_isolation`
  can produce a result rather than an artefact. The numbering skips
  `192.168.107.0/24` deliberately — Docker's default address pool commonly hands
  that block to unrelated compose projects on a developer machine, and it did
  exactly that while `lan_g` was being added, so the lab could not come up. Any
  block here can collide the same way; the symptom is
  `Pool overlaps with other one on this address space` and the fix is to
  renumber the network in `compose.yaml`.
- The relay is `relay-package/` built unchanged — upstream `iroh-relay` pinned
  to `1.0.3`, the same image an operator would run. Only its configuration
  differs; see "The lab's relay is not a production relay" below.
- The runner recreates the topology per scenario. Scenarios must be reproducible
  one at a time, which they are not if they inherit another scenario's pairing
  state or NAT conntrack.

The agent image builds `rackio-agent` for `linux/arm64` from this repository's
source in a multi-stage build; the dependency graph is compiled in its own layer
from manifests alone, so editing agent code does not recompile iroh and friends.
No binary is vendored, and `summary.json` records the commit the image was built
from and whether the tree was dirty.

## Scenarios and what each proves

### `same_lan_direct`

Two machines on one LAN, no router between them. Proves that pairing over a LAN
selects a direct path reported as `lan_direct`, with a measured RTT, and that
the capture on the LAN interface shows only the two machines' own packets.

### `port_forwarded_direct`

The monitored machine sits behind `router-b` with a fixed listen port
(`rackio listen-port set 41641`) published by a DNAT port forward; the viewer
sits behind a second NAT. Proves that a fixed listen port gives a NATed machine
a stable forwardable address, and that a session crossing two NATs over the
non-private segment is carried directly and reported as `wan_direct`. The
capture is taken on the router's WAN interface.

Verified while building the lab: from `lan_c`, `192.0.2.2` answers and
`192.168.102.10` does not. The NAT really is in the way.

The machine cannot observe the address its router forwards, so the operator
supplies it with `rackio advertise-address add 192.0.2.2:41641` and
`pairing create` carries it in the bundle alongside the interface addresses.
The scenario asserts that the bundle contains it and imports the bundle exactly
as produced — `bundle_addresses.substitution_performed` is `false`, and the lab
has no bundle-rewriting code left.

### `address_change`

The monitored machine rebinds to a different address _and_ port, then returns.
The assertions are the ones
[`scripts/test-two-daemon-address-change.sh`](../test-two-daemon-address-change.sh)
already makes about the documented recovery behaviour; they are not re-invented
here. What the container lab adds is a real address change — the host script can
only move the port, while here the machine also loses its IP and takes another,
which is the case the checklist's "stale address" row is about.

Phases: restart on the paired address recovers on its own; a move the viewer
cannot follow goes visibly offline while keeping its last known values and
naming the setting that fixes it; returning recovers without re-pairing and
survives a viewer restart. Reconnect duration is measured for each.

### `symmetric_nat_relay_fallback`

`lan-f-monitored` sits behind `router-f`, whose `--random-fully` masquerade
gives it a different external port for every destination. It is configured
exactly as a machine that _could_ be reached directly would be — a fixed listen
port and `rackio advertise-address add 192.0.2.7:41641` — and pointed at the
self-hosted relay with `rackio relay set`. The only thing standing between it
and a direct path is the NAT's mapping behaviour.

Proves that the viewer reports the path as `relayed` and never as direct. The
path is sampled every two seconds for thirty seconds, across several of iroh's
five-second holepunch retries, so "never direct" is a measurement rather than a
single lucky reading; the viewer's own event log is checked for any direct
transport as well. The relay's `relayserver_bytes_recv_total` counter is read
before and after, so the claim that the relay carried the session comes from the
relay.

The capture on `router-f`'s WAN interface does show UDP between the two NAT
external addresses: both sides tried to open a direct path. It never became one.
The report records the attempt under `direct_path_attempts` rather than
presenting a clean wire.

This scenario also carries the relay payload-opacity evidence for
[#19](https://github.com/genm/rackio/issues/19); see below.

### `relay_outage`

One viewer watches two machines at once: `lan-b-monitored` through its port
forward, and `lan-f-monitored` through the relay. Both are pointed at the same
relay, so the outage is the only variable between them.

Proves that stopping the relay container leaves the direct machine `healthy`,
still on `wan_direct`, and still producing fresh metric sequences — checked at
the start _and_ at the end of the outage, so "unaffected" is not read from one
early sample. The relay-only machine becomes `offline` while keeping its last
CPU and memory values and its frozen sequence number: it is never reported
healthy and never reads as zero. Starting the relay again recovers it without
re-pairing, and it comes back `relayed` rather than silently direct. Recovery
duration is measured.

### `udp_blocked`

UDP is dropped in both directions at `router-b` with a single
`iptables -I FORWARD 1 -p udp -j DROP`, which leaves TCP untouched. The
checklist row does not say what the answer must be, so the scenario measures
both cases instead of choosing the flattering one:

- with the relay reachable over TCP, the session migrates onto it and keeps
  reporting — the path becomes `relayed` and fresh samples keep arriving;
- with the relay stopped as well, nothing is reachable, and the viewer reports
  `offline` with its last known values intact, the sequence frozen at the last
  sample received over the relay, and the machine still registered.

Then the block is lifted and the relay started, and the machine recovers.

### `path_migration`

The same session migrates twice, without re-pairing: direct, then relayed when
UDP is dropped at `router-b`, then direct again when the drop rule is removed.

Proves that the ordered sequence of paths the viewer logged is exactly
`WanDirect`, `Relayed`, `WanDirect`, that a `remote connection path changed`
event records leaving the direct path and another records arriving on the relay,
and that the return is a single event naming `Relayed` as the previous path and
`WanDirect` as the current one. Both migrations are timed.

Two things this scenario measures rather than asserts, both recorded in the
report. `in_session_upgrade` answers whether the relayed session climbs back
onto the direct path without reconnecting — since
[#158](https://github.com/genm/rackio/issues/158) it does, in under five seconds
(4,454 ms in the run that produced the current reports), where the same
measurement previously found nothing in 240 seconds. The second
is that the outbound migration passes through a transient `Unknown` path, which
is still the case and is listed as a finding below.

### `cone_nat_hole_punch`

`lan-d-monitored` and `lan-e-viewer` sit behind cone NATs with **no port forward
on either router**. Both pin their listen port and declare the address their own
NAT maps them to with `rackio advertise-address add`, and both are pointed at
the relay, which carries the address exchange the punch needs.

Proves that an operator-declared address is a real traversal candidate rather
than a pairing-bundle entry: it appears in `rackio status`.`direct_addresses` on
both machines — asserted, and the machine cannot observe it on any interface —
and the two machines, neither of which publishes anything inbound, end up on a
direct path reported as `wan_direct`. The path is then sampled every two seconds
for thirty seconds to show it held rather than flickering, and the capture on
`router-d`'s WAN interface must show UDP between the two NAT external addresses.

What makes it a hole punch rather than `port_forwarded_direct` under another
name is asserted, not assumed: both routers' `nat` tables are read back during
the run and must contain no DNAT rule. The relay is configured and does carry
the address exchange, so the scenario also inherits the shared check that a
relayed transport is never reported as direct — the selected path must be
direct, backed by the capture, with no unicast peer beyond the two machines and
the declared relay.

The report records `first_path_the_viewer_reported` and any
`Relayed -> WanDirect` transition under `in_session_promotion`, so whether the
direct path arrived by promoting the running session or on a later connect is
visible rather than assumed.

### `direct_only_isolation`

Two machines on a flat LAN that also carries a DNS resolver and an HTTP server,
both reachable, with the machines' `/etc/resolv.conf` pointing at that resolver.
Covers the "Privacy and security" row **direct-only packet capture has no
peer-external DNS, HTTP or QUIC** — the other scenarios cross-check a _direct
claim_ against a capture, and this one asserts the isolation property on its
own.

The two off-path services are the whole point. A machine that contacts nothing
on an empty LAN has proven nothing about itself. Here the resolver is the one in
`resolv.conf` and the web server is one hop away on the same segment, and the
scenario proves from the monitored machine that both answer it — `getent hosts`
for DNS, `curl` for HTTP — **before** the capture starts and again **after** it
stops, so the services were up on both sides of the window. Those probes run
outside the window on purpose: inside it they would be exactly the traffic the
scenario claims is absent.

The capture runs in the monitored machine's own namespace for the whole
scenario — started before either daemon, so daemon start, pairing and
steady-state monitoring are all in one window — and it is **unfiltered**. The
path scenarios capture `udp or icmp or icmp6`; an HTTP request is TCP, so a
scenario asserting none was sent from a UDP-only capture would be describing its
filter rather than the daemon. That the filter really was empty is itself
asserted, because it regressed once during development.

The result is directional, under `egress_isolation`: of every packet whose
source was one of the machine's own addresses, where did it go? A bidirectional
flow summary cannot tell a packet the machine sent from one somebody sent at it,
and only the first is the daemon's doing. Every destination is listed with its
packet count and classified as the configured peer, link housekeeping
(multicast, broadcast, IPv6 link-local, the unspecified address) or unexpected;
the assertion is that the unexpected list is empty, plus named assertions that
nothing at all went to the resolver's or the HTTP server's address. Nothing is
filtered out of the evidence — housekeeping is separated from the peer set and
counted, not hidden.

Packets without an IP header are counted too. ARP is link-layer broadcast with
no IP destination, so it cannot appear among the destinations, and an ARP for an
address is itself an attempt to reach it. `link_layer.non_ip_packets` closes the
arithmetic against the capture total and
`link_layer.arp_requests_sent_by_this_machine` lists the targets, with its own
assertion that neither off-path service is among them.

In the run that produced the current report the machine sent 90 IP packets, all
to its peer, and one ARP request, for its peer. There was **no** mDNS: the
pairing bundle carries the peer's addresses, so nothing had to be discovered on
this LAN. The scenario records an empty housekeeping list rather than implying
traffic that did not occur.

What it does not prove, recorded in the report under `scope.does_not_prove`:

- nothing about a machine with a relay configured — relay mode is a different
  claim and lives in the relay scenarios;
- nothing at the payload level. The capture is unfiltered but keeps the lab's
  128-byte snaplen, so this is an address-level result;
- nothing about an operator who configures a _hostname_. No name is resolved
  anywhere in this scenario, so a DNS lookup at configuration time is untested;
- nothing about IPv6 routed traffic — the topology is IPv4-only;
- not the negative for all time. It is one bounded window on one topology, not a
  proof that no code path can ever reach out.

## The lab's relay is not a production relay

`relay-package/README.md` tells an operator to mount a valid TLS certificate and
to configure agents with `rackio relay set https://relay.example.test`. The lab
cannot do that, and the reason is worth stating precisely because it is the one
place where the lab's relay differs from the one the product documents.

Unless the operator pins a CA, the agent builds its iroh endpoint without
setting a CA configuration, so iroh's default applies:
`CaTlsConfig::EmbeddedWebPki`, the compiled-in Mozilla root set. That is the
trust anchor for the relay's HTTPS connection _and_ for the QUIC
address-discovery connection. No certificate the lab can mint is signed by a
Mozilla root, and the lab's networks are `internal`, so no publicly trusted
certificate can be obtained for `192.0.2.4` either.

Since [#157](https://github.com/genm/rackio/issues/157) an operator _can_ pin a
CA, with `rackio relay set <URL> --ca-certificate <PATH>`, so an HTTPS lab relay
is now possible in principle. The lab has not been rebuilt around it; what that
would take is recorded under finding 1 below.

The lab therefore runs the relay over plain HTTP, at `http://192.0.2.4/`. This
is not a lab hack around a product rule: iroh 1.0.3's relay client disables TLS
exactly when the relay URL scheme is `http`
(`iroh-relay-1.0.3/src/client/tls.rs`, `use_tls`), and `iroh-relay`'s server
serves every relay service over plain HTTP when the config has no `[tls]`
section. `rackio relay set` accepts the URL because `validate_relay_url` accepts
anything iroh will parse as a `RelayUrl`; no agent-side validation was weakened
or bypassed to make this work. The relay still runs with the fail-closed
`access.allowlist` the production package documents, holding exactly the
endpoint IDs of the machines in the scenario, rendered by the runner once those
IDs exist and embedded verbatim in each relay report.

What the deviation changes:

- **The relay leg is not encrypted in transport.** An observer on the wire
  between a machine and the lab relay sees the relay protocol in the clear.
  This makes the lab strictly _weaker_ than production, which is why it
  strengthens rather than weakens the payload-opacity evidence below: the scan
  finds no readable application payload even with the relay's own TLS removed.
- **There is no QUIC address discovery.** `iroh-relay` requires TLS for its QUIC
  endpoint, so the lab's config sets `enable_quic_addr_discovery = false`. A
  machine in the lab therefore never learns its own NAT-mapped address by
  itself. `cone_nat_hole_punch` works anyway, because the operator declares that
  address with `rackio advertise-address`; what the lab cannot exercise is the
  case where nobody configured one and iroh has to discover it.

What it does **not** change:

- The relay protocol, the relay build, and the relaying behaviour are upstream
  `iroh-relay` 1.0.3 either way.
- End-to-end confidentiality. Rackio's session is a QUIC connection between the
  two endpoints, carried inside relay frames; the relay's own transport security
  is a separate layer and removing it does not expose the session.
- Path classification. A relayed transport is reported as `relayed` whether the
  relay is reached over HTTP or HTTPS.

## Relay payload opacity (issue #19)

`symmetric_nat_relay_fallback` captures inside the relay container's own network
namespace, with full payloads rather than headers — the claim under test is
about payload bytes, so a header-only capture would prove nothing — bounded by
packet count and duration. A sidecar built from the lab's router image joins the
relay's namespace, so the relay image itself stays the production package
unmodified, and what the sidecar sees is what the relay's socket sees.

[`lib/scan-relay-capture.mjs`](lib/scan-relay-capture.mjs) then reports three
things separately, and the report keeps them under `relay_payload_opacity`:

1. **What the relay can read.** Both endpoint identities appear in the bytes as
   raw 32-byte keys — the relay routes by them, so it must know them — along
   with the per-direction packet counts, the payload volume, and the first and
   last packet times. This is real metadata and is recorded as observed. A relay
   operator can see who talks to whom, when, and how much.
2. **Whether any value the viewer read is in the bytes.** The needles are taken
   from the session that just ran: the machine's display name, its node id, and
   the `memory_total_bytes` and disk `total_bytes` the viewer displayed, the
   last two searched for as protobuf varints. None are found.
3. **Whether a protobuf frame can be decoded.** `rackio-protocol` frames a
   message as a big-endian `u32` length followed by the encoded protobuf, so the
   scan walks every offset in the reassembled payload stream, and at each one
   tries that framing and a full protobuf wire-format parse of the payload. A
   parse counts only if it consumes exactly the framed length and yields at
   least two fields. Zero frames decode.

The assertions fail closed: if the capture cannot be retrieved, or contains no
payload bytes, the scenario fails rather than reporting opacity it did not
check.

## How the reports stay honest

- **No fabricated pass.** A scenario reports `pass` only if it reaches its final
  step and every recorded assertion held. A scenario that dies early — including
  a shell that exits zero without finishing — is reported as `fail` with the
  state it observed. Nothing is retried until green.
- **Direct claims are cross-checked against the wire.** Reporting `lan_direct`
  or `wan_direct` requires the capture to show the two peers' own packets, no
  unicast address beyond the ones the scenario declared, and the most recent
  path event not to be a relayed one. Multicast and link-local addresses (mDNS
  pairing advertisement, neighbour discovery) are counted separately as link
  housekeeping. On a machine with no relay configured, the stronger form is
  asserted: it must never have had a relayed transport at all.
- **Relayed is never reported as direct.** Asserted in every scenario. It was
  written while the lab had no relay, as a seam the relay scenarios would land
  on; they have landed on it. The relay scenarios drive the branch that
  requires a non-direct transport to surface as `relayed` and to be backed by a
  configured relay, and the direct scenarios drive the other one. Each
  scenario's relay configuration is also checked against what it set, so a
  scenario cannot claim direct-only operation while a relay is configured.
- **Captures are bounded** by duration, packet count and a 128-byte snaplen.
  Headers are enough to prove which sockets carried a session, and a header-only
  capture keeps metric payloads out of evidence. The relay-side capture in
  `symmetric_nat_relay_fallback` is the one exception: it keeps full payloads,
  because the claim it tests is about payload bytes, and it is bounded by packet
  count instead.
- **The relay's own numbers come from the relay.** The "relay byte count" the
  checklist asks for is read from `iroh-relay`'s `/metrics`
  (`relayserver_bytes_sent_total`, `relayserver_bytes_recv_total`) before and
  after a session, not inferred from a capture. A counter read while the relay
  is stopped is reported as unavailable, not as zero.
- **Packet loss is labelled by source.** The agent exposes no packet-loss metric
  — `rtt_ms` is the only connection-quality number it reports — so loss is
  measured at the link with ICMP and reported as `source: "icmp_probe"` with
  `agent_reported_percent` explicitly `null`. A failed probe reports `null`, not
  zero.
- **No `.env`.** All configuration is in `compose.yaml` or set explicitly by the
  runner.

## Findings from the relay and NAT-traversal run

Four findings came out of building the relay scenarios. None is worked around,
and none is patched by weakening anything on the agent side. The second has
since been fixed in the product and re-measured here; the entry keeps the
original observation so the evidence trail survives.

1. **The agent cannot be told to trust a self-hosted relay's own certificate
   authority.** `bind_endpoint` in
   [`crates/rackio-iroh/src/transport.rs`](../../crates/rackio-iroh/src/transport.rs)
   never calls `Endpoint::builder().ca_tls_config(..)`, so iroh's default —
   the compiled-in Mozilla root set — is the only trust anchor, for both the
   relay's HTTPS connection and its QUIC address-discovery connection.
   `relay-package/README.md` tells an operator to "mount a valid TLS
   certificate", and an operator running a relay on a private network commonly
   has an internal CA. Such a relay is unusable: the agent will not connect to
   it at all, and `rackio relay set` accepts the URL without warning because
   `validate_relay_url` only checks that iroh can parse it. There is no `rackio`
   setting that adds a root.

   Consequence for the lab: no relay it can build would be trusted over HTTPS,
   so the relay runs over plain HTTP and QUIC address discovery is off. See
   "The lab's relay is not a production relay".

   **Fixed** for [#157](https://github.com/genm/rackio/issues/157):
   `rackio relay set <URL> --ca-certificate <PATH>` stores the anchor beside the
   relay URL and `bind_endpoint` passes it to
   `Endpoint::builder().ca_tls_config(CaTlsConfig::custom_roots(..))`, which
   replaces the WebPKI root set for relay connections. A file that is missing,
   unreadable or not a usable authority is refused when it is configured, and a
   pinned relay whose CA cannot be loaded refuses to start rather than falling
   back to the public roots.

   Not yet re-measured in this lab. The lab's relay still runs over plain HTTP,
   because switching it to HTTPS means minting a lab CA, issuing a certificate
   for `192.0.2.4`, mounting it in the relay container, adding a `[tls]` section
   to the relay config, distributing the CA into each machine image and
   configuring `--ca-certificate` in the runner — and the plain-HTTP relay is
   what makes the payload-opacity evidence in "Relay payload opacity" strictly
   stronger than production. Doing so would additionally allow
   `enable_quic_addr_discovery = true`, which is the one relay capability the
   lab currently cannot exercise at all. Neither is in scope here.

2. **`rackio advertise-address` was not a NAT-traversal candidate.** The
   configured address reached a peer only inside a pairing bundle
   (`bundle_direct_addresses` in `apps/agent/src/runtime/local_ipc.rs`); it was
   never passed to the endpoint as an external address, though iroh has an API
   for exactly this — `Endpoint::builder().external_addr(addr)`, documented as
   "will be used in NAT traversal and to establish direct connections". Three
   consequences were observed: the address was absent from
   `rackio status`.`direct_addresses`, so an operator could not see from the
   machine that the setting had taken effect; a session that fell back to the
   relay never returned to the direct path on its own, measured at 240 seconds
   in `path_migration`; and cone-NAT hole punching could not be exercised at
   all.

   **Fixed** for [#158](https://github.com/genm/rackio/issues/158):
   `EndpointConfig` carries the configured addresses and `bind_endpoint` passes
   each one to `external_addr`, so they join the endpoint's own advertised set.
   Re-measured in this lab, all three consequences are gone:

   - `cone_nat_hole_punch` reports the declared address in
     `rackio status`.`direct_addresses` on both machines and asserts it;
   - the same scenario reaches a direct path between two cone NATs with no port
     forward on either side;
   - `path_migration`'s `in_session_upgrade` now reports
     `running_session_returned_to_direct_without_reconnecting: true` in under
     five seconds — **4,454 ms** in the run that produced the current reports —
     where the same measurement previously found nothing in 240 seconds. It is still recorded rather than asserted, because the
     relay-to-direct migration the scenario _does_ assert is the one that
     follows.

   Still no probing and no discovery: the address remains operator-supplied
   configuration, and a wrong one stays an unreachable candidate.

3. **A direct-to-relay migration is logged as two transitions through
   `Unknown`.** Losing the direct path ends the session, and the replacement is
   classified before iroh has selected a path for it, so the viewer logs
   `WanDirect -> Unknown` and then, about five seconds later, `Unknown ->
Relayed`, and reports the path as unknown in between. The migration is
   recorded with the right paths at each end, but a consumer looking for one
   direct-to-relayed transition will not find it. Recorded in `path_migration`
   under `transient_unknown_path`; the scenario asserts the ordered path
   sequence with the `Unknown` readings removed, and says so.

4. **An unreachable machine always blames its listen port.** During a relay
   outage the viewer reports the relay-only machine as offline with "if this
   machine restarted on a new port, give it a fixed one with
   `rackio listen-port set <PORT>`". That is the right advice for the
   `address_change` case and the wrong advice here: the machine did not move,
   the relay stopped. The text is recorded verbatim in `relay_outage` under
   `outage.relayed_machine.details`. Minor next to the three above, but it will
   send an operator to the wrong setting.

## Findings from the first full run

Two product gaps were found by the first run, and neither was worked around.

1. **The agent emitted no usable path-transition events.** Session start
   assigned the path to the snapshot silently; only the five-second refresh
   compared before assigning. Since a reconnect goes through session start, an
   event could fire only during a mid-session migration — which direct-only
   mode cannot produce. Across three full disconnect/address-change/recovery
   cycles the viewer's log held nothing but lifecycle messages, so
   `address_change` was left failing on that assertion rather than having it
   downgraded.

   **Fixed** in [#151](https://github.com/genm/rackio/pull/151): both call sites
   now share one owner of the rule, and a session start or resumption is logged
   as `remote monitoring session established` with the path it runs over. The
   scenario asserts one resumption per recovery, because the path itself does
   not change here — a count of path changes alone would be zero even when
   recovery worked.

2. **A machine behind NAT had no supported way to advertise its forwarded
   address.** `pairing create` could only advertise the machine's own
   interfaces, and in direct-only mode nothing discovers the router's external
   address, so the first version of `port_forwarded_direct` had to rewrite the
   bundle — a lab affordance standing in for missing product behaviour.

   **Fixed** in [#153](https://github.com/genm/rackio/pull/153) for
   [#152](https://github.com/genm/rackio/issues/152): `rackio advertise-address`
   stores the operator-known address as configuration and pairing bundles carry
   it. The scenario now imports what the product produced, and the rewrite
   helper was deleted rather than kept for the next gap.

## What a container lab does **not** prove

Be careful about what these reports are worth. This lab exercises real Linux
kernel networking, real iptables NAT, and real agent binaries, but it is not
hardware and it is not the internet.

- **Not real carrier-grade NAT.** `iptables MASQUERADE` is one NAT
  implementation. Real CGNAT boxes differ in mapping and filtering behaviour,
  port-allocation limits, mapping lifetimes, hairpinning, and how aggressively
  they drop idle UDP state. `--random-fully` approximates a symmetric mapping;
  it is not a specific vendor's symmetric NAT.
- **No IPv6 on a real ISP.** The topology is IPv4-only. Prefix delegation,
  rotating prefixes, ISP firewall defaults and happy-eyeballs behaviour are
  untested, so the checklist's "IPv6 direct" row is **not** covered here.
- **No physical link loss, jitter or bandwidth limits.** These paths are
  loopback-speed veth pairs; every measured RTT is around a millisecond and
  measured loss is zero. The RTT and loss numbers show the lab was clean, not
  that the product performs well on a real link. No `netem` impairment is
  applied yet.
- **No Wi-Fi, roaming, sleep or interface flap.** The `address_change` scenario
  changes an address administratively; it does not reproduce a laptop suspending,
  changing SSID, or losing an interface.
- **Only one kind of middlebox.** `udp_blocked` drops UDP at a router, which is
  a real impairment and covers the checklist's "UDP blocked" row for the case
  where TCP survives. There is still no stateful firewall policy, no DPI, no
  MTU black hole and no PMTUD failure.
- **One host, one kernel, one architecture.** Every container shares this
  machine's kernel and clock, on `linux/arm64`. Cross-OS and cross-architecture
  behaviour is not exercised.
- **Not a production relay.** The relay scenarios run against upstream
  `iroh-relay` 1.0.3 configured for plain HTTP, with no QUIC address discovery.
  See "The lab's relay is not a production relay" for exactly what that changes.
- **Hole punching without a declared address is not covered.**
  `cone_nat_hole_punch` proves the punch when each operator declares the address
  their NAT maps them to. It does not exercise the case where nobody configured
  one and iroh has to discover it: that needs QUIC address discovery, which the
  lab's relay cannot serve without TLS (finding 1). A machine whose external
  address is neither configured nor discoverable is still untested here.

- **Isolation is proven against two services, not against the internet.**
  `direct_only_isolation` shows the daemon ignored a reachable resolver and a
  reachable web server on its own LAN. It is not a proof that no code path
  anywhere can reach out, and its networks are `internal`, so it cannot
  distinguish "did not try" from "could not have succeeded" for a destination
  outside the topology.

A green run of this lab is necessary evidence for the direct-path, hole-punch,
relay-fallback, relay-outage, UDP-blocked and path-migration rows of the NAT
matrix, and for the relay payload-opacity and direct-only isolation items under
"Privacy and security". It is not sufficient evidence that the product works on
real networks, and the one checklist row it does not cover — IPv6 direct — must
stay unticked.
