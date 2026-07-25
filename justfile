set shell := ["bash", "-euo", "pipefail", "-c"]

check:
    mise run check

bootstrap:
    mise run bootstrap

doctor:
    mise run doctor

doctor-relay:
    mise run doctor:relay

test-rust:
    mise exec -- cargo nextest run --workspace

coverage:
    mise exec -- cargo llvm-cov nextest --workspace --lcov --output-path test-results/coverage/lcov.info

dependencies:
    mise exec -- cargo deny check
    mise exec -- cargo machete

release-check:
    mise exec -- cargo build --release -p tray-monitor-agent
    mise exec -- scripts/check-release-binary-cloud-independence.sh

agent *args:
    mise exec -- cargo run -p tray-monitor-agent -- {{args}}

desktop:
    mise run desktop:dev
