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
| Headless Linux evaluation now | Install a published pre-release from its GitHub Release | systemd, `x86_64` or `aarch64`, root or `sudo`, and acceptance that it is unsupported |
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

## Headless Linux evaluation pre-release

An evaluation pre-release is a real, installable build of the headless Linux
agent published as an immutable GitHub pre-release. It removes the need to build
an archive yourself before using either the `curl`-based install or the
desktop's SSH bootstrap. It is not a supported release: the signed macOS and
Windows packages, reboot recovery, and the NAT matrix in
[`release-checklist.md`](release-checklist.md) are still open. Do not run it on
a machine whose monitoring you depend on.

Download the installer and the archive from the release, inspect the installer,
then install the exact version:

```sh
tag=v0.1.0-rc.1
curl --proto '=https' --tlsv1.2 -O "https://github.com/genm/rackio/releases/download/$tag/install.sh"
less install.sh
sh install.sh --version "${tag#v}" \
  --releases-url https://github.com/genm/rackio/releases/download
```

`--version` is mandatory here. A pre-release publishes no version pointer, so a
plain `latest` install cannot select it and fails with that explanation instead
of silently picking another build.

Verify provenance independently before trusting the archive. The checksum
detects transfer corruption and asset mismatch; the GitHub attestation is what
ties the archive to the workflow and commit that produced it:

```sh
gh attestation verify rackio-v0.1.0-rc.1-x86_64-unknown-linux-gnu.tar.gz \
  --repo genm/rackio
```

The desktop SSH bootstrap accepts the same downloaded `.tar.gz` and `.sha256`
pair, which is the fastest way to install and pair a server that has no
internet access.

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

### Reach a relay signed by an internal certificate authority

The command above expects the relay to present a publicly trusted TLS
certificate: the agent verifies it against a compiled-in WebPKI root set. A
relay on an internal network usually cannot have one — no public authority
issues for an internal-only name — while the organisation running that network
usually does operate its own CA.

Pin that CA on each monitored machine, alongside the relay URL:

```sh
sudo install -o root -g root -m 0644 relay-ca.pem /etc/rackio/relay-ca.pem
sudo rackio relay set https://relay.internal.example.test \
  --ca-certificate /etc/rackio/relay-ca.pem
sudo systemctl restart rackio.service
sudo rackio status   # the machine reaches the relay, or reports why it cannot
```

Points to get right:

- **Supply the issuing authority's certificate**, PEM encoded — not the relay's
  own certificate, not a private key. Include intermediates the relay does not
  send; every `CERTIFICATE` block in the file is used, which is also how you
  keep the old and new anchors valid across a CA rotation.
- **Give an absolute path.** The daemon reads the file, from its own working
  directory rather than your shell's, at every start.
- **Make the relay's certificate match the URL.** Its Subject Alternative Name
  must contain the exact host in the relay URL. Pinning the CA does not relax
  hostname verification.
- **Protect the file's integrity, not its secrecy.** A CA certificate is
  public. Whoever can write this file chooses what the agent trusts for the
  relay, so keep it root-owned and not writable by unprivileged users. The
  trade this makes is stated in [`threat-model.md`](threat-model.md).

The pin **replaces** the public root set for relay connections rather than
adding to it, so a pinned relay is not also accepted on a publicly issued
certificate.

`rackio relay set` refuses a CA file that is missing, unreadable, or not a
usable certificate authority, naming the file and the correction; the relay
configuration you already had is left exactly as it was. If the file later
becomes unusable the daemon refuses to start rather than falling back to the
public root set — an unusable relay is visibly unusable, never a quietly
widened trust anchor. The startup log records which anchor is in use as
`relay_trust_anchor=pinned_ca` or `relay_trust_anchor=webpki`; the certificate
itself is never logged.

Setting up the relay side is described in
[`../relay-package/README.md`](../relay-package/README.md).
`sudo rackio relay set direct-only` clears the relay and its pinned CA
together.

## Keep a monitored machine reachable across restarts

By default an agent listens on an ephemeral UDP port, which the OS reassigns on
every restart. Viewers hold the direct addresses they were paired on, so a
restarted machine that moves to another port is unreachable to them until it is
paired again. On any machine other operators monitor over a direct path,
configure a fixed listen port:

```sh
sudo rackio listen-port set 7777
sudo systemctl restart rackio.service
sudo rackio status   # `bind_port` and every direct address show the fixed port
```

The setting takes effect on the next start. If the port is already in use the
daemon fails to start and says so; it never falls back to an ephemeral port,
because that would silently strand the viewers this setting exists to protect.
`sudo rackio listen-port set ephemeral` returns to an OS-assigned port.

A viewer whose stored addresses stop answering reports the machine as `offline`
with a message naming this command. It keeps the last known values and never
reports the machine as healthy. Recovering means putting the machine back on an
address the viewer knows — by restoring the port or forwarding rule — or pairing
again; do not hand-edit `monitored-machines.json`.

When a relay is configured, viewers also reconnect through the relay and learn
the machine's current direct address from that authenticated session, so a
direct path is restored without re-pairing.

## Pair a machine behind NAT

A machine behind a router only sees its own LAN addresses, so a bundle it
creates carries `192.168.x.y:PORT` — an address no viewer outside that LAN can
use. Rackio does not discover the router's external address: nothing is probed
and no discovery service is contacted. Tell the agent the address you already
know instead.

On the monitored machine, fix the listen port, forward that same UDP port on the
router to this machine, then advertise the forwarded address:

```sh
sudo rackio listen-port set 41641
sudo systemctl restart rackio.service
sudo rackio advertise-address add 198.51.100.7:41641   # the router's forwarded address
sudo systemctl restart rackio.service
sudo rackio advertise-address list
sudo rackio pairing create
```

The bundle then carries the machine's own interface addresses *and* the
advertised ones, so the same bundle pairs from inside the LAN and from outside
it. Advertised addresses take effect for bundles created afterwards; already
paired viewers keep the addresses they were given.

The restart above is what hands the address to the endpoint itself, which is
why `advertise-address add` and `remove` report `restart_required`. After it,
`rackio status` lists the advertised address among its `direct_addresses` — the
way to confirm from the machine that the setting took effect — and the address
is a candidate for path selection, so a session that fell back to a relay can
return to a direct path without waiting for the next connection. Up to eight
addresses are
kept — adding a ninth is refused, naming the address to remove first, rather
than dropping one silently. `sudo rackio advertise-address remove
198.51.100.7:41641` stops advertising one.

Rackio stores the address exactly as given. It never resolves a hostname,
probes the address, or asks anything on the internet to confirm it, so a typo or
a forwarding rule that is not in place is not corrected: it behaves as an
ordinary unreachable candidate, and the viewer reports the machine as `offline`
with its recovery hint. Verify the forwarding rule on the router itself.

Nothing here discovers an address for you. It works when the operator knows a
stable UDP address for the machine — a forwarded port, or the address an
endpoint-independent ("cone") NAT maps it to. With a self-hosted relay
configured on both sides to carry the address exchange, two such machines can
open a direct path between them without any port forward. A machine behind
carrier-grade NAT or a symmetric NAT, whose mapped port changes per
destination, has no stable address to advertise and is reached through the
relay.

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
| `warning` / `critical` | A local health threshold was crossed | Read the detail line on the card, which names the resource, its value and the threshold |
| `stale` | No metric or heartbeat for 10 seconds | Check path, RTT and local agent logs; preserve last known values |
| `offline` | No metric or heartbeat for 30 seconds | Check agent process, network reachability and relay availability if shown as relayed; if the machine restarted on a new port, give it a fixed listen port |
| `auth_error` | The remote agent rejected this viewer | Confirm endpoint pairing and allowlist; do not retry with a reused bundle |
| `incompatible` | Protocol major versions differ | Upgrade or roll back a machine to a compatible release |
| `degraded` | A collector, storage, notification or local dependency failed | Read the displayed error and structured agent logs; values are not silently zeroed |

### Local health thresholds

Rackio ships the levels that are generally worth acting on, so an untouched
machine still reports trouble:

| Rule | Condition | Sustained for | State |
| --- | --- | --- | --- |
| `disk-capacity-warning` | fullest filesystem at or above 90 % | 6 s | `warning` |
| `disk-capacity-critical` | fullest filesystem at or above 95 % | 6 s | `critical` |
| `memory-pressure-warning` | memory in use at or above 90 % | 1 min | `warning` |
| `memory-pressure-critical` | memory in use at or above 97 % | 1 min | `critical` |
| `cpu-saturation-warning` | CPU at or above 90 % | 5 min | `warning` |
| `temperature-headroom-warning` | within 5 °C of the hardware's own limit | 30 s | `warning` |
| `temperature-critical` | at or past the hardware's own limit | 30 s | `critical` |

The sustained window is what separates a busy machine from one in trouble: a
compile pins the CPU, a backup fills a cache, and neither should page anyone.
Temperature is measured against the limit the hardware itself publishes, never
a number Rackio picked — a machine whose OS publishes no limit resolves neither
temperature rule instead of being judged against a guess. `disk_percent` is the
fullest mounted filesystem, and every alert publishes a line naming what
crossed, for example `Disk /data 93% is at or above the warning threshold of
90%`, which is what the desktop shows on the card and sends in the OS
notification.

CPU saturation is the rule build, render and simulation machines most often
switch off; that is what `rackio alerts disable` is for.

#### Changing the levels

```bash
rackio alerts list
```

```bash
sudo rackio alerts set disk-capacity-warning --threshold 80
```

`set` accepts `--threshold`, `--samples` (two-second samples in a row),
`--severity warning|critical`, and — when defining a rule Rackio does not ship —
`--metric` and `--comparison at-or-above|at-or-below`. Omitted options keep
their current value, so retuning a level never restates the rest of the rule.

| Command | Effect |
| --- | --- |
| `rackio alerts list` | every effective rule, with `built_in` or `configured` as its source |
| `sudo rackio alerts set <id> …` | change one rule, or define a new one |
| `sudo rackio alerts disable <id>` / `enable <id>` | switch one rule off or on without losing its level |
| `sudo rackio alerts reset [<id>]` | drop changes to one rule, or to all of them |
| `sudo rackio alerts off` / `on` | stop or resume evaluating thresholds on this machine |

The reading command needs no privilege beyond viewer-group membership; the
changing ones write the daemon's configuration and run as the daemon's owner,
the same as `rackio relay set`.

Changes apply to the running daemon immediately — no restart, and no gap in
what paired viewers see. A change that cannot be applied is rejected before
anything is written, and the reason is reported rather than saved.

Turning alerting off silences only thresholds. `stale`, `offline`,
`auth_error`, `incompatible` and `degraded` come from evidence rather than from
a level, and are still reported.

#### Metrics a rule may name

`cpu_percent`, `memory_percent`, `swap_percent`, `disk_percent` and
`temperature_headroom_celsius` (degrees remaining before the hardware's own
limit). A rule naming anything else is rejected: a metric that never resolves
would leave the machine silent in exactly the way a healthy one is. A source
the host cannot read — no swap, no published sensor limit — leaves its rule
inactive rather than reading as zero, and a rule whose metric becomes
unreadable clears instead of staying latched. A degraded collector or storage
subsystem still reports `degraded` in preference to a threshold state, because
the underlying values are no longer trustworthy.

The daemon's `config.json` is the same setting seen from the other side: an
`alerts` array of partial overrides keyed by rule `id`, plus `alerts_enabled`.
Overrides are merged over the shipped rules, so a machine that only ever
retuned one level still receives later releases' defaults for the rest.

```json
{
  "alerts_enabled": true,
  "alerts": [
    { "id": "disk-capacity-warning", "threshold": 80.0 },
    { "id": "cpu-saturation-warning", "enabled": false },
    {
      "id": "swap-warning",
      "metric": "swap_percent",
      "comparison": "greater_than_or_equal",
      "threshold": 50.0,
      "consecutive_samples": 30,
      "severity": "warning"
    }
  ]
}
```

Edit the file directly only while the daemon is stopped; a running daemon owns
it, and `rackio alerts` is the interface that keeps the file and the running
rules in step.

#### Where a breach is visible

The machine card shows the fullest filesystem; its disk tile names the mount,
and **View history** opens a per-filesystem list of every mount the machine
reported, fullest first. The tray submenu carries the same metrics as the card
— CPU, memory, swap, the fullest filesystem by name, temperature, network rate
and uptime — so the machine that raised an alert can be read without opening
the window. A reading a machine cannot report is an em dash with no bar, never
a zero.

#### Notifications in the desktop

Which state raises an OS notification, and whether notifications are sent at
all, is the viewer's own setting: **Notify at** (Warning / Degraded / Critical /
Offline) and the notifications toggle in the desktop header. It is stored per
viewer, so one operator can watch at `warning` while another is paged only at
`critical`, without changing what any machine evaluates.

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
