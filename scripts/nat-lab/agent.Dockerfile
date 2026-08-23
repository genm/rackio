# syntax=docker/dockerfile:1.7
#
# Build `rackio-agent` for linux/arm64 from this repository's source. Nothing is
# vendored: the binary in the lab image is compiled from the checked-out tree so
# a NAT report always describes the code it was produced from.

FROM rust:1.97.1-bookworm@sha256:0e2bcaef56d041a486784e54104a81aebe0da44bd03019bd70bc0401e42e4a97 AS builder

WORKDIR /src
ENV CARGO_TERM_COLOR=never CARGO_INCREMENTAL=0

# Dependency layer. Only manifests, the lock file and the protobuf inputs are
# copied, with stub crate roots standing in for the real sources, so editing
# agent code does not recompile iroh, rusqlite, tokio and the rest of the graph.
# `rackio-desktop` is a workspace member and must exist for resolution, but it
# is never built here: the lab needs the headless daemon, not Tauri or GTK.
COPY Cargo.toml Cargo.lock rust-toolchain.toml ./
COPY crates/rackio-core/Cargo.toml crates/rackio-core/Cargo.toml
COPY crates/rackio-protocol/Cargo.toml crates/rackio-protocol/Cargo.toml
COPY crates/rackio-protocol/build.rs crates/rackio-protocol/build.rs
COPY crates/rackio-iroh/Cargo.toml crates/rackio-iroh/Cargo.toml
COPY crates/rackio-windows-ipc/Cargo.toml crates/rackio-windows-ipc/Cargo.toml
COPY apps/agent/Cargo.toml apps/agent/Cargo.toml
COPY apps/desktop/src-tauri/Cargo.toml apps/desktop/src-tauri/Cargo.toml
COPY proto proto
RUN set -eu; \
    for crate in crates/rackio-core crates/rackio-protocol crates/rackio-iroh \
                 crates/rackio-windows-ipc apps/desktop/src-tauri; do \
      mkdir -p "$crate/src"; \
      : >"$crate/src/lib.rs"; \
    done; \
    mkdir -p apps/agent/src apps/desktop/src-tauri/src; \
    echo 'fn main() {}' >apps/agent/src/main.rs; \
    echo 'fn main() {}' >apps/desktop/src-tauri/src/main.rs
RUN cargo build --locked -p rackio-agent

# Real sources. `touch` defeats cargo's mtime fingerprint so the workspace
# crates rebuild while the third-party graph above stays cached.
COPY crates crates
COPY apps/agent apps/agent
RUN find crates apps/agent -name '*.rs' -exec touch {} + \
    && cargo build --locked -p rackio-agent \
    && install -m 0755 target/debug/rackio /usr/local/bin/rackio

FROM debian:bookworm-slim@sha256:abd67ffcfa541b485a3dff59865ab629aa048a6c613e639d36e7456b0b229241

# iproute2 points the LAN machines at their NAT router, tcpdump captures the
# evidence, iputils-ping measures link loss and procps stops the daemon by name.
RUN apt-get update \
    && apt-get install --yes --no-install-recommends \
        iproute2 iputils-ping procps tcpdump \
    && rm -rf /var/lib/apt/lists/*

COPY --from=builder /usr/local/bin/rackio /usr/local/bin/rackio
COPY scripts/nat-lab/container/agent-entrypoint.sh /usr/local/bin/agent-entrypoint.sh

ENV RACKIO_CONFIG_DIR=/var/lib/rackio/config \
    RACKIO_DATA_DIR=/var/lib/rackio/data \
    RACKIO_STATE_DIR=/var/lib/rackio/state \
    RACKIO_LOG_DIR=/var/lib/rackio/log \
    RACKIO_SOCKET=/var/lib/rackio/state/agent.sock

ENTRYPOINT ["/usr/local/bin/agent-entrypoint.sh"]
