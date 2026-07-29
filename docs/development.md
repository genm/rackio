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
For the user/operator lifecycle rather than source development, use
[`operations.md`](operations.md); it is also the authoritative guide for the
SSH-assisted Linux bootstrap trust boundary.

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
mise run measure:desktop-build
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

`measure:desktop-build` performs a clean desktop debug build in an isolated
temporary target directory, writes machine-readable size evidence to
`test-results/desktop-build-footprint.json`, enforces the 1.5 GiB target
directory budget, and removes its temporary build output. Normal development
and test builds retain line-number backtraces while omitting full dependency
debug data. Run `cargo build --profile debugging` when a source-level debugger
needs full symbols.

`test:installer` builds a synthetic Linux release archive, installs it under an
isolated temporary root and verifies checksum rejection. It does not modify the
host systemd configuration.

`test:pairing` executes the isolated two-daemon smoke below, including viewer
restart and reconnect. It writes daemon logs under `test-results/two-daemon/`
only when the smoke fails.

## Scheduled deep verification

Three checks cost far more than a pull request should wait for, so they run in
the scheduled `Deep verification` workflow rather than in `check`. Each has a
local equivalent for reproducing a reported failure:

```sh
mise run mutants
mise run fuzz
mise run links:check
```

`mutants` re-runs the suite once per injected behaviour change across
`rackio-core`, `rackio-protocol` and `rackio-iroh`. A **missed** mutant is a
function whose behaviour can be changed without any test noticing — the gap that
line coverage cannot show. Reports land in `test-results/mutants/`.

`fuzz` drives the two decoders reachable before any peer is authorised:
`pairing_bundle` (`PairingBundle::decode`, which parses scanned or pasted text)
and `metric_frame` (`read_frame`, whose length prefix guards allocation). It is
the only task that leaves the pinned stable toolchain, because libFuzzer needs
nightly instrumentation:

```sh
rustup toolchain install nightly --profile minimal --component rust-src
cargo +nightly install cargo-fuzz --version 0.13.2 --locked
```

The fuzz crate under `fuzz/` is deliberately outside the workspace so its
sanitizer flags and the `unsafe` code `libfuzzer-sys` generates never reach a
shipped binary. `cargo check --manifest-path fuzz/Cargo.toml` runs on every
Rust-affecting pull request so a refactor cannot silently break a target. Commit
any crashing input from `fuzz/artifacts/` into `fuzz/corpus/<target>/` together
with the regression test that covers it.

`links:check` resolves every link in the tracked Markdown. Reserved names from
RFC 6761 are excluded in `.lycheeignore` because they are unresolvable by design.

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
