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

| File                                      | Contents                                                                                                                |
| ----------------------------------------- | ----------------------------------------------------------------------------------------------------------------------- |
| `test-results/nat-matrix/<scenario>.json` | scenario id, result, selected path, RTT, reconnect durations, packet loss, path-transition events, per-assertion detail |
| `test-results/nat-matrix/<scenario>.pcap` | bounded header-only capture of that scenario                                                                            |
| `test-results/nat-matrix/summary.json`    | pass/fail roll-up and the agent build provenance                                                                        |

## Topology

```
              lan_a  192.168.101.0/24  (flat, no router)
              ├── lan-a-viewer      .10
              └── lan-a-monitored   .11

  lan_b 192.168.102.0/24        net_internet 192.0.2.0/24        lan_c 192.168.103.0/24
  lan-b-monitored .10 ── router-b .2 ─────┬───── router-c .3 ── lan-c-viewer .10
                        (192.0.2.2)       │      (192.0.2.3)
                        DNAT udp/41641    │      MASQUERADE only
```

- Every network is `internal`, so a lab machine physically cannot reach anything
  outside the topology. Direct-only operation is enforced by the topology, not
  only by configuration.
- `net_internet` is `192.0.2.0/24`, RFC 5737 TEST-NET-1. It is deliberately not
  an RFC 1918 range, so the agent classifies a path across it as `wan_direct`
  rather than `lan_direct` — matching what a real WAN path must report.
- Container hostnames use the reserved `.test` domain.
- The routers share one image. NAT behaviour is chosen by `NAT_MODE`
  (`endpoint_independent` today, `symmetric` implemented and unused), so the
  symmetric-NAT scenarios can be added by selecting a mode rather than by
  forking a container.
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

## How the reports stay honest

- **No fabricated pass.** A scenario reports `pass` only if it reaches its final
  step and every recorded assertion held. A scenario that dies early — including
  a shell that exits zero without finishing — is reported as `fail` with the
  state it observed. Nothing is retried until green.
- **Direct claims are cross-checked against the wire.** Reporting `lan_direct`
  or `wan_direct` requires relaying to be off _and_ the capture to show the two
  peers' own packets _and_ no third-party unicast address anywhere in the
  capture. Multicast and link-local addresses (mDNS pairing advertisement,
  neighbour discovery) are counted separately as link housekeeping.
- **Relayed is never reported as direct.** Asserted in every scenario. It holds
  trivially here — there is no relay in the lab — and is asserted anyway so the
  relay scenarios inherit a check that is already wired into every report
  instead of one written later under pressure to make a relayed run look green.
- **Captures are bounded** by duration, packet count and a 128-byte snaplen.
  Headers are enough to prove which sockets carried a session, and a header-only
  capture keeps metric payloads out of evidence.
- **Packet loss is labelled by source.** The agent exposes no packet-loss metric
  — `rtt_ms` is the only connection-quality number it reports — so loss is
  measured at the link with ICMP and reported as `source: "icmp_probe"` with
  `agent_reported_percent` explicitly `null`. A failed probe reports `null`, not
  zero.
- **No `.env`.** All configuration is in `compose.yaml` or set explicitly by the
  runner.

## Findings from the first full run

Two product gaps were found by running this, and neither was worked around.

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
- **No middleboxes.** No stateful firewall policy, DPI, UDP blocking, MTU black
  holes or PMTUD failures. The checklist's "UDP blocked" row is not covered.
- **One host, one kernel, one architecture.** Every container shares this
  machine's kernel and clock, on `linux/arm64`. Cross-OS and cross-architecture
  behaviour is not exercised.
- **Not yet covered at all:** cone-NAT hole punching, symmetric-NAT relay
  fallback, relay absent/stopped/restarted, and direct↔relay migration. The
  `symmetric` NAT mode and the relay-versus-direct assertion exist as seams for
  that work; no scenario selects them yet, and nothing here should be read as
  evidence for those rows.

A green run of this lab is necessary evidence for the direct-path rows of the
NAT matrix. It is not sufficient evidence that the product works on real
networks, and the checklist rows it does not cover must stay unticked.
