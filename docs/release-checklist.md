# Release checklist

No build is a v1 release candidate until every unchecked item has evidence
attached to a release issue.

Implementation progress without complete release evidence is summarized in
[`current-state.md`](current-state.md). Keep a top-level box unchecked when only
a local, same-host or single-platform check exists.

Execution order, issue-sized scope, and required evidence for unchecked gates
are maintained in [`backlog.md`](backlog.md).

## Public repository identity

- [x] accept `Rackio` as the public name after reviewing the inactive,
      functionally distinct Python project with the same name
- [x] create the intended public remote, then enable private vulnerability
      reporting and required CI checks before accepting contributions
  - [x] publish `genm/rackio` with `main` as the protected default branch
  - [x] enable private vulnerability reporting, Dependabot alerts, security
        updates, secret scanning and push protection
  - [x] require CI, Security and cross-platform Rust checks on pull requests
  - [x] document release approval, publication authority and canonical
        artifact ownership in
        [`release-governance.md`](release-governance.md)
  - [x] review successful initial
        [CI](https://github.com/genm/rackio/actions/runs/30189950751) and
        [Security](https://github.com/genm/rackio/actions/runs/30189950752)
        runs in [REL-01](https://github.com/genm/rackio/issues/13)
- [x] scan the complete Git history for committed secrets locally and in
      security CI

## Functional blockers

- [ ] Windows named-pipe IPC with explicit ACL and caller verification
  - [x] implementation uses the `Rackio Viewers` group DACL, configures
        `reject_remote_clients` and rechecks the connected process token
  - [x] GitHub-hosted Windows CI verifies `Rackio Viewers` access and
        post-removal denial, and confirms the client-side pipe-name prefix
        check locally, in
        [run 30199245306](https://github.com/genm/rackio/actions/runs/30199245306)
  - [ ] clean-host verification that `reject_remote_clients` actually refuses
        a connection attempt from a separate remote host (the same-host CI
        run above never reaches the server's remote-client check)
- [ ] system service installers for Windows, macOS and Linux, including reboot
      recovery and desktop access to the local socket
  - [x] checksum-verified Linux systemd installer and rootless installer tests
  - [x] native x86_64 and arm64 Ubuntu runners verify clean systemd install,
        viewer-group access, non-member rejection, restart, preserving
        reinstall and purge in
        [run 30212938779](https://github.com/genm/rackio/actions/runs/30212938779);
        reboot, SSH client coverage and supported-distribution coverage remain
        open
  - [x] SSH client-push implementation with host-key confirmation, no server
        download, automatic pairing flow and visible failure states
  - [ ] SSH bootstrap evidence against clean Linux hosts from macOS, Windows
        and Linux clients
  - [ ] Linux reboot recovery on supported distributions
  - [x] native x86_64 and arm64 Linux archives have independently verified
        SLSA provenance in
        [run 30212938779](https://github.com/genm/rackio/actions/runs/30212938779);
        this evidence does not make the short-lived workflow artifacts a
        supported public release
  - [ ] signed macOS package and Windows service installer
    - [x] macOS pkg builder refuses release output without a signing identity
          and optionally waits for notarization and staples the ticket
    - [ ] Windows Service archive installer configures the viewer group,
          service recovery and secured ProgramData directories; the installer
          refuses to run without an explicit expected Authenticode signer
          thumbprint and rejects a mismatch, but the project's release signing
          certificate identity does not exist yet, so no pinned publisher is
          wired into a real release pipeline
    - [ ] real macOS notarization receipt and Windows Authenticode/MSI signature
- [x] immediate teardown of active connections when a peer is revoked
- [x] five-minute mDNS advertisement lifecycle during pairing only
- [x] QR and private file import/export for pairing bundles
- [x] remote peer inventory and on-demand history in the desktop viewer
  - [x] pairing import, persisted remote inventory and live metrics
  - [x] on-demand 24-hour minute history with explicit empty/error states
- [x] dynamic tray icon reflects the fleet's current worst state
- [x] viewer heartbeat transitions nodes to stale at 10 seconds and offline at
      30 seconds
- [x] path migration and relay fallback/direct recovery emit structured events
- [x] OS notifications and user-configured severity threshold
- [x] Linux desktop environments without tray support fall back to a normal
      window
- [ ] remove `glib 0.18` from the Linux Tauri dependency graph or upgrade to an
      upstream Tauri/GTK stack that resolves `RUSTSEC-2024-0429`

## NAT matrix

- [ ] same-LAN direct
- [ ] IPv6 direct
- [ ] port-forwarded IPv4 direct
- [ ] cone NAT hole punching
- [ ] strict/symmetric NAT relay fallback
- [ ] UDP blocked
- [ ] relay absent, stopped and restarted
- [ ] direct-to-relay and relay-to-direct migration
- [ ] address change and stale address

Each scenario stores JSON containing success, selected path, RTT, reconnect
duration, packet loss and relay byte count.

## Privacy and security

- [ ] direct-only packet capture has no peer-external DNS, HTTP or QUIC
  - measured by the NAT laboratory's `direct_only_isolation` scenario
    (`scripts/nat-lab/run.sh direct_only_isolation`), which places a reachable
    DNS resolver and HTTP server on the monitored machine's own LAN so the
    absence of traffic to them is a result rather than an empty segment. Read
    `scope.does_not_prove` in its report before accepting it
- [x] binary scan contains no upstream public relay/discovery hostname defaults;
      run `just release-check`
- [ ] relay cannot decode application protobuf frames
- [x] logs contain no key, pairing secret or metric payload
- [x] revoked, unknown and expired identities fail closed
- [ ] dependency, license, SBOM and vulnerability reports are clean
  - [x] Rust dependency notices are generated from the `deny.toml` policy,
        checked against `Cargo.lock`, and included in service artifacts
  - [x] JavaScript notices are generated from the locked production graph and
        bundled with the desktop application
- [ ] independent security review completed

`cargo deny` contains one scoped exception for `RUSTSEC-2024-0429`. It is an
upstream Linux Tauri/GTK dependency, not accepted release risk: **Linux desktop**
artifacts must not be published while the functional blocker above remains open.

The scope is the desktop application, not every Linux artifact. `glib 0.18`
enters the graph only through `rackio-desktop`; the published headless archive
builds `rackio-agent`, whose Linux dependency graph contains no `glib`, GTK or
Tauri crate. Verify that separation rather than assuming it:

```sh
cargo tree -p rackio-agent --target x86_64-unknown-linux-gnu -e normal |
  grep -E 'glib|gtk|tauri' && echo 'desktop dependency reached the agent' >&2
```

The command must print nothing. Any match means the headless archive no longer
excludes the advisory and must not be published.

A headless Linux agent archive may therefore be published as the evaluation
pre-release defined in [`release-governance.md`](release-governance.md). The
desktop application stays unpublished on Linux until the advisory is resolved.

The exact `iroh 1.0.3` dependency is built without default features. Rackio
clears relay transports for direct-only operation and constructs only the
custom relay variant when a self-hosted URL is configured. This keeps the
upstream production and staging relay constants out of the final LTO binary;
the artifact scan above guards against regression.

## Product acceptance

- [ ] three mixed-OS nodes appear within 5 seconds of pairing
- [ ] normal metric freshness stays below 3 seconds
- [ ] stale at 10 seconds and offline at 30 seconds
- [ ] average daemon CPU below 1% and RSS below 40 MiB
  - [ ] `mise run benchmark:agent` passes on each supported OS and its
        `test-results/resource-benchmark.json` evidence is attached
  - [ ] the active-peer profile is also measured;
        `mise run benchmark:agent-active-peer` pairs two daemons on the host,
        streams metrics between them and writes
        `test-results/resource-benchmark-active-peer.json`. The
        `idle_direct_only` profile does not cover network traffic
- [ ] normal traffic below 2 KiB/s per active peer
  - measured by the `active_peer` profile as
    `traffic.bytes_per_second_per_active_peer`. The report names the method it
    used and what the number does and does not include; read those before
    accepting the figure
- [ ] tray exit, logout and reboot do not stop collection
- [ ] storage failure, notification denial and relay outage stay visible
