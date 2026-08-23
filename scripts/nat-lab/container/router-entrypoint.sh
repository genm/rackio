#!/usr/bin/env bash
# NAT router for the Rackio NAT laboratory.
#
# The NAT *behaviour* is a parameter, not a copy-pasted container: every router
# in the topology runs this one script and differs only by environment. The
# symmetric-NAT and relay scenarios of the release matrix attach here by adding
# a mode, not by forking the image.
#
#   NAT_MODE            endpoint_independent | symmetric
#   NAT_LAN_SUBNET      CIDR of the LAN this router hides, e.g. 192.168.102.0/24
#   NAT_WAN_SUBNET      CIDR of the shared "internet" segment, e.g. 192.0.2.0/24
#   NAT_PORT_FORWARDS   space separated proto:wan_port:lan_ip:lan_port entries
set -euo pipefail

mode="${NAT_MODE:-endpoint_independent}"
lan_subnet="${NAT_LAN_SUBNET:?NAT_LAN_SUBNET is required}"
wan_subnet="${NAT_WAN_SUBNET:?NAT_WAN_SUBNET is required}"
forwards="${NAT_PORT_FORWARDS:-}"

# Resolve interfaces from addresses. Docker does not guarantee that the first
# network in the compose file becomes eth0, so naming an interface literally
# would silently NAT the wrong side.
interface_in_subnet() {
  local subnet="$1" prefix
  prefix="${subnet%/*}"
  prefix="${prefix%.*}."
  ip -o -4 addr show |
    awk -v prefix="$prefix" '$4 ~ "^"prefix { print $2; exit }'
}

address_in_subnet() {
  local subnet="$1" prefix
  prefix="${subnet%/*}"
  prefix="${prefix%.*}."
  ip -o -4 addr show |
    awk -v prefix="$prefix" '$4 ~ "^"prefix { split($4, a, "/"); print a[1]; exit }'
}

lan_interface="$(interface_in_subnet "$lan_subnet")"
wan_interface="$(interface_in_subnet "$wan_subnet")"
wan_address="$(address_in_subnet "$wan_subnet")"

if [[ -z "$lan_interface" || -z "$wan_interface" || -z "$wan_address" ]]; then
  echo "router could not find both NAT interfaces (lan=$lan_interface wan=$wan_interface)" >&2
  ip -o -4 addr show >&2
  exit 1
fi

# compose sets net.ipv4.ip_forward through `sysctls:`, and /proc/sys is
# read-only in an unprivileged container. Verify it rather than assuming: a
# router that cannot forward would turn every scenario into a silent timeout
# instead of an obvious configuration error.
sysctl -w net.ipv4.ip_forward=1 >/dev/null 2>&1 || true
if [[ "$(cat /proc/sys/net/ipv4/ip_forward)" != "1" ]]; then
  echo "router cannot forward IPv4: net.ipv4.ip_forward is not enabled" >&2
  exit 1
fi
iptables -t nat -F
iptables -F FORWARD
iptables -P FORWARD ACCEPT
iptables -F INPUT

# A NAT router does not hand unsolicited inbound UDP to its own host stack, and
# this container must not either. Left accepting, the router answers a hole
# punch probe aimed at its WAN address itself: conntrack confirms an entry for
# (peer:port -> wan:port), and the internal machine that then wants to send out
# from that same port finds the reply tuple already taken, so nf_nat gives it a
# different external port. The peer's own NAT drops the answer because it came
# from a port it never wrote down, and a punch that both sides attempted
# correctly fails on the router's host stack rather than on any NAT property.
#
# Observed directly while building `cone_nat_hole_punch`: the monitored machine
# left its router as 192.0.2.5:19522 despite listening on 41641, and the
# viewer's router discarded every reply. ICMP is deliberately left alone so the
# scenarios' packet-loss probes still reach the router, and DNAT'd traffic is
# unaffected because a port forward is translated in PREROUTING and traverses
# FORWARD, never INPUT.
iptables -A INPUT -i "$wan_interface" -p udp -m conntrack --ctstate NEW -j DROP

case "$mode" in
endpoint_independent)
  # Linux MASQUERADE keeps the source port when it is free, so one internal
  # (address, port) keeps one external (address, port) whoever it talks to.
  # That is the cone-like behaviour a port forward depends on.
  masquerade_options=()
  ;;
symmetric)
  # `--random-fully` picks a fresh external port per flow, so an external
  # address learned from one peer is useless to another. `router-f` selects it
  # for `symmetric_nat_relay_fallback`, which is otherwise configured exactly
  # like the hole-punching pair.
  masquerade_options=(--random-fully)
  ;;
*)
  echo "unknown NAT_MODE: $mode" >&2
  exit 1
  ;;
esac

# Port forwards first: a forwarded service must keep one stable external
# (address, port) in both directions, so its return traffic is pinned with an
# explicit SNAT ahead of the general masquerade rule.
for forward in $forwards; do
  IFS=':' read -r protocol wan_port lan_ip lan_port <<<"$forward"
  if [[ -z "$protocol" || -z "$wan_port" || -z "$lan_ip" || -z "$lan_port" ]]; then
    echo "malformed NAT_PORT_FORWARDS entry: $forward" >&2
    exit 1
  fi
  if [[ "$mode" != "endpoint_independent" ]]; then
    echo "port forwarding is only meaningful with an endpoint-independent mapping, got $mode" >&2
    exit 1
  fi
  iptables -t nat -A PREROUTING -i "$wan_interface" -p "$protocol" \
    --dport "$wan_port" -j DNAT --to-destination "$lan_ip:$lan_port"
  iptables -t nat -A POSTROUTING -o "$wan_interface" -s "$lan_ip" -p "$protocol" \
    --sport "$lan_port" -j SNAT --to-source "$wan_address:$wan_port"
done

iptables -t nat -A POSTROUTING -s "$lan_subnet" -o "$wan_interface" \
  -j MASQUERADE "${masquerade_options[@]+"${masquerade_options[@]}"}"

echo "router ready: mode=$mode lan=$lan_interface($lan_subnet) wan=$wan_interface($wan_address) forwards=[${forwards}]"
iptables -t nat -S
iptables -S INPUT

# The runner drives the lab with `docker exec`; the container only has to stay
# up and keep forwarding.
exec sleep infinity
