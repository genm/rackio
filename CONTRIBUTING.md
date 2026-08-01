# Contributing to Rackio

Rackio is preparing for its first public release. Contributions are welcome,
but an implementation is not release-ready until the required cross-platform,
network, security, and operational evidence has been reviewed.

Review here is strict about evidence and never about the contributor. That
expectation runs both ways and is written down in
[`CODE_OF_CONDUCT.md`](CODE_OF_CONDUCT.md), which applies to every issue, pull
request and review in this repository.

## Before opening an issue

Do not disclose vulnerabilities, endpoint private keys, pairing bundles, live
metric payloads, or routable addresses in a public issue. Follow
[`SECURITY.md`](SECURITY.md) for private reporting.

Search existing issues before opening a bug report or feature proposal. Use the
provided issue forms so that the environment, observed behavior, and acceptance
criteria are explicit.

## Sources of truth

Each kind of state has one owner:

| Information | Authoritative location |
| --- | --- |
| Executable work and discussion | GitHub Issues |
| First-release gate status | [`docs/release-checklist.md`](docs/release-checklist.md) |
| Release work scope and required evidence | [`docs/backlog.md`](docs/backlog.md) |
| Release approval, authority, and artifact locations | [`docs/release-governance.md`](docs/release-governance.md) |
| Project decisions, maintainers, and succession | [`GOVERNANCE.md`](GOVERNANCE.md) |
| Bug, question, feature, and security routing | [`SUPPORT.md`](SUPPORT.md) |
| Priorities and current workflow state | GitHub Project |
| Time-bounded release target | GitHub Milestone |
| Label names and meanings | [`.github/labels.json`](.github/labels.json) |

Closing an issue does not automatically satisfy a release gate. Update the
release checklist only after the evidence linked from the issue has been
reviewed. The Project must reflect issue state rather than introducing another
set of task checkboxes.

## Issue lifecycle

1. Triage the issue with one `type:*` label, the relevant `area:*` and
   `platform:*` labels, and a priority when known.
2. Add accepted first-release work to the `0.1.0-rc1` milestone.
3. Track workflow state in the Project as `Backlog`, `Ready`, `In progress`,
   `In review`, `Blocked`, or `Done`.
4. Link pull requests with `Closes #<issue>` for complete work or
   `Refs #<issue>` for partial work.
5. Attach machine-readable results, screenshots, signatures, or other required
   evidence before closing release issues.

Use `status:blocked` only when the issue names the missing authority, hardware,
upstream change, or dependency. Use `status:needs-evidence` when implementation
exists but the release evidence is incomplete.

## Contribution terms and provenance

Unless a file states otherwise, contributions are accepted under Rackio's
outbound `MIT OR Apache-2.0` license. By submitting a contribution, you represent
that you have the right to provide it on those terms. Identify copied, adapted,
generated, or third-party material and preserve its provenance and required
notices; do not submit employer, client, or other confidential material.

AI-assisted contributions are allowed, but the human contributor remains
responsible for every submitted line and for the same authorship, licensing,
security, and test representations as any other contribution. Disclose material
AI assistance in the pull request, review the output for copied or
license-incompatible material, and never provide repository secrets or private
security reports to an unapproved tool. Unreviewed bulk-generated changes are
not accepted.

## Development

Install the pinned toolchain and dependencies:

```sh
mise trust mise.toml
mise run bootstrap
```

Run the full local quality gate:

```sh
mise run check
```

Development setup, platform prerequisites, and targeted commands are documented
in [`docs/development.md`](docs/development.md).

## Pull requests

- Keep one coherent behavior or operational outcome per pull request.
- Add or update tests for behavior changes. Exercise a realistic failure,
  invalid-input, permission-denied, or degraded path as well as the happy path.
- Preserve the fail-closed identity, authorization, protocol, and installer
  boundaries.
- Include screenshots for visible desktop changes.
- Update owning documentation when a contract, command, or runbook changes.
- Use English Conventional Commit messages without generated-by trailers.

Repository decision and escalation authority is defined in
[`GOVERNANCE.md`](GOVERNANCE.md). Current support capacity and reporting routes
are defined in [`SUPPORT.md`](SUPPORT.md); neither an accepted contribution nor
an evaluation pre-release creates a support commitment.

The default branch accepts changes only after required CI, Security and CodeQL
checks pass. A green local run is useful evidence but does not replace platform
or network checks that require another environment.

Dependabot security patch/minor updates and routine patch updates may enable
GitHub's native auto-merge after their metadata and unmodified bot ownership are
verified. Auto-merge does not bypass the protected branch: the branch must be
current, every required check must pass, and review conversations must be
resolved. Major updates, routine minor updates, maintainer-modified branches,
and unrecognized metadata remain open for manual review. The executable policy
is [`scripts/dependabot-automerge-policy.mjs`](scripts/dependabot-automerge-policy.mjs).

Mutation testing, fuzzing and link checking run on a schedule instead, in the
`Deep verification` workflow. They do not gate a merge, but a failure there is a
real gap in the evidence this repository claims to have — see
[`docs/development.md`](docs/development.md) for how to reproduce one locally.
