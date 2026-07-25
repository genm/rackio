# Service packaging

The agent is designed to outlive the tray and user login. The templates in this
directory define that process boundary.

Installation and removal are package-owned operations. Rackio intentionally
does not expose CLI commands that would register only a binary while omitting
the platform user/group, IPC permissions, persistence directories, recovery
policy, or signed service definition. Use the installer for the target OS below
as the single authoritative path.

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

Each archive contains `rackio`, `rackio.service`, `uninstall.sh`, the project
licenses, and the generated `THIRDPARTY.html` Rust dependency-license bundle. CI
regenerates that bundle from `Cargo.lock` and rejects drift. Build the archive
from an already compiled binary:

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

The desktop SSH bootstrap does not ask the server to download from the release
host. It confirms the server host key, uploads the archive and checksum from the
client, then invokes the same installer implementation:

```sh
sudo sh install.sh \
  --archive rackio-v0.1.0-x86_64-unknown-linux-gnu.tar.gz \
  --checksum rackio-v0.1.0-x86_64-unknown-linux-gnu.tar.gz.sha256
```

SSH bootstrap requires key or agent authentication and non-interactive root
access (`root` login or passwordless `sudo`) so the desktop never captures or
stores an SSH password. A host-key change after confirmation fails closed.

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

The pkg scripts create a dedicated `_rackio` daemon account and
`_rackio-viewers` group, create the data/log/runtime directories, install the
binary and load the plist under `/Library/LaunchDaemons`.

Release package creation fails unless `RACKIO_INSTALLER_IDENTITY` names a
Developer ID Installer certificate. When `RACKIO_NOTARY_PROFILE` is set, the
builder waits for Apple notarization, staples the ticket and validates it:

```sh
RACKIO_APPLICATION_IDENTITY="Developer ID Application: Example (TEAMID)" \
RACKIO_INSTALLER_IDENTITY="Developer ID Installer: Example (TEAMID)" \
RACKIO_NOTARY_PROFILE=rackio-notary \
  packaging/macos/package-release.sh \
  target/aarch64-apple-darwin/release/rackio \
  0.1.0 \
  aarch64-apple-darwin
```

The uninstaller preserves identity, configuration and history unless
`--purge` is explicit:

```sh
sudo /usr/local/lib/rackio/uninstall.sh
sudo /usr/local/lib/rackio/uninstall.sh --purge
```

## Windows Service

The Windows archive contains `rackio.exe` and the preserving uninstaller.
Run the installer from an elevated PowerShell:

```powershell
.\packaging\windows\install.ps1 `
  -Archive .\rackio-v0.1.0-x86_64-pc-windows-msvc.zip `
  -Checksum .\rackio-v0.1.0-x86_64-pc-windows-msvc.zip.sha256
```

It verifies SHA-256 before extraction, creates the `Rackio Viewers` local
group, installs an automatic LocalSystem service with recovery actions, applies
explicit filesystem ACLs, starts the service and requires a successful secured
named-pipe health check. Sign out and back in after the first install so the
non-elevated tray token contains its new viewer-group membership.

Named-pipe access is enforced twice: its DACL grants only LocalSystem,
administrators and `Rackio Viewers`, and the server rejects remote clients then
checks the connected process token. Build the archive on Windows with a
code-signing certificate installed in the certificate store:

```powershell
$env:RACKIO_SIGNTOOL_CERT_SHA1 = "CERTIFICATE_THUMBPRINT"
.\packaging\windows\package-release.ps1 `
  -Binary .\target\release\rackio.exe `
  -Version 0.1.0 `
  -Target x86_64-pc-windows-msvc
```

The builder fails closed without the signing certificate, timestamps
`rackio.exe`, and verifies Authenticode before archiving it. The installer
checks that signature again before copying or registering the service.
