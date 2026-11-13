#!/usr/bin/env bash
# Lab machine for the Rackio NAT laboratory.
#
# The container does not start the daemon. The runner starts, stops and
# restarts `rackio daemon` through `docker exec`, exactly as the host two-daemon
# scripts start and stop it as a child process, so a scenario can restart a
# machine without restarting its network namespace.
#
#   LAB_DEFAULT_GATEWAY   LAN address of this machine's NAT router, if any
#   LAB_RESOLVER          DNS resolver this machine is configured to use, if any
set -euo pipefail

mkdir -p \
  "${RACKIO_CONFIG_DIR:?}" \
  "${RACKIO_DATA_DIR:?}" \
  "${RACKIO_STATE_DIR:?}" \
  "${RACKIO_LOG_DIR:?}"

# Docker points every container at its own bridge gateway. A machine that is
# supposed to live behind a NAT router has to route through that router
# instead, or the lab would quietly test a flat network.
if [[ -n "${LAB_DEFAULT_GATEWAY:-}" ]]; then
  ip route replace default via "$LAB_DEFAULT_GATEWAY"
fi

# Point the machine at the lab's own resolver.
#
# Only `direct_only_isolation` sets this, and it matters there: a machine whose
# resolver is Docker's embedded one would send any DNS it did emit to a
# loopback address, where a capture on the LAN interface could not see it. With
# a real resolver on the LAN, a DNS query the daemon makes is a packet on the
# wire, and "no DNS was sent" becomes a measurement.
#
# /etc/resolv.conf is a bind mount, so its contents are rewritten in place
# rather than the file being replaced.
if [[ -n "${LAB_RESOLVER:-}" ]]; then
  printf 'nameserver %s\nsearch lab.test\n' "$LAB_RESOLVER" >/etc/resolv.conf
fi

echo "lab machine ready: $(hostname) gateway=${LAB_DEFAULT_GATEWAY:-docker-bridge}"
echo "resolver: ${LAB_RESOLVER:-docker-embedded}"
ip -o -4 addr show
ip route show

exec sleep infinity
