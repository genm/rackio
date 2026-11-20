FROM rust:1.100.0-bookworm@sha256:ade39a710ef85f3506cffcfec3e8a24ba090badef1dff2fb798f71d878833fea AS builder
RUN cargo install iroh-relay --version 1.0.3 --locked --features server

FROM debian:bookworm-slim@sha256:734a72b3d6142a71993b4bc580ca01d8ca687f77a2c422d96a961d3376b179bc
RUN apt-get update \
    && apt-get install --yes --no-install-recommends ca-certificates curl \
    && rm -rf /var/lib/apt/lists/*
COPY --from=builder /usr/local/cargo/bin/iroh-relay /usr/local/bin/iroh-relay
USER 65532:65532
EXPOSE 80 443 7824/udp 9090
ENTRYPOINT ["/usr/local/bin/iroh-relay"]
CMD ["--config-path", "/etc/iroh-relay/config.toml"]
