# syntax=docker/dockerfile:1.7
#
# The two off-path services the `direct_only_isolation` scenario needs: a DNS
# resolver and an HTTP server, sitting on the monitored machine's own LAN.
#
# They exist so that "the daemon contacted neither of them" is a result rather
# than an artefact of there being nothing to contact. One image, role chosen at
# run time by `LAB_SERVICE_ROLE`, the same pattern the router image uses.

FROM debian:bookworm-slim@sha256:abd67ffcfa541b485a3dff59865ab629aa048a6c613e639d36e7456b0b229241

# dnsmasq answers the lab's names, busybox serves HTTP and provides nslookup for
# the health check, curl is the HTTP health check, iproute2 reports the
# addresses each container actually got.
RUN apt-get update \
    && apt-get install --yes --no-install-recommends \
        busybox curl dnsmasq iproute2 \
    && rm -rf /var/lib/apt/lists/*

COPY scripts/nat-lab/container/services-entrypoint.sh /usr/local/bin/services-entrypoint.sh

ENTRYPOINT ["/usr/local/bin/services-entrypoint.sh"]
