# Governance

Rackio is currently maintained by one person. This document records the actual
decision and escalation model without implying a committee, service level, or
succession capacity that does not exist yet.

## Authority and decisions

The GitHub account [`genm`](https://github.com/genm) is the project maintainer
and final decision-maker for source changes, contributor moderation, repository
access, and project direction. Release authority is narrower and is defined in
[`docs/release-governance.md`](docs/release-governance.md).

Routine changes are decided through reviewable GitHub Issues and pull requests.
Larger proposals should first describe the user outcome, alternatives, security
and operational boundaries, and acceptance evidence in an Issue. The maintainer
may reject a change that conflicts with Rackio's cloud-independent, fail-closed
product contract even when the implementation is technically sound.

Release-gate completion remains owned by
[`docs/release-checklist.md`](docs/release-checklist.md); closing an
implementation Issue is not release approval.

## Conflicts and conduct

The maintainer discloses a material personal or financial conflict when it
could affect a project decision. When practical, a conflicted or
security-sensitive decision should seek review from an independent person with
relevant expertise. Rackio cannot currently guarantee an independent appeal;
conduct reporting and the available external escalation route are defined in
[`CODE_OF_CONDUCT.md`](CODE_OF_CONDUCT.md).

Private vulnerability details never move into public governance discussion.
They follow [`SECURITY.md`](SECURITY.md).

## Maintainer changes

Maintainer access is granted explicitly after sustained, reviewable
contributions demonstrate sound judgment across Rackio's product and security
boundaries. Repository, release, registry, signing, domain, and infrastructure
authority are separate grants; one does not imply the others.

Before adding, removing, or replacing a maintainer, the project records the
decision and reviews repository administration, recovery access, vulnerability
reporting, release credentials, package namespaces, signing identities, and
domain ownership. Departing access is revoked or transferred deliberately.

## Inactivity, transfer, and archival

If Rackio can no longer be maintained safely, the maintainer should stop new
supported releases, state the last supported version and known limitations, and
either transfer the project through a recorded decision or archive it. Archival
includes disabling release and scheduled automation that could still publish,
incur cost, or promise unattended security handling; existing source, licenses,
notices, releases, and provenance remain available where practical.

This governance model is reviewed when a maintainer changes, a new distribution
channel or hosted surface is added, the license or ownership changes, or the
first supported release is proposed.
