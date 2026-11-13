FROM rust:1.99.1-bookworm@sha256:e6f2cff35d0f309acf8433b4f05878ed77092602c5065cc81801177da0bb3a9c AS builder
RUN cargo install iroh-relay --version 1.0.3 --locked --features server

FROM debian:bookworm-slim@sha256:7c7b2c966bc9ee8cedfeef67e0e279108992c77681fa595db4a9d65c06ccc587
RUN apt-get update \
    && apt-get install --yes --no-install-recommends ca-certificates curl \
    && rm -rf /var/lib/apt/lists/*
COPY --from=builder /usr/local/cargo/bin/iroh-relay /usr/local/bin/iroh-relay
USER 65532:65532
EXPOSE 80 443 7824/udp 9090
ENTRYPOINT ["/usr/local/bin/iroh-relay"]
CMD ["--config-path", "/etc/iroh-relay/config.toml"]
