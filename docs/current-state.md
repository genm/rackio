# Current implementation state

This is the handoff-oriented snapshot of the repository. Product invariants
live in [`architecture.md`](architecture.md) and
[`threat-model.md`](threat-model.md); release eligibility lives in
[`release-checklist.md`](release-checklist.md). Implemented code listed here
does not make a release-checklist item complete without its required
cross-platform, network or supply-chain evidence.

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
7. render state, metrics, connection path and RTT in the desktop rack.
8. inspect and explicitly confirm an SSH host key, push a local Linux release
   archive, verify and install it without server internet access, then import
   the resulting one-time pairing bundle automatically.

A reusable two-daemon same-host smoke exercises this flow: pairing produces one
`lan_direct` remote machine with a live CPU sample, importing the same bundle
again fails, and restarting the viewer daemon reconnects from its secret-free
record. That is useful integration evidence, but it is not a substitute for the
NAT and mixed-OS release matrices.

## Capability matrix

| Surface | Implemented now | Missing or incomplete |
| --- | --- | --- |
| Collector | CPU, memory, swap, disk, network rate, uptime, OS and architecture | GPU, temperature, processes, containers and logs are not implemented |
| Local history | 2-second samples, SQLite WAL batches, raw/minute retention and 64 MiB pruning | Cross-machine long-term aggregation is intentionally absent |
| Remote server | Endpoint authentication, allowlist authorization, immediate revoke teardown, node info, live metrics, health, path and bounded history protocol | NAT and relay migration evidence remains incomplete |
| Pairing | Five-minute, attempt-limited, single-use secret, window-scoped mDNS endpoint advertisement, copy/paste import, local QR generation and private file import/export | Cross-LAN pairing depends on transferred direct addresses or a configured self-hosted relay |
| Viewer daemon | Secret-free remote inventory, reconnect loop, persisted last-known snapshot, bounded remote history query, periodic health/path refresh, structured path events and stale/offline derivation | NAT and relay migration evidence remains incomplete |
| Desktop | Local/remote cards, 24-hour remote history detail, dynamic worst-state tray icon, configurable OS notifications, QR/file/bundle pairing, SSH bootstrap, and explicit degraded states | Cross-platform packaging |
| Linux packaging | Release archive builder, HTTPS or client-pushed archive install, checksum verification, hardened systemd unit and preserving/purge uninstaller | No signed public release; reboot and distribution coverage are not proven |
| macOS | LaunchDaemon template | Signed/notarized package, ownership rollback and installer receipts |
| Windows | Collector and remote endpoint code participate in workspace checks | Named-pipe IPC ACL, Windows Service and installer |
| Relay | Version-pinned upstream relay container and configuration | Internet-exposure workflow, token delivery and NAT evidence |

## Persistence and directionality

The daemon data directory owns:

- `identity.key`: the endpoint private key;
- `node-id`: the random application node UUID;
- `metrics.sqlite3`: only this machine's metric history;
- `peers.json`: inbound viewers allowed to read this machine;
- `monitored-machines.json`: outbound machines this daemon views.

`monitored-machines.json` stores the pinned endpoint ID, direct addresses,
explicit relay URLs, basic node information and pairing time. It never stores
the one-time pairing secret.

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

The Linux installer contract is implemented, but there is no supported public
release to install yet. The intended interface is:

```sh
curl --proto '=https' --tlsv1.2 -LsSf https://rackio.genm.dev/install.sh | sh
```

That URL is a target interface, not a claim that artifacts are live. Publication
remains blocked by [`release-checklist.md`](release-checklist.md), including the
upstream iroh hostname scan and independent artifact signature/provenance. See
[`../packaging/README.md`](../packaging/README.md) for the archive contract and
rootless installer test.

## Next coherent milestones

1. Finish a publishable Linux headless supply chain: reproducible artifacts,
   signature/provenance, SBOM, immutable hosting and reboot evidence.
2. Run same-LAN, NAT, relay-outage and path-migration matrices.
3. Finish macOS and Windows service/IPC packaging.

Do not move remote transport into Tauri to accelerate UI work. Keeping it in the
daemon is what preserves collection and monitoring after tray exit or logout.
