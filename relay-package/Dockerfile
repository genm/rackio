FROM rust:1.99.0-bookworm@sha256:114c7a4425406451c2866b6aafe69fe29b1b298832db1277d411ac73c82d04d6 AS builder
RUN cargo install iroh-relay --version 1.0.3 --locked --features server

FROM debian:bookworm-slim@sha256:7b140f374b289a7c2befc338f42ebe6441b7ea838a042bbd5acbfca6ec875818
RUN apt-get update \
    && apt-get install --yes --no-install-recommends ca-certificates curl \
    && rm -rf /var/lib/apt/lists/*
COPY --from=builder /usr/local/cargo/bin/iroh-relay /usr/local/bin/iroh-relay
USER 65532:65532
EXPOSE 80 443 7824/udp 9090
ENTRYPOINT ["/usr/local/bin/iroh-relay"]
CMD ["--config-path", "/etc/iroh-relay/config.toml"]
