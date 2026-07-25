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
relay operators, DNS and all network traffic are untrusted.

## Guarantees

- QUIC TLS 1.3 authenticates endpoint public keys and encrypts payloads end to
  end, including while packets pass through a relay.
- Application authorization pins the authenticated endpoint ID in a local
  allowlist.
- Unknown peers and protocol-major mismatches fail closed.
- Pairing secrets are random, short-lived, single-use, attempt-limited and never
  logged.
- Device identity is a random endpoint key plus a random UUID. Hostname, IP and
  MAC address are not identity.
- direct-only mode configures no public relay, vendor DNS discovery, gateway
  port-mapping probe or telemetry.
- metrics, keys and pairing secrets are excluded from structured logs.

## Relay visibility

A relay forwards encrypted packets and cannot decrypt metric or history
payloads. It can observe endpoint IDs involved in relay connections, connection
times, source network addresses, duration and byte counts. Traffic analysis is
out of scope for v1. Operators must document retention of relay access logs.

## Known limitations before v1 release

- The current local IPC implementation is Unix-domain only and owner mode 0600.
  Windows named-pipe ACL enforcement is a release blocker.
- Revoking a peer removes future authorization but does not yet track and close
  every existing QUIC connection immediately.
- mDNS pairing advertisement and its five-minute lifecycle are not implemented.
- OS keystore integration is not complete. The private key is stored in a
  daemon-owned 0600 file inside a 0700 directory on Unix.
- Relay endpoint allowlisting and token delivery need an operator-facing secret
  workflow before internet exposure.
- Packet-capture proof of zero non-peer egress and relay payload opacity remains
  part of the release NAT/privacy matrix.

These limitations are surfaced here rather than being represented as completed
security properties.
