# Release checklist

No build is a v1 release candidate until every unchecked item has evidence
attached to a release issue.

## Functional blockers

- [ ] Windows named-pipe IPC with explicit ACL and caller verification
- [ ] system service installers for Windows, macOS and Linux, including reboot
      recovery and desktop access to the local socket
- [ ] immediate teardown of active connections when a peer is revoked
- [ ] five-minute mDNS advertisement lifecycle during pairing only
- [ ] QR and file import/export for pairing bundles
- [ ] remote peer inventory and on-demand history in the desktop viewer
- [ ] dynamic tray icon reflects the fleet's current worst state
- [ ] viewer heartbeat transitions nodes to stale at 10 seconds and offline at
      30 seconds
- [ ] path migration and relay fallback/direct recovery emit structured events
- [ ] OS notifications and user-configured alert rules
- [ ] Linux desktop environments without tray support fall back to a normal
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
- [ ] binary scan contains no upstream public relay/discovery hostname defaults;
      run `just release-check`
- [ ] relay cannot decode application protobuf frames
- [ ] logs contain no key, pairing secret or metric payload
- [ ] revoked, unknown and expired identities fail closed
- [ ] dependency, license, SBOM and vulnerability reports are clean
- [ ] independent security review completed

`cargo deny` contains one scoped exception for `RUSTSEC-2024-0429`. It is an
upstream Linux Tauri/GTK dependency, not accepted release risk: Linux artifacts
must not be published while the functional blocker above remains open.

The exact `iroh 1.0.3` dependency currently embeds its production and staging
relay hostname constants in the linked agent even when the application builds
an endpoint from the `Minimal` preset and configures only disabled or
self-hosted relays. Runtime source-policy checks pass, but the release-binary
check above intentionally fails. Do not publish an agent artifact until an
upstream release makes those defaults removable without maintaining a private
fork, or the architecture is changed and the check passes.

## Product acceptance

- [ ] three mixed-OS nodes appear within 5 seconds of pairing
- [ ] normal metric freshness stays below 3 seconds
- [ ] stale at 10 seconds and offline at 30 seconds
- [ ] average daemon CPU below 1% and RSS below 40 MiB
- [ ] normal traffic below 2 KiB/s per active peer
- [ ] tray exit, logout and reboot do not stop collection
- [ ] storage failure, notification denial and relay outage stay visible
