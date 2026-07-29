# Rackio

**Your machines, one rack.**

[![CI](https://github.com/genm/rackio/actions/workflows/ci.yml/badge.svg?branch=main)](https://github.com/genm/rackio/actions/workflows/ci.yml?query=branch%3Amain)
[![Security](https://github.com/genm/rackio/actions/workflows/security.yml/badge.svg?branch=main)](https://github.com/genm/rackio/actions/workflows/security.yml?query=branch%3Amain)
[![License: Apache-2.0 OR MIT](https://img.shields.io/badge/license-Apache--2.0%20OR%20MIT-blue.svg)](#license)

> [!WARNING]
> Rackio is under active development and has no supported production release
> yet. Source code and development builds are provided for evaluation only.

Rackio is a cloud-independent, peer-to-peer system monitor for groups of
machines. Each machine collects and owns its own metrics. Trusted viewers
connect directly over QUIC; a self-hosted relay is optional when NAT traversal
cannot produce a direct path.

The name combines a machine rack with `I/O`. It also echoes *rakkyō*, the
Japanese word for the pickled scallion, giving Rackio a playful visual motif.

This repository is a release-candidate implementation, not yet a published
production release. The core collector, bounded history store, authenticated
P2P protocol, one-time pairing, headless daemon/CLI, Tauri tray shell,
state-driven React UI, and relay packaging are present. The release checklist
documents the remaining cross-OS and NAT matrix work.

For a handoff-oriented view of completed and partial behavior, read
[`docs/current-state.md`](docs/current-state.md). An implementation listed
there is not release-complete until [`docs/release-checklist.md`](docs/release-checklist.md)
has the required evidence. Remaining work is organized into issue-sized,
dependency-ordered items in [`docs/backlog.md`](docs/backlog.md).
Contribution workflow, issue ownership, and release evidence conventions are in
[`CONTRIBUTING.md`](CONTRIBUTING.md). Participation is governed by the
[code of conduct](CODE_OF_CONDUCT.md).

## What works

- CPU, memory, swap, disk, network rate, uptime, OS and architecture collection
- 2-second live sampling, 10-second SQLite WAL batches, 24-hour raw and 7-day
  minute history, capped at 64 MiB
- iroh QUIC/TLS 1.3 transport with pinned endpoint identities
- strict direct-only runtime configuration with no vendor relay or DNS discovery
- optional, explicitly configured self-hosted relay
- five-minute, 256-bit, single-use pairing secrets and persisted viewer allowlist
- Desktop and CLI pairing-bundle import with persisted remote machine inventory
- live remote metrics, truthful LAN/WAN/relay path, RTT, stale and offline state
- fail-closed unknown peer and protocol-major handling
- peer-credential-gated Unix socket or explicit-DACL Windows named-pipe local
  IPC, CLI operations, and Tauri tray UI
- checksum-verified Linux release packaging and an idempotent systemd installer
- explicit `Relayed`, stale/offline, auth, incompatibility and degraded states

Real signed/notarized installers, reboot persistence on all three platforms,
Windows integration evidence, the upstream Linux Tauri/GTK
`RUSTSEC-2024-0429` dependency, and the full NAT laboratory are tracked as
release blockers in
[`docs/release-checklist.md`](docs/release-checklist.md).

The release build disables iroh default features, bypasses every vendor relay
mode, and contains no upstream public relay or discovery hostname. The
fail-closed binary scan verifies that property without maintaining an upstream
fork.

## Quick start

Prerequisites and tasks are declared in [`mise.toml`](mise.toml). After
installing [`mise`](https://mise.jdx.dev/):

```sh
mise trust mise.toml
mise run bootstrap
mise run agent:daemon
```

In a second terminal:

```sh
mise exec -- cargo run -p rackio-agent -- status
mise exec -- cargo run -p rackio-agent -- pairing create
```

On the viewing machine, paste the bundle into **Pair machine** in the desktop
app, or import it through the CLI:

```sh
mise exec -- cargo run -p rackio-agent -- pairing import 'rackio-pair:...'
mise exec -- cargo run -p rackio-agent -- fleet
```

Run the desktop frontend:

```sh
mise run desktop:dev
```

The daemon uses direct-only mode unless a relay is explicitly saved:

```sh
cargo run -p rackio-agent -- relay set https://relay.example.test
```

Restart the daemon after changing relay configuration. `example.test` is only a
reserved example; replace it with the TLS hostname of your own relay.

## Installation and operations

Published evaluation pre-releases of the headless Linux agent install without a
local build:

```sh
sh install.sh --version <VERSION> \
  --releases-url https://github.com/genm/rackio/releases/download
```

They are immutable GitHub pre-releases with per-architecture checksums and
build-provenance attestations, and they are explicitly unsupported: no version
pointer advertises them, and the publication workflow refuses to create a
supported release.

The end-to-end operator workflow—source evaluation, intended Linux server
installation, pairing, SSH-assisted air-gapped bootstrap, relay configuration,
state interpretation and decommissioning—is in
[`docs/operations.md`](docs/operations.md). It also makes the current
no-public-release status and the distinction between checksum verification and
independent release provenance explicit.

The release archive contract and package-specific build details remain in
[`packaging/README.md`](packaging/README.md).
Release approval, immutable artifact ownership, and the planned relationship
between GitHub Releases and `rackio.genm.dev` are defined in
[`docs/release-governance.md`](docs/release-governance.md).

## Workspace

| Path | Responsibility |
| --- | --- |
| `crates/rackio-core` | domain types, collectors, alerts, bounded history |
| `crates/rackio-protocol` | protobuf schema and length-delimited framing |
| `crates/rackio-iroh` | endpoint identity, pairing, allowlist, iroh adapter |
| `apps/agent` | daemon, local IPC and headless CLI |
| `apps/desktop` | Tauri tray and React viewer |
| `relay-package` | version-pinned upstream relay container and runbook |

See [`docs/architecture.md`](docs/architecture.md) and
[`docs/threat-model.md`](docs/threat-model.md) before changing transport or
pairing behavior. Development setup and platform prerequisites are in
[`docs/development.md`](docs/development.md).

## Verification

```sh
mise run check
```

Machine-readable reports are written under `test-results/`. UI review evidence
is written under `output/playwright/`.

`just release-check` combines the release build and cloud-independence binary
scan. It must pass before any release artifact is published.

## License

Licensed under either the Apache License, Version 2.0 or the MIT license, at your
option.
