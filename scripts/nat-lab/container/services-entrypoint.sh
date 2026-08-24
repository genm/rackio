#!/usr/bin/env bash
# Off-path services for the Rackio NAT laboratory's `direct_only_isolation`
# scenario: a DNS resolver and an HTTP server on the monitored machine's LAN.
#
# Neither is part of any path under test. They are here so that "the daemon
# sent nothing to a resolver or a web server" is an observation about the
# daemon rather than an observation that the LAN was empty.
#
#   LAB_SERVICE_ROLE     resolver | http
#   LAB_HTTP_ADDRESS     address the resolver answers `http.lab.test` with
#   LAB_MONITORED_ADDRESS, LAB_VIEWER_ADDRESS   answered for the machine names
set -euo pipefail

role="${LAB_SERVICE_ROLE:?LAB_SERVICE_ROLE is required}"

echo "lab service ready: role=$role host=$(hostname)"
ip -o -4 addr show

case "$role" in
resolver)
  # `-R` and `-h`: no upstream resolver and no /etc/hosts. This resolver knows
  # exactly the lab's reserved `.test` names and nothing else, so it cannot
  # quietly forward a query out of the topology, and a query for anything else
  # is refused rather than answered from somewhere unrecorded.
  # `--local=/lab.test/` makes this resolver authoritative for the lab's domain.
  # Without it a query it has no record for — an AAAA lookup for a name with
  # only an A record, which every glibc and busybox client sends — comes back
  # REFUSED rather than empty, and a client reads that as a broken resolver.
  exec dnsmasq \
    --keep-in-foreground \
    --no-resolv \
    --no-hosts \
    --log-queries \
    --log-facility=- \
    --local=/lab.test/ \
    --address="/http.lab.test/${LAB_HTTP_ADDRESS:?}" \
    --address="/monitored.lab.test/${LAB_MONITORED_ADDRESS:?}" \
    --address="/viewer.lab.test/${LAB_VIEWER_ADDRESS:?}"
  ;;
http)
  mkdir -p /srv/www
  cat >/srv/www/index.html <<'HTML'
<!doctype html>
<title>rackio nat lab off-path service</title>
<p>This server exists so that a daemon not contacting it is a measurement.</p>
HTML
  exec busybox httpd -f -p 80 -h /srv/www
  ;;
*)
  echo "unknown LAB_SERVICE_ROLE: $role" >&2
  exit 1
  ;;
esac
