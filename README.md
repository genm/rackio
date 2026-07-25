# Tray Monitor

Tray Monitor is a cloud-independent, peer-to-peer system monitor for small
fleets. Each machine collects and owns its own metrics. Trusted viewers connect
directly over QUIC; a self-hosted relay is optional when NAT traversal cannot
produce a direct path.

This repository is an early implementation, not a production release. The core
collector, bounded history store, authenticated P2P protocol, one-time pairing,
headless daemon/CLI, Tauri tray shell, state-driven React UI, and relay packaging
are present. The release checklist documents the remaining cross-OS and NAT
matrix work.

## What works

- CPU, memory, swap, disk, network rate, uptime, OS and architecture collection
- 2-second live sampling, 10-second SQLite WAL batches, 24-hour raw and 7-day
  minute history, capped at 64 MiB
- iroh QUIC/TLS 1.3 transport with pinned endpoint identities
- strict direct-only runtime configuration with no vendor relay or DNS discovery
- optional, explicitly configured self-hosted relay
- five-minute, 256-bit, single-use pairing secrets and persisted viewer allowlist
- fail-closed unknown peer and protocol-major handling
- Unix-domain local daemon IPC, CLI operations, and Tauri tray UI
- explicit `Relayed`, stale/offline, auth, incompatibility and degraded states

Windows named-pipe IPC, signed installers, reboot persistence on all three
platforms, QR rendering, mDNS pairing discovery, active-connection teardown on
revoke, the upstream Linux Tauri/GTK `RUSTSEC-2024-0429` dependency, and the
full NAT laboratory are tracked as release blockers in
[`docs/release-checklist.md`](docs/release-checklist.md).

The current upstream `iroh 1.0.3` code also leaves vendor relay hostname
constants in the linked agent binary despite the fail-closed runtime
configuration. The release binary check deliberately blocks publication until
those constants can be removed without a private upstream fork.

## Quick start

Prerequisites are declared in [`mise.toml`](mise.toml). After installing
[`mise`](https://mise.jdx.dev/):

```sh
mise install
pnpm install
cargo run -p tray-monitor-agent -- daemon
```

In a second terminal:

```sh
cargo run -p tray-monitor-agent -- status
cargo run -p tray-monitor-agent -- pairing create
```

Run the desktop frontend:

```sh
pnpm --filter @tray-monitor/desktop dev
```

The daemon uses direct-only mode unless a relay is explicitly saved:

```sh
cargo run -p tray-monitor-agent -- relay set https://relay.example.test
```

Restart the daemon after changing relay configuration. `example.test` is only a
reserved example; replace it with the TLS hostname of your own relay.

## Workspace

| Path | Responsibility |
| --- | --- |
| `crates/monitor-core` | domain types, collectors, alerts, bounded history |
| `crates/monitor-protocol` | protobuf schema and length-delimited framing |
| `crates/monitor-iroh` | endpoint identity, pairing, allowlist, iroh adapter |
| `apps/agent` | daemon, local IPC and headless CLI |
| `apps/desktop` | Tauri tray and React viewer |
| `relay-package` | version-pinned upstream relay container and runbook |

See [`docs/architecture.md`](docs/architecture.md) and
[`docs/threat-model.md`](docs/threat-model.md) before changing transport or
pairing behavior.

## Verification

```sh
cargo nextest run --workspace
cargo clippy --workspace --all-targets -- -D warnings
pnpm format:check
pnpm typecheck
pnpm test
pnpm test:ct
pnpm build
cargo build --release -p tray-monitor-agent
scripts/check-release-binary-cloud-independence.sh
```

Machine-readable reports are written under `test-results/`. UI review evidence
is written under `output/playwright/`.

`just release-check` combines the release build and cloud-independence binary
scan. It currently fails for the upstream-hostname blocker described above.

## License

Licensed under either the Apache License, Version 2.0 or the MIT license, at your
option.
