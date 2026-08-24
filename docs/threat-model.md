# Threat model

## Protected assets

- endpoint private keys and pairing secrets
- live metrics and history payloads
- viewer authorization and revocation state
- accurate connection-path and health reporting
- availability of the local collector

## Trust boundaries

The local agent is trusted. The tray UI is trusted only through OS-local IPC
permissions. Remote peers, LAN discovery input, pairing bundles after expiry,
relay operators, DNS and all network traffic are untrusted. SSH-assisted
bootstrap also treats the network-provided SSH host key as untrusted until the
operator verifies its displayed fingerprint through an independent channel.

## Guarantees

- QUIC TLS 1.3 authenticates endpoint public keys and encrypts payloads end to
  end, including while packets pass through a relay.
- Application authorization pins the authenticated endpoint ID in a local
  allowlist.
- Unknown peers and protocol-major mismatches fail closed.
- Pairing secrets are random, short-lived, single-use, attempt-limited and never
  logged.
- Pairing-window mDNS publishes only the endpoint ID and reachable addresses
  under Rackio's private service name. It never publishes the Node ID, one-time
  secret, permissions or metrics. Discovery is not an authentication factor.
- Device identity is a random endpoint key plus a random UUID. Hostname, IP and
  MAC address are not identity.
- direct-only mode configures no public relay, vendor DNS discovery, gateway
  port-mapping probe or telemetry.
- metrics, keys and pairing secrets are excluded from structured logs.
- SSH bootstrap uses non-interactive SSH/SCP with strict host-key checking
  after explicit fingerprint confirmation. It uploads a locally selected
  archive and does not transfer the SSH private key or capture a password.

## Relay visibility

A relay forwards encrypted packets and cannot decrypt metric or history
payloads. It can observe endpoint IDs involved in relay connections, connection
times, source network addresses, duration and byte counts. Traffic analysis is
out of scope for v1. Operators must document retention of relay access logs.

## Relay trust anchor

By default the agent verifies a relay's TLS certificate against iroh's
compiled-in WebPKI root set, so a relay must present a certificate issued by a
publicly trusted authority. `rackio relay set <URL> --ca-certificate <PATH>`
replaces that root set, for relay connections only, with the certificates in
the named PEM file. It is a replacement, not an addition: a pinned relay is not
also accepted on a publicly issued certificate.

This is a deliberate trade, made so that an organisation self-hosting a relay on
an internal network — the fallback this product supports — can use the internal
CA it already runs. What it moves:

- **Before:** the relay's identity is vouched for by the public CA system.
  Compromise requires a publicly trusted authority to issue in error.
- **After:** the relay's identity is vouched for by one file on the monitored
  machine. Whoever can write that file chooses the authority the agent trusts
  for the relay.

Consequences of that shift:

- An attacker who can write the pinned PEM, and who can also intercept the
  relay's network path, can present a relay of their own that the agent
  accepts. They then see what any relay operator sees: endpoint IDs, timing,
  addresses and byte counts. They do **not** gain metric or history payloads —
  those stay inside the QUIC session between the two endpoints, which is
  authenticated by endpoint public keys and is unaffected by relay TLS — and
  they cannot become an authorized peer, because peer authorization is the
  local allowlist, not the relay.
- The pinned file therefore needs the protection of a trust anchor, not of a
  secret. Its contents are public; its *integrity* is what matters. Store it
  root-owned and not writable by unprivileged users, on the same footing as the
  daemon's own configuration.
- The path is stored, not the certificate. Replacing the file's contents changes
  what the agent trusts at the next daemon start, with no further operator
  action. That is what makes CA rotation possible and what makes write access to
  the file security-relevant.

The trust anchor is configuration, not a secret: its path is recorded in the
daemon's configuration and its use is reported in the startup log as
`relay_trust_anchor=pinned_ca`. The certificate's contents are never logged.

A missing, unreadable or unusable pinned CA fails closed. The configuration is
refused when it is set, and if the file becomes unusable later the daemon
refuses to start rather than falling back to the public root set — falling back
would silently restore the anchor the operator deliberately replaced. Pinning
does not make the relay an identity authority and does not make a relayed path
direct; both remain as described above.

## Known release limitations

- Unix local IPC is protected by filesystem permissions, not by a credential
  comparison: the socket is mode 0600, or 0660 and group-owned by the viewer
  group, inside a directory the viewer group can traverse but not write. The
  daemon reads the caller's credentials and refuses a connection that has none,
  but it does not compare the uid or gid against a list — the socket's mode and
  ownership are what decide who may connect. Windows local IPC is stronger: it
  rejects remote pipe clients, grants access only to LocalSystem, administrators
  and the `Rackio Viewers` local group, and then independently verifies the
  connected process token.
- OS keystore integration is not complete. The private key is always stored in
  a daemon-owned 0600 file on Unix, and that file mode is the daemon's own
  guarantee. Directory modes are the service manager's or the installer's:
  systemd's `StateDirectoryMode=0700` for `/var/lib/rackio`, re-applied on every
  start, and the macOS installer's 0750 `_rackio:_rackio-viewers` on the shared
  data directory so the viewer group can traverse it to reach the adjacent
  runtime socket. The daemon narrows a directory it creates itself, which on a
  packaged install is never the case; do not read that branch as an active
  control.
- Relay endpoint allowlisting and token delivery need an operator-facing secret
  workflow before internet exposure.
- `ssh-keyscan` cannot authenticate a host key by itself. A mistaken fingerprint
  confirmation authorizes installation on the wrong host; its verification is
  an operator responsibility, documented in [`operations.md`](operations.md).
- SSH bootstrap is an initial installation aid, not the monitoring transport.
  A successful SSH session is not proof of P2P reachability, NAT traversal or
  the identity of a later Rackio endpoint.
- Packet-capture proof of zero non-peer egress and relay payload opacity remains
  part of the release NAT/privacy matrix.

These limitations are surfaced here rather than being represented as completed
security properties.
