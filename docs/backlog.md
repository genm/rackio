# Release backlog

This document turns the remaining release gates into issue-sized work. The
authoritative completion state remains [`release-checklist.md`](release-checklist.md);
do not maintain a second set of checkboxes here.

When the public GitHub remote exists, create one issue per backlog ID, keep the
ID in the issue title, and attach the resulting evidence to that issue. Closing
an issue does not by itself satisfy a release gate: update the release checklist
only after its evidence has been reviewed.

## Execution order

| Order | ID | Work item | Depends on | Suggested labels |
| --- | --- | --- | --- | --- |
| 1 | REL-01 | Create and secure the public GitHub repository | — | `release`, `governance` |
| 2 | REL-02 | Resolve the Linux Tauri/GTK security blocker | — | `security`, `linux`, `desktop` |
| 3 | REL-03 | Verify and sign the Linux service release | REL-01 | `release`, `linux`, `packaging` |
| 4 | REL-04 | Verify and sign the macOS service release | REL-01 | `release`, `macos`, `packaging` |
| 5 | REL-05 | Verify and sign the Windows service release | REL-01 | `release`, `windows`, `packaging` |
| 6 | REL-06 | Execute the direct, NAT, and relay network matrix | REL-03 | `network`, `p2p`, `test` |
| 7 | REL-07 | Prove privacy and relay payload opacity | REL-06 | `security`, `privacy`, `test` |
| 8 | REL-08 | Execute mixed-OS product and resource acceptance | REL-03, REL-04, REL-05, REL-06 | `release`, `performance`, `test` |
| 9 | REL-09 | Complete independent security review and release decision | REL-02 through REL-08 | `security`, `release` |

## REL-01: Create and secure the public GitHub repository

Objective: establish the public collaboration and release authority without
changing repository history.

Scope:

- choose the owning GitHub organization and create the public repository;
- add `origin` and push the current `main` history;
- enable private vulnerability reporting and Dependabot alerts;
- require the existing CI and Security workflows on `main`;
- configure branch rules, release permissions, and contributor-facing metadata;
- record where `rackio.genm.dev` release artifacts will be published.

Evidence required:

- public repository URL and default-branch protection screenshot or API output;
- successful CI and Security runs from the public repository;
- vulnerability-reporting URL;
- documented release-role ownership.

## REL-02: Resolve the Linux Tauri/GTK security blocker

Objective: remove the scoped `RUSTSEC-2024-0429` exception rather than accepting
it as release risk.

Scope:

- evaluate an upstream Tauri/GTK upgrade or another maintained Linux desktop
  backend;
- preserve tray fallback, notifications, pairing, SSH bootstrap, and local IPC;
- remove the `glib 0.18` path and the corresponding `deny.toml` exception;
- rerun component tests and a real Linux desktop smoke test.

Evidence required:

- `cargo tree -i glib@0.18.5` has no path;
- `cargo deny check` succeeds without the exception;
- Linux tray-present and tray-unavailable screenshots;
- Linux desktop bundle starts and connects to its system daemon.

## REL-03: Verify and sign the Linux service release

Objective: turn the tested installer contract into a reproducible, signed
headless release.

Scope:

- build `x86_64-unknown-linux-gnu` and `aarch64-unknown-linux-gnu` artifacts;
- add independent artifact signature or provenance verification;
- exercise `curl | sh` and client-pushed SSH installation on clean supported
  distributions;
- verify systemd enablement, reboot recovery, viewer-group socket access,
  upgrade, preserving uninstall, and purge uninstall;
- publish immutable artifacts, checksums, SBOM, and license notices.

Evidence required:

- clean-host matrix with distribution, architecture, installer path, and result;
- signature/provenance verification output;
- reboot and post-reboot `rackio status` output;
- public immutable artifact URLs and checksums.

## REL-04: Verify and sign the macOS service release

Objective: produce a notarized macOS package whose daemon survives logout and
reboot while the desktop retains local IPC access.

Scope:

- build and sign the agent and package with Developer ID identities;
- submit for notarization, wait, staple, and validate;
- install on a clean supported macOS host;
- verify `_rackio` ownership, `rackio-viewers` access, LaunchDaemon recovery,
  logout behavior, upgrade, preserving uninstall, and purge uninstall.

Evidence required:

- `codesign`, `pkgutil`, `spctl`, and notarization outputs;
- stapled package checksum;
- clean-install, logout, reboot, upgrade, and uninstall results;
- desktop-to-daemon IPC proof after reboot.

## REL-05: Verify and sign the Windows service release

Objective: validate the explicit-DACL named pipe and signed Windows Service
package on a clean Windows host.

Scope:

- run the Windows named-pipe integration job in GitHub Actions;
- Authenticode-sign and timestamp the executable and release archive;
- install on a clean supported Windows host;
- verify `Rackio Viewers` access, rejection of unauthorized and remote pipe
  clients, Service recovery, reboot, upgrade, and uninstall;
- confirm the tray application reconnects after login.

Evidence required:

- successful Windows CI URL;
- `Get-AuthenticodeSignature` and timestamp verification output;
- allowed, denied, and remote-client IPC results;
- clean-install, reboot, recovery, upgrade, and uninstall results.

## REL-06: Execute the direct, NAT, and relay network matrix

Objective: prove path selection and reconnection behavior with network
conditions that match the product contract.

Scenarios:

- same-LAN direct and IPv6 direct;
- port-forwarded IPv4 direct;
- cone-NAT hole punching;
- strict or symmetric NAT relay fallback;
- UDP blocked;
- relay absent, stopped, and restarted;
- direct-to-relay and relay-to-direct migration;
- address change and stale address.

Evidence required for every scenario:

- machine-readable JSON containing connection result, selected path, RTT,
  reconnect duration, packet loss, and relay byte count;
- structured path-transition events;
- packet capture or topology description sufficient to reproduce the result;
- explicit confirmation that relayed paths are never reported as direct.

## REL-07: Prove privacy and relay payload opacity

Objective: validate cloud independence and E2E confidentiality from observable
network evidence.

Scope:

- capture direct-only traffic and reject peer-external DNS, HTTP, or QUIC;
- capture relay traffic and demonstrate that the relay cannot decode protobuf
  frames or metric values;
- stop the relay and prove that direct peers continue while relay-only peers
  become offline;
- repeat key, pairing-secret, metric-payload, and complete-history secret scans.

Evidence required:

- packet captures with a documented allowlist and automated assertion output;
- relay logs and captures showing metadata visibility but no plaintext payload;
- relay outage state-transition results;
- clean Gitleaks, Trivy, runtime-log, and binary cloud-independence reports.

## REL-08: Execute mixed-OS product and resource acceptance

Objective: prove the user-visible contract on a three-machine Windows, Linux,
and macOS rack.

Scope:

- pair all three machines and measure appearance and metric freshness;
- verify stale at 10 seconds and offline at 30 seconds;
- measure release-daemon CPU and RSS on every OS;
- measure total traffic below 2 KiB/s per active peer;
- verify tray exit, logout, and reboot do not stop collection;
- exercise storage failure, notification denial, relay outage, auth error, and
  protocol incompatibility without false healthy or zero states.

Evidence required:

- timestamped mixed-OS test report and screenshots;
- resource JSON from `mise run benchmark:agent` on every OS;
- packet-byte measurements for idle and active peers;
- degraded-state screenshots and structured logs.

## REL-09: Complete independent security review and release decision

Objective: make a documented go/no-go decision from all implementation and
environment evidence.

Scope:

- review identity storage, pairing, allowlists, protocol framing, IPC ACLs,
  SSH bootstrap, installers, updater assumptions, relay metadata, and CI;
- confirm every dependency, license, SBOM, vulnerability, and secret report;
- resolve findings or explicitly block the release;
- review every release-checklist gate and link its evidence issue.

Evidence required:

- reviewer identity and dated review report;
- findings with severity and resolution links;
- completed release checklist;
- signed release decision identifying the exact commit and artifact checksums.
