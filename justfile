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
    mise run licenses:check
    mise run secrets:history

release-check:
    mise exec -- cargo build --release -p rackio-agent
    mise exec -- scripts/check-release-binary-cloud-independence.sh

benchmark-agent:
    mise run benchmark:agent

test-installer:
    packaging/linux/install.test.sh

agent *args:
    mise exec -- cargo run -p rackio-agent -- {{args}}

desktop:
    mise run desktop:dev
