set shell := ["bash", "-euo", "pipefail", "-c"]

default:
    just --list

check:
    mise run check

check-windows-cross:
    mise run check:windows-cross

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

mutants:
    mise run mutants

fuzz:
    mise run fuzz

links:
    mise run links:check

dependencies:
    mise exec -- cargo deny check
    mise exec -- cargo deny --manifest-path fuzz/Cargo.toml check
    mise exec -- cargo machete
    mise run licenses:check
    mise run secrets:history

release-check:
    mise exec -- cargo build --release -p rackio-agent
    mise exec -- scripts/check-release-binary-cloud-independence.sh

benchmark-agent:
    mise run benchmark:agent

test-installer:
    mise run test:installer

test-pairing:
    mise run test:pairing

agent *args:
    mise exec -- cargo run -p rackio-agent -- {{args}}

desktop:
    mise run desktop:dev

frontend:
    mise run frontend:dev

measure-desktop-build:
    mise run measure:desktop-build
