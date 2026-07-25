# Development environment

`mise.toml` is the tool and task source of truth. It pins Rust, Node.js, pnpm,
Just, Lefthook, test, coverage, dependency-policy and security tools. Shell
profile changes are not required; use `mise run` or `mise exec` so IDE,
automation and terminal commands resolve the same versions.

## First setup

Install `mise` with a verified package for the host OS by following the
[mise getting-started guide](https://mise.jdx.dev/getting-started), then run:

```sh
mise trust mise.toml
mise run bootstrap
```

The bootstrap task installs every pinned tool, performs a frozen pnpm install,
fetches the locked Cargo graph, installs the project Playwright Chromium build,
validates Lefthook, installs the pre-commit hook and finishes with the
environment doctor. Re-running it is safe.

No `.env` file is required. Node identities and pairing secrets are generated
by the agent in its protected data directory, not placed in the repository.

## Host prerequisites

The bootstrap task does not invoke an administrator package manager.

- macOS desktop development requires Xcode Command Line Tools:
  `xcode-select --install`.
- Debian/Ubuntu desktop development requires:
  `libwebkit2gtk-4.1-dev build-essential curl wget file libxdo-dev libssl-dev
  libayatana-appindicator3-dev librsvg2-dev`.
- Windows desktop development requires Microsoft C++ Build Tools with
  `Desktop development with C++`, the MSVC Rust host and WebView2 Runtime.

These are Tauri host requirements; the owning package lists for supported
distributions are maintained in the
[Tauri prerequisites](https://v2.tauri.app/start/prerequisites/). The headless
agent does not require the desktop WebView dependencies.

## Readiness and common tasks

```sh
mise run doctor
mise run doctor:relay
mise run agent:daemon
mise run desktop:dev
mise run frontend:dev
mise run test:pairing
mise run test:installer
mise run check
```

`doctor` emits machine-readable JSON. Docker is optional and produces an
explicit `degraded` result when unavailable. `doctor:relay` promotes Docker to
a required check and fails until the self-hosted relay runtime is ready.

Environment contract tests write JUnit XML to
`test-results/environment-doctor.junit.xml`. Rust, Vitest and Playwright reports
are stored in the same ignored directory.

`test:installer` builds a synthetic Linux release archive, installs it under an
isolated temporary root and verifies checksum rejection. It does not modify the
host systemd configuration.

`test:pairing` executes the isolated two-daemon smoke below, including viewer
restart and reconnect. It writes daemon logs under `test-results/two-daemon/`
only when the smoke fails.

## Two-daemon pairing smoke

Pairing, reconnect and remote-snapshot changes need two isolated sets of
`RACKIO_CONFIG_DIR`, `RACKIO_DATA_DIR`, `RACKIO_STATE_DIR` and `RACKIO_SOCKET`
values. Each daemon must have its own identity and socket. The minimum smoke is:

1. start both daemons;
2. create a bundle on the monitored daemon;
3. import it on the viewer daemon;
4. poll `rackio fleet` until one remote metric sample is present;
5. assert the selected path is truthful (`lan_direct` for a same-host fixture);
6. assert importing the same bundle again fails;
7. stop both daemons and remove only their isolated temporary roots.

Never use the developer's normal Rackio directories for this smoke. Passing it
is not NAT or relay evidence; record those separately against
[`release-checklist.md`](release-checklist.md).

## Troubleshooting

- `mise.toml is not trusted`: inspect it, then run `mise trust mise.toml`.
- missing Git hook: run `mise run bootstrap`, then `lefthook run pre-commit`.
- missing Playwright browser: rerun `mise run bootstrap`.
- desktop dependency failure: install the host prerequisite above and rerun
  `mise run doctor`.
- relay doctor degraded or failed: start Docker or another compatible container
  runtime, then rerun `mise run doctor:relay`.

Do not bypass a failed hook. Run the failing command through `mise run` so it
uses the repository-pinned toolchain.
