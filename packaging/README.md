# Service packaging

The agent is designed to outlive the tray and user login. The templates in this
directory define that process boundary.

## Linux system service

[`../install.sh`](../install.sh) automates the Linux headless installation. It
downloads a versioned release archive from
`https://rackio.genm.dev/releases`, verifies its SHA-256 digest before executing
the binary, creates a non-login `rackio` user and `rackio-viewers` group,
installs the binary and unit, enables the service, and fails unless both
systemd and the local daemon health check succeed.

```sh
curl --proto '=https' --tlsv1.2 -LsSf https://rackio.genm.dev/install.sh | sh
```

This is the target public interface. The source tree does not imply that the
URL or a supported release is currently live. An auditable alternative must be
published alongside it:

```sh
curl --proto '=https' --tlsv1.2 -O https://rackio.genm.dev/install.sh
less install.sh
sh install.sh
```

Log out and back in after the group change. The systemd runtime directory and
0660 socket gate desktop access; the agent also requires Unix peer credentials
to be available for every accepted local connection.

The release archive contract is:

```text
releases/
├── latest.txt
└── v<VERSION>/
    ├── rackio-v<VERSION>-<TARGET>.tar.gz
    └── rackio-v<VERSION>-<TARGET>.tar.gz.sha256
```

Each archive contains `rackio`, `rackio.service`, `uninstall.sh`, and both
license files. Build it from an already compiled binary:

```sh
packaging/linux/package-release.sh \
  target/release/rackio \
  0.1.0 \
  x86_64-unknown-linux-gnu
```

Verify the normal install and checksum-rejection paths without root or systemd:

```sh
mise run test:installer
```

Uninstall preserves identity, pairing records, configuration, history and logs
by default. `--purge` explicitly removes those directories:

```sh
sudo /usr/local/lib/rackio/uninstall.sh
sudo /usr/local/lib/rackio/uninstall.sh --purge
```

SHA-256 fetched from the same HTTPS origin protects against transfer corruption
and mismatched artifacts, but does not independently protect against compromise
of that origin. Public release publication must additionally provide signed
provenance or artifact attestation; it remains blocked by `just release-check`.

## macOS LaunchDaemon

The final package installer must create `/var/run/rackio` with an
installer-owned viewer group and mode 2770, create the data/log directories,
install the binary, and load the plist under `/Library/LaunchDaemons`.

Do not manually copy this template into production yet: the signed pkg,
dedicated local group, ownership rollback and uninstall receipts remain release
work.

## Windows

Windows Service registration is intentionally not packaged yet. The Rust agent
can collect and serve P2P traffic on Windows, but named-pipe IPC with an explicit
ACL is still a release blocker. Shipping a service before that boundary exists
would leave the tray disconnected or encourage an insecure fallback.
