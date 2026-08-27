# Current implementation state

This is the handoff-oriented snapshot of the repository. Product invariants
live in [`architecture.md`](architecture.md) and
[`threat-model.md`](threat-model.md); release eligibility lives in
[`release-checklist.md`](release-checklist.md). Implemented code listed here
does not make a release-checklist item complete without its required
cross-platform, network or supply-chain evidence.
Issue-sized remaining work and execution order live in
[`backlog.md`](backlog.md).

## Working vertical slice

The source currently supports this development flow:

1. run an independent Rackio daemon on the monitored machine;
2. open a five-minute pairing window with `rackio pairing create`;
3. import its `rackio-pair:…` bundle in the desktop pairing dialog or with
   `rackio pairing import BUNDLE`;
4. let the viewer daemon pin the monitored endpoint ID, complete the one-time
   pairing and persist a secret-free monitored-machine record;
5. stream remote metrics into the viewer daemon;
6. expose local and remote snapshots to Tauri over local IPC;
7. render state, metrics, connection path, RTT, a selectable live trend chart
   and a cross-machine comparison view in the desktop rack.
8. inspect and explicitly confirm an SSH host key, push a local Linux release
   archive, verify and install it without server internet access, then import
   the resulting one-time pairing bundle automatically.

A second two-daemon smoke covers restart recovery: a monitored daemon on a
configured fixed port is restarted and the viewer resumes on its own, a daemon
moved to another port is reported offline with the recovery step and its last
known values intact, and returning it to the paired port recovers without
re-pairing or editing the registry.

A container laboratory (`scripts/nat-lab/`, `mise run test:nat-lab`) runs the
agent against real Linux kernel networking and iptables NAT, and writes one JSON
report and one bounded capture per scenario. Eight scenarios pass: same-LAN
direct, port-forwarded direct, address change, cone-NAT hole punching, symmetric
NAT falling back to a relay, relay outage and recovery, UDP blocked, and path
migration in both directions. A direct claim is cross-checked against the
capture, and the relay scenarios show no decodable protobuf frame while
recording the endpoint identities the relay genuinely does see. It is not
carrier-grade NAT, not a real ISP's IPv6, and has no link impairment: necessary
evidence for those rows, not sufficient on its own, and no release-checklist box
is ticked from it.

A reusable two-daemon same-host smoke exercises this flow: pairing produces one
`lan_direct` remote machine with a live CPU sample, importing the same bundle
again fails, and restarting the viewer daemon reconnects from its secret-free
record. That is useful integration evidence, but it is not a substitute for the
NAT and mixed-OS release matrices.

## Capability matrix

| Surface | Implemented now | Missing or incomplete |
| --- | --- | --- |
| Collector | CPU, memory, swap, disk, network rate, hottest-sensor temperature, uptime, OS and architecture | GPU, per-sensor temperature history, processes, containers and logs are not implemented |
| Tray | One submenu per machine carrying the same metrics as the dashboard card — CPU, memory, swap, the fullest filesystem by mount name, temperature, network rate and uptime — plus the connection path and RTT | Metrics an unreachable machine cannot report show an em dash and no bar rather than a stale value |
| Local history | 2-second samples, SQLite WAL batches, raw/minute retention and 64 MiB pruning | Cross-machine long-term aggregation is intentionally absent |
| Health thresholds | Built-in disk, memory, CPU and hardware-relative temperature levels with sustained-condition windows, per-rule `rackio alerts` retuning, disable and machine-wide off, applied to the running daemon without a restart, each breach published as a named detail line that reaches the card and the OS notification | Thresholds are changed from the CLI or `config.json`; the desktop has no threshold editor |
| Remote server | Endpoint authentication, allowlist authorization, immediate revoke teardown, node info, live metrics, health, path and bounded history protocol, an operator-configured fixed listen port that survives restarts, and operator-configured advertised addresses given to the endpoint as NAT-traversal candidates | Container-lab evidence covers the direct, hole-punched, relayed and outage paths; real-network, IPv6 and carrier-grade NAT evidence remains incomplete |
| Pairing | Five-minute, attempt-limited, single-use secret, window-scoped mDNS endpoint advertisement, copy/paste import, local QR generation, private file import/export, and bundles that carry operator-configured advertised addresses alongside this machine's own | Cross-LAN pairing still depends on transferred direct addresses or a configured self-hosted relay: a machine behind NAT needs an operator-supplied UDP address configured with `rackio advertise-address`, because nothing discovers the router's external address |
| Viewer daemon | Secret-free remote inventory, reconnect loop, persisted last-known snapshot, bounded remote history query, periodic health/path refresh, direct-address refresh from authenticated sessions, structured path-change and session-established events, and stale/offline derivation | Container-lab evidence covers path migration in both directions; real-network evidence remains incomplete |
| Desktop | Local/remote cards with a per-card live trend chart (CPU, memory, swap, disk, temperature, network or RTT, selected by clicking a metric tile and remembered per card), uptime as a card field rather than a series, a remote history dialog over 1/6/24/168 hours with the same selector minus network and RTT, a fleet view overlaying one metric across every machine, severity-ordered cards, dynamic worst-state tray icon, per-machine OS notifications including recovery and the reporting machine's own breach detail, a per-filesystem capacity list in the machine detail, QR/file/bundle pairing, SSH bootstrap, and explicit degraded states | Cross-platform packaging |
| Trend charts | Time-proportional x axis, lines broken across gaps wider than the series' own spacing, hover crosshair reading the real sample rather than an interpolation, the hardware's own temperature critical drawn as a threshold, and network carried as separate received/sent lines | CPU and memory thresholds are deliberately absent: each peer evaluates its own alert rules, so the viewer has no truthful value to draw |
| Linux packaging | Native x86_64/arm64 release archives with independently verified SLSA provenance, clean Ubuntu systemd lifecycle evidence, HTTPS or client-pushed archive install, checksum verification, viewer-group isolation, preserving/purge uninstaller and gated GitHub pre-release publication | No supported release: the publication workflow refuses a non-pre-release version; SSH client, reboot and supported-distribution coverage are not proven |
| macOS | Dedicated daemon user/group, LaunchDaemon lifecycle, preserving uninstaller and fail-closed signed/notarized pkg builder | A real Developer ID signature, notarization receipt and reboot evidence require release credentials and a clean macOS host |
| Windows | Explicit-DACL named-pipe IPC with caller-token verification, Windows Service installer/uninstaller and a GitHub-hosted CI integration scenario | Authenticode/MSI signing, clean-host remote-client rejection and reboot evidence require a Windows release host |
| Relay | Version-pinned upstream relay container and configuration, and an operator-pinned CA certificate so a relay signed by an internal authority is reachable instead of only a publicly trusted one | Internet-exposure workflow and token delivery; the container lab covers fallback, outage and migration, but not a relay reached across the public internet |

CI evaluates every non-draft PR transition against its complete base-to-head
change set using the planner from the protected base revision. It runs only the
owning Rust OS, frontend and security gates while keeping every required context
reportable. Selector, workflow or failure-guard changes, scheduled security
checks, malformed planner output and selector failures run every gate.
Documentation-only updates retain commit policy, required-context reporting and
affected-range secret scanning without rebuilding Rust or the desktop.
The protected-base planner prevents script-only selector changes from shrinking
those gates; it does not make a pull request's workflow definition immutable,
so workflow changes remain security-sensitive control-plane review.

## Persistence and directionality

The daemon data directory owns:

- `identity.key`: the endpoint private key;
- `node-id`: the random application node UUID;
- `metrics.sqlite3`: only this machine's metric history;
- `peers.json`: inbound viewers allowed to read this machine;
- `monitored-machines.json`: outbound machines this daemon views.

`monitored-machines.json` stores the pinned endpoint ID, direct addresses,
explicit relay URLs, basic node information and pairing time. It never stores
the one-time pairing secret. The direct addresses are refreshed from sessions
already authenticated against the pinned endpoint ID and are bounded, so a peer
that rebinds cannot grow the record without limit.

Inbound authorization and outbound monitoring are separate directions. Pairing
machine A as a viewer of machine B does not authorize B to view A. Mutual
monitoring requires a second explicit pairing.

## Current remote-state semantics

- A metric sample or heartbeat updates viewer-local `last_seen`.
- Ten seconds without either is presented as `stale`.
- Thirty seconds without either is presented as `offline`.
- Authorization rejection is `auth_error`; protocol-major rejection is
  `incompatible`.
- Last known in-memory values remain visible instead of being replaced by zero.
- A viewer-daemon restart retains remote identity, connection information and
  the last-known metric snapshot without permanently replicating remote history.

Remote health and connection path are refreshed during a monitoring session.
Path changes, relay fallback and direct recovery are emitted as structured
events and surfaced without presenting relayed traffic as direct.

## Distribution status

The Linux installer contract is implemented, and publication of the headless
Linux agent is automated: a `v*` tag whose commit is contained in protected
`main` drives [`Release`](../.github/workflows/release.yml), which reuses the
verified artifact build and creates the immutable GitHub pre-release described in
[`release-governance.md`](release-governance.md). Both architectures ship with a
checksum and a build-provenance attestation, and the reviewed `install.sh` is
published beside them:

```sh
sh install.sh --version <VERSION> \
  --releases-url https://github.com/genm/rackio/releases/download
```

There is still no *supported* release. The workflow refuses any version without a
pre-release suffix, because the signed cross-platform, reboot and NAT evidence in
[`release-checklist.md`](release-checklist.md) remains open, and a pre-release
publishes no `latest.txt`. The stable interface

```sh
curl --proto '=https' --tlsv1.2 -LsSf https://rackio.genm.dev/install.sh | sh
```

is therefore still a target interface, not a claim that artifacts are live. See
[`../packaging/README.md`](../packaging/README.md) for the archive contract and
rootless installer test.

## Next coherent milestones

Follow the dependency order in [`backlog.md`](backlog.md), beginning with the
Linux desktop security blocker and signed service-release evidence.

Do not move remote transport into Tauri to accelerate UI work. Keeping it in the
daemon is what preserves collection and monitoring after tray exit or logout.
