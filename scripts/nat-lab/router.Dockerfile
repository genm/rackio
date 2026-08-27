# syntax=docker/dockerfile:1.7
#
# One NAT router image for every router in the lab. Its behaviour is chosen at
# run time by `NAT_MODE` and friends rather than by forking the image, because
# the symmetric-NAT scenarios land on the same container later.

FROM debian:bookworm-slim@sha256:abd67ffcfa541b485a3dff59865ab629aa048a6c613e639d36e7456b0b229241

RUN apt-get update \
    && apt-get install --yes --no-install-recommends \
        iproute2 iptables iputils-ping procps tcpdump \
    && rm -rf /var/lib/apt/lists/*

COPY scripts/nat-lab/container/router-entrypoint.sh /usr/local/bin/router-entrypoint.sh

ENTRYPOINT ["/usr/local/bin/router-entrypoint.sh"]
