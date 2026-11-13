<!--
Keep one coherent behavior or operational outcome per pull request.
Full conventions: CONTRIBUTING.md
-->

## What changes

<!-- The behavior or operational outcome, not a list of edited files. -->

## Why

<!-- The problem this solves. Link the issue: `Closes #123`, or `Refs #123`
     for partial work. -->

## Evidence

<!-- What you ran and what it showed. A green CI run is not evidence on its own
     for platform or network behavior that CI cannot reach. -->

- [ ] `mise run check` passes locally
- [ ] Tests cover a realistic failure, invalid-input, permission-denied or
      degraded path, not only the happy path
- [ ] Screenshots attached for visible desktop changes

## Boundaries

- [ ] The fail-closed identity, authorization, protocol and installer
      boundaries are preserved
- [ ] No new user-visible limit, quota, timeout or narrowed input format
      without a stated authority for it
- [ ] Owning documentation updated if a contract, command or runbook changed
- [ ] Release evidence in `docs/release-checklist.md` updated, or not affected
