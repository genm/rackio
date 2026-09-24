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

The desktop update channel's signing and release requirements are documented
in [`desktop-updates.md`](desktop-updates.md).

## Minimum supported Rust version

`Cargo.toml`'s `workspace.package.rust-version` is the single stated owner of
the MSRV, and it is pinned to the exact version in `rust-toolchain.toml`. This
repository does not verify a lower MSRV: every CI job builds with the pinned
toolchain, and `cargo clippy --workspace --all-targets --all-features`
routinely relies on newly stabilised APIs, so a floor below the pinned
toolchain would be an unverified claim rather than a supported target. Bump
`rust-version` and `rust-toolchain.toml` together; do not change one without
the other (#116).

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
mise run cache:report
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

`test:pairing` executes the isolated two-daemon smokes below, including viewer
restart and reconnect, and the monitored-machine address-change recovery and
failure paths. It writes daemon logs under `test-results/two-daemon/` and
`test-results/two-daemon-address-change/` only when a smoke fails.

## Build directory

`mise.toml` sets `CARGO_TARGET_DIR` to `$XDG_CACHE_HOME/rackio/target`
(`~/.cache/rackio/target` by default), so build output does not live under the
checkout. A full `mise run check` leaves roughly 5 GB behind, and without this
every additional Git worktree pays that cost again for the same dependency
graph. The nested `fuzz/` workspace shares the directory for the same reason.

It also sets Cargo's `build.build-dir` (as `CARGO_BUILD_BUILD_DIR`) to the
sibling `$XDG_CACHE_HOME/rackio/build`:

```text
~/.cache/rackio/
├── target/   # final artifacts: binaries, rustdoc, nextest reports
└── build/    # intermediates: deps/, incremental/, build/, .fingerprint/
```

Two consequences are worth knowing:

- Cargo takes an exclusive lock on the directory, so a build started from a
  second worktree waits for the first to finish rather than running beside it.
- `cargo clean` still works, but it now clears the directories every checkout
  shares, not just the current one.

Exporting `CARGO_TARGET_DIR` yourself overrides this, which is how
`measure:desktop-build` isolates its own build. When only `CARGO_TARGET_DIR`
is exported, the build directory follows it, which is Cargo's own default, so
an isolated build does not write intermediates into the shared cache. Export
`CARGO_BUILD_BUILD_DIR` as well to place intermediates elsewhere. Scripts that
need a built binary resolve the directory from `cargo metadata` or the same
variable instead of assuming `target/`, so an override stays consistent
across the repository. Builds run outside `mise` fall back to `target/` inside
the checkout for both; `mise` shell activation or `mise exec --` keeps them
together. CI does not load the `mise` environment and keeps Cargo's default
single `target/` directory.

### Cache contract

Everything under both directories is reproducible Cargo output built from the
pinned toolchain and `Cargo.lock`. It is not Rackio application data: machine
identities, the peer allowlist, pairing state, metrics history and
configuration live in the agent's configuration, data and state directories
(the OS application directories for `dev.rackio.rackio`, the `RACKIO_*_DIR`
overrides, or `/etc/rackio` and `/var/lib/rackio` for the Linux service; see
[`operations.md`](operations.md)). The agent never writes under
`~/.cache/rackio`, so the build cache never contains them. Deleting the
cache costs a recompile, not data. The size of this local compiler cache is a
separate concern from the size of the shipped binary, `.app`, installer or
release archive.

Prefer the tasks below to deleting cache paths by hand. They resolve the
directories from `cargo metadata`, and cleanup only ever runs `cargo clean`,
so they do not depend on Cargo's internal layout, which is not stable.

```sh
mise run cache:report                   # read-only size breakdown
mise run cache:report -- --json         # the same, machine-readable
mise run cache:clean -- --dry-run       # preview what the default clean removes
mise run cache:clean                    # remove the dev, test and debugging profiles
mise run cache:clean -- --all           # remove everything, including release and cross targets
```

`cache:report` prints the size of each directory and, per profile, final
artifacts separately from `deps/`, `incremental/`, build-script `build/` and
fingerprints. Hard links between a final binary and its `deps/` copy are
counted once, against the final artifact. The report never modifies anything.

`cache:clean` without `--all` runs `cargo clean --profile dev` and
`cargo clean --profile debugging`, which leaves release builds, rustdoc output
and cross-compilation targets such as the one `check:windows-cross` produces
in place. `--all` runs a plain `cargo clean`. Both print the directories they
affect first and refuse to run when a directory is not tagged by Cargo
(`CACHEDIR.TAG`), is a filesystem root, or contains the checkout or the home
directory.

Stop any Rackio agent or desktop app that was started from a development build
(`mise run agent:daemon`, `mise run desktop:dev`, a binary under
`~/.cache/rackio/target`) before cleaning. `cache:clean` never stops them: on
Linux and macOS it lists processes whose executable lives in the cache and
refuses to clean while any do, and on Windows, where it cannot inspect them,
it says so and relies on you. An installed Rackio service runs from its
installation directory and is unaffected.

After switching an existing cache to this layout, run
`mise run cache:clean -- --all` once: intermediates from the old single
directory are not reused by the new one and otherwise linger in `target/`.

### `build-dir` compatibility

Verified with Cargo 1.97.1 on Linux (the pinned toolchain): workspace builds,
`cargo nextest`, `cargo doc`, release builds, the two-daemon E2E scripts and
the Linux installer test all run unchanged, because they locate artifacts
through `cargo metadata`'s `target_directory` or `CARGO_TARGET_DIR`, where
Cargo still places final artifacts. `cargo-llvm-cov` honours
`CARGO_BUILD_BUILD_DIR` itself. `cargo clean` covers both directories.

One known interaction: `tauri-build` finds the directory beside the app binary
by walking three levels up from its `OUT_DIR`, so with a separate build
directory it copies `bundle.resources` (the licence files) into
`build/<profile>/` instead of `target/<profile>/`. The desktop app does not read
those files at runtime, and `tauri build` bundles them from their source
paths, so neither `tauri dev` nor packaging is affected. A future change that
reads bundled resources at runtime in development builds must account for
this. The desktop build and macOS and Windows hosts were not exercised by the
measurements below; if the layout causes trouble there, export
`CARGO_BUILD_BUILD_DIR` equal to `CARGO_TARGET_DIR` to restore the single
directory.

### Measurements and incremental compilation

Recorded on 2026-09-24 on a 4-vCPU Linux x86_64 container with the pinned
Cargo 1.97.1. The container has no WebKitGTK, so `rackio-desktop` is excluded;
the ~9.7 GiB cache reported in #211 came from a macOS host that also builds
the desktop app and its `objc2` dependency graph, and so is larger than this
reproduction. The workload, repeated for each layout from an empty cache:

1. `cargo build --locked --workspace --exclude rackio-desktop --all-targets`
2. the same after touching `crates/rackio-core/src/lib.rs`
3. the same after touching `apps/agent/src/main.rs`
4. `cargo build --locked --release -p rackio-agent`
5. `cargo doc --workspace --exclude rackio-desktop --no-deps`

| Layout | Total | `debug` (final / deps / incremental / build) | `release` | Build 1 / 2 / 3 / 4 |
| --- | ---: | --- | ---: | --- |
| Single `target/` (before) | 2.3 GiB | 1.6 GiB (88 MiB / 1019 MiB / 445 MiB / 49 MiB) | 748 MiB | 63 s / 3 s / 2 s / 295 s |
| `target/` + `build/` (after) | 2.3 GiB: 113 MiB in `target/`, 2.2 GiB in `build/` | 88 MiB final in `target/`, 1.5 GiB intermediates in `build/` | 748 MiB | 68 s / 3 s / 2 s / 295 s |
| `target/` + `build/`, `CARGO_INCREMENTAL=0` | 1.7 GiB | 1.0 GiB, no `incremental/` | same | 63 s / 7 s / 5 s / — |

`cache:report` totals matched `du -sB1` to the byte.
`mise run cache:clean` reduced 2.3 GiB to 758 MiB in both layouts (release
profile and rustdoc kept), and `cache:clean -- --all` to nothing. After the
default clean, step 1 rebuilt in 65 s from the pinned toolchain and
`Cargo.lock`, `cargo test --workspace --exclude rackio-desktop` passed, the
two-daemon cleanup smoke and the Linux installer test passed, and an agent
daemon started from the rebuilt binary. Cleaning while that daemon ran was
refused with its PID and left it running.

The separate build directory does not change total size or build time; it
makes the split between final artifacts and disposable intermediates visible
and lets a report attribute it without guessing.

Incremental compilation stays enabled for development builds (Cargo's
default). Disabling it saved about 0.6 GiB of the `debug` profile but made the
edit-rebuild loop in steps 2 and 3 roughly 2.3x slower, which is the path a
developer repeats most. `mise run check` and `mise run mutants` already set
`CARGO_INCREMENTAL=0`, so the quality gate adds no incremental data of its
own. Use `cache:clean` to reclaim `incremental/` rather than disabling it.

## Refreshing dependency PR license notices

For an open dependency PR in this repository, maintainers can regenerate both
bundled notice files without changing the license drift checks:

```sh
gh workflow run refresh-license-notices.yml --ref main -f pr=PR_NUMBER
```

The workflow copies only changed, existing dependency manifests and lockfiles
into the trusted default-branch checkout and runs its generators in a read-only
job. Rebase the PR onto the current default branch first. PR scripts, toolchains,
and package-manager configuration are never copied or executed; PRs changing
those files or the pinned package manager require a manual refresh. A separate
trusted publisher can update only `THIRDPARTY.html` and
`THIRDPARTY-JAVASCRIPT.html`. It rejects forks, closed PRs, non-default bases,
and a head that moved during generation; it never force-pushes or merges. An
unchanged result creates no commit.

A changed PR is left in Draft because commits made by `GITHUB_TOKEN` do not
trigger PR CI. Review the notice diff and run `gh pr ready PR_NUMBER` to start
the normal required checks. The existing Security comparisons still reject
drift. Authentication comes from GitHub Actions' built-in job token; no added
secret or local credential file is needed. Failed generation publishes nothing.

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
values. Each daemon must have its own identity and socket. On Linux, when
`RACKIO_SOCKET` is unset, the CLI checks the system service socket, then the
user systemd socket under `$XDG_RUNTIME_DIR/rackio/agent.sock`, and finally the
developer state socket. The minimum smoke is:

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
