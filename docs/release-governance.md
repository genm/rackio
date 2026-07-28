# Release governance

This document owns Rackio's release approval, publication authority, and
artifact-location policy. Release eligibility remains owned by
[`release-checklist.md`](release-checklist.md); package layouts and signing
commands remain owned by [`../packaging/README.md`](../packaging/README.md).

Rackio has no supported public production release yet. This policy defines how
one may be published after the checklist evidence is complete; it does not make
the planned distribution endpoint or any artifact available.

## Authority

The release maintainer is the administrator of the public
[`genm/rackio`](https://github.com/genm/rackio) repository. Until another
maintainer is explicitly recorded, the GitHub account `genm` owns release
approval, tag creation, GitHub Release publication, and the
`rackio.genm.dev` distribution configuration.

A release approval must be recorded in its GitHub release-evidence issue and
identify:

- the exact protected-`main` commit and version tag;
- the reviewed release-checklist gates and any unresolved blocker;
- every artifact checksum, signature, attestation, and SBOM;
- the successful CI and Security runs for that commit;
- the person acting as release maintainer and the approval time.

Repository write access alone is not release approval. Automation may assemble
or upload artifacts, but it must not decide that incomplete evidence is
acceptable.

## Canonical artifacts

The immutable GitHub Release for tag `v<VERSION>` is the canonical source for
Rackio binaries, installers, checksums, signatures, attestations, SBOMs, and
license notices. Published assets are never replaced in place. A correction
requires a new version and tag.

A GitHub Release is self-sufficient. Its asset URLs
(`https://github.com/genm/rackio/releases/download/v<VERSION>/<ASSET>`) are the
versioned release root that [`../install.sh`](../install.sh) accepts through
`--releases-url`, so distribution does not depend on the custom domain existing.

`https://rackio.genm.dev` is the planned stable operator-facing entry point:

- `/install.sh` serves the reviewed installer;
- `/releases/latest.txt` names an immutable published version;
- `/releases/v<VERSION>/...` serves or redirects to the exact canonical
  GitHub Release asset.

The custom domain must not contain independently built variants. Mirrored or
cached files must match the canonical asset digest. GitHub Release URLs remain
the recovery path when the custom domain is unavailable.

## Evaluation pre-releases

An evaluation pre-release exists so that operators can install and pair a real
build while the supported-release evidence is still open. It is a distinct
release class, not an early supported release.

A pre-release uses a version with a pre-release suffix (`0.1.0-rc.1`) and its
matching tag. The [`Release`](../.github/workflows/release.yml) workflow
publishes it automatically and fails closed unless:

- the pushed ref is a tag whose commit is contained in protected `main`;
- the tag names exactly the built `rackio-agent` package version;
- that version carries a pre-release suffix;
- CI and Security both have a successful push run for the same commit.

A pre-release is marked as a GitHub pre-release, publishes no `latest.txt`, and
is therefore reachable only by an explicit `--version`. Contents are limited to
what the workflow actually builds and verifies: the headless `x86_64` and
`aarch64` Linux agent archives, their checksums, their build-provenance
attestations, and the reviewed installer. The desktop application, macOS package
and Windows service installer are not published as pre-releases.

A pre-release must not be described as supported, must not receive a
`latest.txt` pointer, and must not be mirrored to a custom-domain path that
implies it is the current version.

## Publication gate

Publication is fail-closed. The release workflow refuses a version without a
pre-release suffix precisely because it cannot verify the conditions below;
automation may assemble and upload artifacts, but a supported release is created
only through the recorded approval in this document. Do not create a supported
release or update `latest.txt` when any of these conditions applies:

- the release checklist still has a blocking gate;
- the commit is not on protected `main`, or required checks are not successful;
- a required platform signature, notarization, attestation, SBOM, checksum, or
  license notice is absent or invalid;
- signing, timestamping, notarization, DNS, or hosting authority is unavailable;
- the custom-domain asset differs from the canonical GitHub Release digest;
- the release-evidence issue lacks an explicit approval.

Secrets used for signing, notarization, DNS, or hosting are operational
credentials. They must not be committed to the repository, embedded in build
artifacts, or exposed in public issue evidence.

## Publication sequence

1. Complete and review the required release-evidence issues.
2. Record a go/no-go decision against the exact protected-`main` commit.
3. Build each artifact from that commit and verify its platform signature.
4. Create the immutable version tag and draft GitHub Release.
5. Attach artifacts, checksums, attestations, SBOMs, and license notices.
6. Verify the uploaded assets before publishing the GitHub Release.
7. Publish the same reviewed installer and canonical asset mapping at
   `rackio.genm.dev`.
8. Verify every custom-domain digest, then update `latest.txt` last.

If a post-publication problem affects integrity, authorization, confidentiality,
or install safety, stop advertising the affected version and publish a new
version only after the release decision has been repeated. Do not silently
replace an existing asset.
