set shell := ["bash", "-euo", "pipefail", "-c"]

check:
    cargo fmt --all -- --check
    cargo clippy --workspace --all-targets --all-features -- -D warnings
    cargo nextest run --workspace
    pnpm format:check
    pnpm typecheck
    pnpm lint
    pnpm test
    pnpm test:ct
    pnpm build

test-rust:
    cargo nextest run --workspace

coverage:
    cargo llvm-cov nextest --workspace --lcov --output-path test-results/coverage/lcov.info

dependencies:
    cargo deny check
    cargo machete

release-check:
    cargo build --release -p tray-monitor-agent
    scripts/check-release-binary-cloud-independence.sh

agent *args:
    cargo run -p tray-monitor-agent -- {{args}}

desktop:
    pnpm --filter @tray-monitor/desktop tauri dev
