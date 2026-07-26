# Installation and operations

This guide is the operator-facing source of truth for installing, pairing and
maintaining Rackio. It describes the intended release workflow as well as the
current source-evaluation workflow. Release eligibility is decided only by
[`release-checklist.md`](release-checklist.md).

> [!WARNING]
> Rackio has no supported public production release yet. In particular,
> `rackio.genm.dev` is a planned distribution endpoint, not evidence that an
> installer or release artifact is available there. Do not use the commands
> below against production machines until the release checklist records the
> required signed-artifact, platform and network evidence. GitHub Release
> ownership, release approval and custom-domain publication are defined in
> [`release-governance.md`](release-governance.md).

## Operating model

Every machine runs the Rackio agent. The agent collects its own metrics,
retains only its own history and owns its endpoint private key. A desktop is
both a viewer and, when its own agent runs, a monitored machine. The tray talks
only to its local agent; agents talk to each other over authenticated QUIC.

```mermaid
flowchart LR
    U["Rackio desktop / tray"] <-->|"OS-local IPC"| A["Local agent"]
    A <-->|"Direct QUIC + TLS 1.3"| B["Remote agent"]
    A <-->|"Encrypted fallback only"| R["Your self-hosted relay"]
    R <-->|"Encrypted fallback only"| B
```

No Rackio account, hosted controller, vendor discovery service or public relay
is contacted. A relay is optional and carries encrypted packets only; it can
still observe connection metadata. See [`architecture.md`](architecture.md)
and [`threat-model.md`](threat-model.md) before exposing a relay to the
internet.

## Choose an installation path

| Situation | Path | What it requires |
| --- | --- | --- |
| Developer evaluation on any supported desktop | Run from the source checkout | `mise`, the host prerequisites in [`development.md`](development.md) and a local build |
| Headless Linux after a supported release exists | Download installer or use the documented HTTPS command | systemd, `x86_64` or `aarch64`, root or `sudo` |
| A Linux host cannot reach a release server | Desktop **Pair machine → SSH** | Existing SSH key/agent access, verified host key, local archive plus checksum, and non-interactive root access |
| macOS or Windows agent | Signed platform package | Not yet supported for public installation; see the release checklist |

Do not substitute an SSH bootstrap or a self-hosted relay for release signing.
They solve different problems: SSH assists an initial Linux installation, while
the P2P transport is used after pairing.

## Evaluate from source

For a local evaluation, use the pinned toolchain:

```sh
mise trust mise.toml
mise run bootstrap
mise run agent:daemon
```

In a second terminal, inspect the local agent and create a one-time pairing
bundle:

```sh
mise exec -- cargo run -p rackio-agent -- status
mise exec -- cargo run -p rackio-agent -- pairing create
```

The bundle expires after five minutes, can be used only once, and grants only
the viewer direction. Treat it like a short-lived secret: transfer it over a
trusted channel, do not paste it into tickets or chat logs, and create a new
one when in doubt.

On the viewer, use **Pair machine** and paste, scan or select the bundle file.
The equivalent CLI flow is:

```sh
mise exec -- cargo run -p rackio-agent -- pairing import 'rackio-pair:...'
mise exec -- cargo run -p rackio-agent -- fleet
```

The first remote metric normally appears within a few seconds for a reachable
peer. `lan_direct`, `wan_direct` and `relayed` are connection-path results,
not user-selected labels. A failed or expired import must remain an error; it
must not create a healthy-looking machine entry.

## Headless Linux release installation

After a signed release has been published, the short form will be:

```sh
curl --proto '=https' --tlsv1.2 -LsSf https://rackio.genm.dev/install.sh | sh
```

For an auditable execution, download and inspect the script first:

```sh
curl --proto '=https' --tlsv1.2 -O https://rackio.genm.dev/install.sh
less install.sh
sh install.sh
```

The installer downloads a versioned archive for `x86_64` or `aarch64`, checks
its SHA-256 digest before running the binary, rejects unsafe archive paths,
installs the systemd service, and fails unless the service and its local health
check succeed. It creates a non-login `rackio` user and a `rackio-viewers`
group. Log out and back in before using Rackio without `sudo`, so your desktop
process has its new group membership.

The installer is intentionally Linux/systemd-only. Its archive layout,
checksum limitations, package build command and rootless integration test are
specified in [`../packaging/README.md`](../packaging/README.md). A checksum
retrieved from the same origin detects transfer corruption and asset mismatch;
it is not independent provenance. Do not treat it as a substitute for the
required release signature or attestation.

After installation:

```sh
sudo rackio status
sudo rackio pairing create
sudo journalctl -u rackio.service -e
```

The installed agent starts in direct-only mode. Configure a relay only when
you operate or explicitly trust its URL:

```sh
sudo rackio relay set https://relay.example.test
sudo systemctl restart rackio.service
```

`example.test` is a reserved documentation hostname. A relay endpoint does
not make the relay an identity authority and does not make a relayed path
direct.

## SSH-assisted Linux bootstrap

The desktop’s **Pair machine → SSH** path is for a trusted operator who already
has administrative SSH access to a Linux server. It uploads a local release
archive, its checksum and the installer to a temporary server directory. The
server verifies the archive locally, installs the systemd service, opens a
one-time pairing window, and the desktop imports the returned bundle over the
normal P2P pairing path. The server does not download Rackio from the internet.

Before using it, prepare:

- a Linux systemd host with `x86_64` or `aarch64` architecture;
- key or SSH-agent authentication (optionally choose a local identity file);
- either `root` login or passwordless `sudo` for the SSH user;
- the exact local `.tar.gz` release archive and its matching `.sha256` file;
- the server’s SSH host-key fingerprint obtained through a trusted, independent
  channel.

The desktop deliberately uses non-interactive SSH (`BatchMode=yes`); it never
prompts for, captures or stores an SSH password. It displays the fingerprint
returned by the server, requires an explicit confirmation, rechecks the host
key immediately before the upload, then uses strict host-key checking for SSH
and SCP. A host-key change stops the installation.

`ssh-keyscan` only obtains a key presented on the network; it does **not**
authenticate that key. Verify the displayed fingerprint through an existing
trusted path such as a console, inventory system, or an administrator you can
independently reach. The accepted keys are stored in Rackio’s local application
configuration so later SSH/SCP calls stay pinned. If a connection drops before
cleanup, inspect and remove only the matching `/tmp/rackio-bootstrap.*`
directory on the target after confirming it belongs to this operation.

SSH is solely the first-install transport. Once pairing succeeds, monitoring
uses Rackio’s pinned-endpoint QUIC connection and follows the direct-first,
self-hosted-relay-fallback policy. Do not interpret a successful SSH session as
proof that P2P reachability or NAT traversal will work; the desktop reports the
actual P2P path separately.

## Health and incident interpretation

| UI state | Meaning | First operator action |
| --- | --- | --- |
| `healthy` | Fresh metrics and no active health warning | None |
| `warning` / `critical` | A configured local health threshold was crossed | Open the machine detail and inspect the affected resource |
| `stale` | No metric or heartbeat for 10 seconds | Check path, RTT and local agent logs; preserve last known values |
| `offline` | No metric or heartbeat for 30 seconds | Check agent process, network reachability and relay availability if shown as relayed |
| `auth_error` | The remote agent rejected this viewer | Confirm endpoint pairing and allowlist; do not retry with a reused bundle |
| `incompatible` | Protocol major versions differ | Upgrade or roll back a machine to a compatible release |
| `degraded` | A collector, storage, notification or local dependency failed | Read the displayed error and structured agent logs; values are not silently zeroed |

When storage is degraded, live sampling continues in memory but history may not
persist. When a relay is unavailable, a direct peer may stay connected while a
relay-only peer becomes offline. Those are distinct conditions, not a general
fleet failure.

## Back up, revoke and remove

Rackio intentionally keeps each machine’s own identity, pairing state and
history local. A preserving uninstall removes the service and binary but leaves
`/etc/rackio`, `/var/lib/rackio` and `/var/log/rackio` in place for recovery:

```sh
sudo /usr/local/lib/rackio/uninstall.sh
```

Use purge only when you intend to permanently discard the local identity,
pairing records, metrics and logs:

```sh
sudo /usr/local/lib/rackio/uninstall.sh --purge
```

Before decommissioning a machine, revoke it from every viewer that monitors it.
Revocation cuts active connections immediately. Backups containing
`identity.key`, `peers.json`, `monitored-machines.json` or `metrics.sqlite3`
are sensitive: protect them as machine credentials and monitoring data, and do
not copy a private key to a second running machine.

## Evidence and support boundary

Run `mise run check` for source-tree verification. The focused tests are
`mise run test:pairing` (live pairing, replay rejection and reconnect) and
`mise run test:installer` (normal install, idempotent reinstall and checksum
rejection). They are implementation evidence, not a claim of a supported
release or a substitute for the cross-OS and NAT matrix.

For a failure that may expose keys, pairing bundles, metrics or routable
addresses, follow [`../SECURITY.md`](../SECURITY.md) rather than posting those
details publicly.
