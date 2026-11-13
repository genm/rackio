# Rackio Repository Guidance

## Product contract

Rackio is a lightweight, cloud-independent monitor for groups of machines.
Every machine owns its metrics and history. Trusted machines communicate over
end-to-end encrypted iroh connections, preferring direct paths and using only
explicitly configured self-hosted relays as a fallback.

The following boundaries are non-negotiable:

- Do not add a central account, control plane, metrics database, or vendor API.
- Do not enable iroh public relays, vendor DNS discovery, or telemetry defaults.
- Direct-only mode must not contact hosts other than configured peers.
- Relay mode may contact only relay URLs explicitly configured by the user.
- The P2P monitoring protocol is read-only. Product lifecycle operations such
  as install, update, repair and uninstall use explicit local or SSH authority;
  do not smuggle an arbitrary remote shell into the metrics protocol.
- Authentication, authorization, protocol-major compatibility, and pairing
  must fail closed.
- Never represent unsupported, unavailable, stale, or degraded data as zero or
  healthy.

Read `docs/architecture.md`, `docs/threat-model.md`, and
`docs/release-checklist.md` before changing transport, identity, pairing,
storage, or packaging behavior.

## Ownership boundaries

| Path | Responsibility |
| --- | --- |
| `crates/rackio-core` | Metrics, history, alerts, and transport-independent domain types |
| `crates/rackio-protocol` | Protobuf schema, framing limits, and protocol compatibility |
| `crates/rackio-iroh` | Endpoint identity, pairing, peer authorization, and iroh transport |
| `crates/rackio-windows-ipc` | Windows named-pipe DACL construction and caller-token verification; the only crate permitted to use `unsafe` |
| `apps/agent` | Daemon lifecycle, local IPC, persistence, and CLI |
| `apps/desktop` | Tauri tray application and React viewer |
| `proto/rackio.proto` | Wire-schema source of truth |
| `relay-package` | Version-pinned packaging for the upstream self-hosted relay |
| `packaging` | OS service definitions and installer requirements |

Keep iroh-specific APIs inside `rackio-iroh`. Do not make the desktop UI own
sampling, history, authorization, or remote transport. The daemon remains
operational when the tray exits or the user logs out.

Use **Rackio** for the product, `rackio` for the CLI and filesystem names,
`rackio-*` for Rust packages, and `RACKIO_*` for environment variables.
User-facing copy should say "machine" or "rack"; protocol and internal domain
types may use "node" where that is technically precise.

## Development workflow

The checked-in `mise.toml` is the tool-version and task source of truth.

```sh
mise trust mise.toml
mise run bootstrap
mise run check
```

`mise run check` must pass before handoff. Its exact steps are defined in
`mise.toml`'s `tasks.check`, not restated here — expect it to run further than
Rust and frontend checks: it also includes the two-daemon pairing and cleanup
E2E scripts and the Linux installer test, so budget time for those.

- Start behavior changes with a failing test when practical.
- Exercise at least one realistic invalid, denied, timeout, or degraded path for
  every meaningful change.
- Do not weaken lints, types, framing limits, or fail-closed behavior to pass a
  check.
- Do not hand-edit generated protobuf or Tauri schema output.
- Keep test reports under `test-results/` and UI evidence under
  `output/playwright/`; both are ignored build artifacts.
- Use RFC 6761 reserved domains such as `example.test` in tests and examples.
- Do not introduce `.env` files. Use explicit OS configuration or the selected
  managed secret system for secrets.

## OSS governance

For GitHub Issues, labels, Milestones, Projects, contribution triage, release
evidence, or public-repository setup, read and follow the project-local
`.agents/skills/rackio-oss-governance/SKILL.md`. Keep external GitHub state,
release-gate state, and repository-local preparation distinct.

## Protocol and security changes

- Preserve the 1 MiB frame allocation boundary and bounded history paging.
- A protocol-major mismatch must fail closed. Minor-version evolution must
  remain capability-driven.
- Endpoint IDs are transport identities; application authorization belongs to
  the persisted peer allowlist.
- Pairing secrets are short-lived, single-use, and redacted from logs. Five
  failed attempts lock out the *requesting peer*, identified by its
  authenticated endpoint ID, for the rest of the window. The window itself
  survives, so a reachable bystander cannot destroy the operator's pairing
  window; the failure table is bounded so minting endpoint IDs cannot grow it.
- Never log private keys, pairing secrets, metric payloads, or authorization
  material.
- Keep path state truthful: report `lan_direct`, `wan_direct`, or `relayed`
  based on the selected connection path.
- Exact iroh upgrades require the NAT and cloud-independence matrices described
  in `docs/release-checklist.md`.

## Commits

All commit messages must follow Conventional Commits:

```text
type(optional-scope): imperative summary
```

Examples:

```text
feat(agent): add bounded history query
fix(pairing): reject reused secrets
docs: document relay metadata exposure
refactor(protocol)!: replace the v1 request envelope
```

Use a `BREAKING CHANGE:` footer or `!` for breaking changes. Keep the header at
100 characters or fewer. `commitlint.config.mjs` is the policy source of truth;
Lefthook enforces it locally and CI validates every commit in the submitted
range. Do not bypass hooks.
