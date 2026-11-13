# Service packaging

The agent is designed to outlive the tray and user login. The templates in this
directory define that process boundary.

Installation and removal are package-owned operations. Rackio intentionally
does not expose CLI commands that would register only a binary while omitting
the platform user/group, IPC permissions, persistence directories, recovery
policy, or signed service definition. Use the installer for the target OS below
as the single authoritative path.

The operator workflow and its security boundaries are maintained in
[`../docs/operations.md`](../docs/operations.md). This document owns artifact
layout, package-building and platform-installer details; it does not imply that
a public release is available.

## Linux system service

[`../install.sh`](../install.sh) automates the Linux headless installation. It
downloads a versioned release archive from the release root
(`https://rackio.genm.dev/releases` by default, `--releases-url` for any other,
including a GitHub Release), verifies its SHA-256 digest before executing
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
<release root>/
├── latest.txt                                   (supported releases only)
└── v<VERSION>/
    ├── rackio-v<VERSION>-<TARGET>.tar.gz
    └── rackio-v<VERSION>-<TARGET>.tar.gz.sha256
```

A GitHub Release satisfies that contract without the custom domain: its download
root is `https://github.com/genm/rackio/releases/download`, under which the tag
directory `v<VERSION>` holds the same asset names. Install a specific version
directly from it:

```sh
sh install.sh --version <VERSION> \
  --releases-url https://github.com/genm/rackio/releases/download
```

`latest` resolution reads a version pointer, by default `<release root>/latest.txt`.
GitHub serves a moving pointer outside the versioned root, so a release root that
publishes one there needs `--latest-url`
(`https://github.com/genm/rackio/releases/latest/download/latest.txt`). An
evaluation pre-release publishes no pointer at all and is reachable only through
an explicit `--version`; the installer fails closed and says so.

Each archive contains `rackio`, `rackio.service`, `uninstall.sh`, the project
licenses, and the generated `THIRDPARTY.html` Rust dependency-license bundle. CI
regenerates that bundle from `Cargo.lock` and rejects drift. Build the archive
from an already compiled binary, naming the version the binary actually reports
(`cargo metadata --locked --no-deps` for `rackio-agent`), because a release tag
must match it exactly:

```sh
packaging/linux/package-release.sh \
  "${CARGO_TARGET_DIR:-target}/release/rackio" \
  <VERSION> \
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
  --archive rackio-v<VERSION>-x86_64-unknown-linux-gnu.tar.gz \
  --checksum rackio-v<VERSION>-x86_64-unknown-linux-gnu.tar.gz.sha256
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

[`Linux release artifacts`](../.github/workflows/linux-release-artifacts.yml) is
the single authoritative build. It compiles the headless agent natively on
GitHub-hosted x86_64 and arm64 Linux runners, rejects vendor relay/discovery
defaults, verifies the archive checksum and contents, records GitHub
build-provenance attestations, then installs that same archive on the clean
runner. The system test verifies systemd enablement and restart, root and
viewer-group health, denial outside the viewer group, preserving reinstall, and
explicit purge on both architectures.

Two workflows call it, so a published asset is produced by exactly the steps the
evidence run verifies:

- [`Linux release evidence`](../.github/workflows/linux-release-evidence.yml) is
  dispatched manually, accepts only the protected default branch, and keeps its
  one-day workflow artifacts. They are test evidence, not reboot evidence, a
  supported release, or a publication.
- [`Release`](../.github/workflows/release.yml) runs on a `v*` tag. It requires
  the tag's commit to be contained in protected `main`, the tag to name the built
  package version, that version to carry a pre-release suffix, and CI plus
  Security to have a successful push run for the same commit. It then re-verifies
  every checksum and creates the immutable GitHub pre-release with the archives,
  their checksums and the reviewed `install.sh`. Publishing a supported version
  is refused by design; see
  [`release-governance.md`](../docs/release-governance.md).

Download an evidence archive from its workflow run and independently verify
the GitHub attestation:

```sh
gh attestation verify \
  rackio-v<VERSION>-<TARGET>.tar.gz \
  --repo genm/rackio
```

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
  "${CARGO_TARGET_DIR:-target}/aarch64-apple-darwin/release/rackio" \
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
  -Checksum .\rackio-v0.1.0-x86_64-pc-windows-msvc.zip.sha256 `
  -ExpectedThumbprint "<the release signing certificate's SHA-1 thumbprint>"
```

`-ExpectedThumbprint` (or the `RACKIO_EXPECTED_SIGNING_THUMBPRINT` environment
variable, or a checked-in `packaging/windows/expected-thumbprint.txt`) is
required. The project's release signing certificate identity does not exist
yet, so there is no built-in default; the installer refuses to run without an
explicit expected thumbprint rather than accepting any binary that merely
chains to a trusted root.

It verifies SHA-256 before extraction, creates the `Rackio Viewers` local
group, refuses to reuse a pre-existing `C:\ProgramData\Rackio` unless it is
already owned by SYSTEM or Administrators, installs an automatic LocalSystem
service with recovery actions, sets the installation and data directory owner
to SYSTEM and applies explicit filesystem ACLs, starts the service and
requires a successful secured named-pipe health check. Sign out and back in
after the first install so the non-elevated tray token contains its new
viewer-group membership.

Named-pipe access is enforced twice: its DACL grants only LocalSystem,
administrators and `Rackio Viewers`, and the server is configured to reject
remote clients and then checks the connected process token after a read. The
CI named-pipe check exercises the token check and the client-side pipe-name
prefix validation on a same-host runner; it does not connect from a remote
host, so `reject_remote_clients` itself is not exercised by that evidence and
still needs verification from a clean Windows host (see
[`release-checklist.md`](../docs/release-checklist.md)). Build the archive on
Windows with a code-signing certificate installed in the certificate store:

```powershell
$env:RACKIO_SIGNTOOL_CERT_SHA1 = "CERTIFICATE_THUMBPRINT"
.\packaging\windows\package-release.ps1 `
  -Binary .\target\release\rackio.exe `
  -Version 0.1.0 `
  -Target x86_64-pc-windows-msvc
```

The builder fails closed without the signing certificate, timestamps
`rackio.exe`, and verifies Authenticode before archiving it. The installer
independently verifies that the signature is `Valid` and that its signer
certificate thumbprint matches the expected thumbprint supplied to
`install.ps1`, refusing to install otherwise.
