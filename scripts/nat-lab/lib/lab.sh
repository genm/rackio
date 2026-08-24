#!/usr/bin/env bash
# Shared driver for the Rackio NAT laboratory scenarios.
#
# The CLI is driven exactly the way scripts/test-two-daemon-pairing.sh and
# scripts/test-two-daemon-address-change.sh drive it — same commands, same jq
# field reads, same polling shape — so the lab and the host E2E tests cannot
# drift into two different definitions of "paired and reporting".
#
# Every scenario writes one report even when it fails. A scenario that cannot
# be established says so, with the state it actually observed; it never retries
# until green and never lowers an assertion to pass.

set -euo pipefail

lab_repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../../.." && pwd)"
lab_dir="$lab_repo_root/scripts/nat-lab"
lab_compose_file="$lab_dir/compose.yaml"
lab_results_dir="$lab_repo_root/test-results/nat-matrix"

LAB_AGENT_IMAGE="${LAB_AGENT_IMAGE:-rackio-nat-lab-agent:local}"
LAB_ROUTER_IMAGE="${LAB_ROUTER_IMAGE:-rackio-nat-lab-router:local}"
LAB_SERVICES_IMAGE="${LAB_SERVICES_IMAGE:-rackio-nat-lab-services:local}"
# Captures are bounded so a long scenario cannot fill a disk with evidence.
LAB_CAPTURE_SECONDS="${LAB_CAPTURE_SECONDS:-180}"
LAB_CAPTURE_PACKETS="${LAB_CAPTURE_PACKETS:-20000}"
LAB_CAPTURE_SNAPLEN="${LAB_CAPTURE_SNAPLEN:-128}"

# --- basics ----------------------------------------------------------------

lab_die() {
  echo "$*" >&2
  return 1
}

lab_now_ms() {
  node -e 'process.stdout.write(String(Date.now()))'
}

lab_compose() {
  docker compose --file "$lab_compose_file" "$@"
}

# --- image build -----------------------------------------------------------

lab_build_images() {
  echo "== building lab images for linux/arm64 from repository source"
  DOCKER_BUILDKIT=1 docker build \
    --platform linux/arm64 \
    --file "$lab_dir/agent.Dockerfile" \
    --tag "$LAB_AGENT_IMAGE" \
    "$lab_repo_root"
  DOCKER_BUILDKIT=1 docker build \
    --platform linux/arm64 \
    --file "$lab_dir/router.Dockerfile" \
    --tag "$LAB_ROUTER_IMAGE" \
    "$lab_repo_root"
  DOCKER_BUILDKIT=1 docker build \
    --platform linux/arm64 \
    --file "$lab_dir/services.Dockerfile" \
    --tag "$LAB_SERVICES_IMAGE" \
    "$lab_repo_root"
}

lab_agent_build_provenance() {
  # Record what the image actually contains, so a report cannot be attributed
  # to source it was not built from.
  local version
  version="$(docker run --rm --entrypoint /usr/local/bin/rackio "$LAB_AGENT_IMAGE" --version)"
  jq -n \
    --arg image "$LAB_AGENT_IMAGE" \
    --arg version "$version" \
    --arg commit "$(git -C "$lab_repo_root" rev-parse HEAD)" \
    --arg dirty "$(git -C "$lab_repo_root" status --porcelain | head -c 1 | grep -q . && echo true || echo false)" \
    --arg platform "linux/arm64" \
    '{image: $image, agent_version: $version, source_commit: $commit,
      source_tree_dirty: ($dirty == "true"), platform: $platform}'
}

# --- topology lifecycle ----------------------------------------------------

lab_up() {
  echo "== bringing the topology up"
  lab_compose up --detach --wait --wait-timeout 120
}

lab_down() {
  lab_compose down --volumes --remove-orphans --timeout 5 >/dev/null 2>&1 || true
}

lab_container() {
  echo "rackio-nat-lab-$1"
}

lab_exec() {
  local service="$1"
  shift
  docker exec "$(lab_container "$service")" "$@"
}

# --- rackio CLI, driven the way the host E2E scripts drive it ---------------

lab_rackio() {
  local service="$1"
  shift
  lab_exec "$service" rackio "$@"
}

lab_daemon_start() {
  local service="$1"
  docker exec --detach "$(lab_container "$service")" \
    sh -c 'exec rackio daemon >>/var/lib/rackio/daemon.log 2>&1'
}

lab_daemon_stop() {
  local service="$1" attempt
  lab_exec "$service" sh -c 'pkill -x rackio 2>/dev/null || true'
  for attempt in $(seq 1 40); do
    if ! lab_exec "$service" sh -c 'pgrep -x rackio >/dev/null 2>&1'; then
      return 0
    fi
    sleep 0.25
  done
  lab_die "daemon on $service did not stop"
}

lab_wait_for_command() {
  local description="$1"
  shift
  local attempt
  for attempt in $(seq 1 60); do
    if "$@" >/dev/null 2>&1; then
      return 0
    fi
    sleep 0.25
  done
  lab_die "timed out waiting for $description"
}

# Field reads mirror scripts/test-two-daemon-address-change.sh exactly.
lab_remote_field() {
  local viewer="$1" field="$2"
  lab_rackio "$viewer" fleet 2>/dev/null | jq -r ".data.remotes[0].$field"
}

lab_wait_for_remote_sample() {
  local viewer="$1" attempt fleet
  for attempt in $(seq 1 80); do
    fleet="$(lab_rackio "$viewer" fleet 2>/dev/null || true)"
    if [[ -n "$fleet" ]] &&
      [[ "$(jq -r '.data.remotes | length' <<<"$fleet")" == "1" ]] &&
      [[ "$(jq -r '.data.remotes[0].latest.cpu_percent != null' <<<"$fleet")" == "true" ]]; then
      printf '%s' "$fleet"
      return 0
    fi
    sleep 0.25
  done
  lab_die "remote metric sample did not arrive within twenty seconds"
}

# The viewer derives stale/offline from its own last-seen clock, so recovery is
# only proven by a metric sequence past the one seen before the restart. Taken
# verbatim from the host address-change test.
lab_wait_for_sequence_beyond() {
  local viewer="$1" previous="$2" deadline="$3" attempt current state
  for attempt in $(seq 1 "$deadline"); do
    current="$(lab_remote_field "$viewer" 'latest.sequence // -1')"
    state="$(lab_remote_field "$viewer" 'state')"
    if [[ "$current" != "null" ]] && [[ "$current" -gt "$previous" ]] &&
      [[ "$state" == "healthy" || "$state" == "warning" || "$state" == "critical" ]]; then
      return 0
    fi
    sleep 1
  done
  lab_die "the viewer did not receive a fresh sample beyond sequence $previous"
}

# The same reads, addressed by endpoint id instead of by position.
#
# A scenario that watches two machines at once cannot use `remotes[0]`: the
# order is the registry's, not the scenario's, and reading the wrong machine
# would silently swap a relayed result for a direct one.
lab_remote_field_of() {
  local viewer="$1" endpoint_id="$2" field="$3"
  lab_rackio "$viewer" fleet 2>/dev/null |
    jq -r --arg id "$endpoint_id" \
      ".data.remotes[] | select(.endpoint_id == \$id) | .$field" |
    head -n 1
}

lab_wait_for_remote_sample_of() {
  local viewer="$1" endpoint_id="$2" attempt value
  for attempt in $(seq 1 80); do
    value="$(lab_remote_field_of "$viewer" "$endpoint_id" 'latest.cpu_percent')"
    if [[ -n "$value" && "$value" != "null" ]]; then
      return 0
    fi
    sleep 0.25
  done
  lab_die "no metric sample arrived from $endpoint_id within twenty seconds"
}

lab_wait_for_state_of() {
  local viewer="$1" endpoint_id="$2" expected="$3" deadline="$4" attempt
  for attempt in $(seq 1 "$deadline"); do
    if [[ "$(lab_remote_field_of "$viewer" "$endpoint_id" 'state')" == "$expected" ]]; then
      return 0
    fi
    sleep 1
  done
  lab_die "the viewer never reported $expected for $endpoint_id"
}

lab_wait_for_path_of() {
  local viewer="$1" endpoint_id="$2" expected="$3" deadline="$4" attempt
  for attempt in $(seq 1 "$deadline"); do
    if [[ "$(lab_remote_field_of "$viewer" "$endpoint_id" 'path')" == "$expected" ]]; then
      return 0
    fi
    sleep 1
  done
  lab_die "the path to $endpoint_id never became $expected"
}

# The same wait, but reporting whether it happened instead of failing.
#
# Used where "did this happen on its own?" is the measurement rather than the
# requirement, so the answer can be recorded either way.
lab_try_wait_for_path_of() {
  local viewer="$1" endpoint_id="$2" expected="$3" deadline="$4" attempt
  for attempt in $(seq 1 "$deadline"); do
    if [[ "$(lab_remote_field_of "$viewer" "$endpoint_id" 'path')" == "$expected" ]]; then
      return 0
    fi
    sleep 1
  done
  return 1
}

lab_wait_for_sequence_beyond_of() {
  local viewer="$1" endpoint_id="$2" previous="$3" deadline="$4" attempt current state
  for attempt in $(seq 1 "$deadline"); do
    current="$(lab_remote_field_of "$viewer" "$endpoint_id" 'latest.sequence // -1')"
    state="$(lab_remote_field_of "$viewer" "$endpoint_id" 'state')"
    if [[ -n "$current" ]] && [[ "$current" != "null" ]] && [[ "$current" -gt "$previous" ]] &&
      [[ "$state" == "healthy" || "$state" == "warning" || "$state" == "critical" ]]; then
      return 0
    fi
    sleep 1
  done
  lab_die "no fresh sample from $endpoint_id past sequence $previous"
}

# Sample the reported path repeatedly while a session runs.
#
#   lab_sample_path_of <viewer> <endpoint_id> <samples> <interval_seconds>
#
# "the path was relayed" read once is a snapshot; a scenario that claims a path
# was *never* direct has to have looked more than once, and over long enough
# for iroh's five-second holepunch retry to have had several attempts.
lab_sample_path_of() {
  local viewer="$1" endpoint_id="$2" samples="$3" interval="$4" attempt
  local observed=()
  for attempt in $(seq 1 "$samples"); do
    observed+=("$(lab_remote_field_of "$viewer" "$endpoint_id" 'path')")
    sleep "$interval"
  done
  printf '%s\n' "${observed[@]}" | jq -R . | jq -s '[.[] | select(length > 0)]'
}

lab_wait_for_state() {
  local viewer="$1" expected="$2" deadline="$3" attempt
  for attempt in $(seq 1 "$deadline"); do
    if [[ "$(lab_remote_field "$viewer" 'state')" == "$expected" ]]; then
      return 0
    fi
    sleep 1
  done
  lab_die "the viewer never reported $expected"
}

# --- structured path-transition events -------------------------------------

# The agent logs two structured connection events as JSON lines under
# RACKIO_LOG_DIR (apps/agent/src/remote.rs):
#
#   * "remote connection path changed" — the path differs from the one this
#     viewer last reported, whether that happened mid-session or across a
#     reconnect;
#   * "remote monitoring session established" — a session started or resumed,
#     carrying the path it runs over. A reconnect that lands on the same path
#     is not a path change, so this is what records recovery.
#
# Both are carried verbatim rather than re-derived, and each keeps its own
# `event` name so a reader can tell a transition from a resumption.
lab_path_transition_events() {
  local viewer="$1"
  lab_exec "$viewer" sh -c 'cat /var/lib/rackio/log/agent.jsonl* 2>/dev/null || true' |
    jq -c 'select(type == "object")' 2>/dev/null |
    jq -s '[.[] | select(.fields.message == "remote connection path changed"
                      or .fields.message == "remote monitoring session established")
            | {timestamp, event: .fields.message,
               endpoint_id: .fields.endpoint_id,
               previous_path: .fields.previous_path,
               current_path: (.fields.current_path // .fields.path),
               rtt_ms: .fields.rtt_ms}]' 2>/dev/null || echo '[]'
}

# Path transitions are logged from a five-second refresh loop, so a transition
# can land after the first metric sample arrives. Give the loop a full cycle
# before reading the log, or a report would claim "no events" when it only
# looked too early.
lab_settle_path_events() {
  sleep 7
}

# What the agent emitted, counted by kind.
#
# Measured, not assumed: the counts come from the viewer's own log. A scenario
# asserts on them rather than on a re-derived summary, and an empty list is
# reported as empty — nothing here synthesises an event the product did not
# produce.
lab_observe_path_events() {
  local viewer="$1" events
  events="$(lab_path_transition_events "$viewer")"
  lab_observe path_transition_events "$events"
  lab_observe path_transition_event_coverage "$(jq -n \
    --argjson events "$events" \
    '{events_recorded: ($events | length),
      path_changes: [$events[] | select(.event == "remote connection path changed")] | length,
      sessions_established: [$events[] | select(.event == "remote monitoring session established")] | length,
      source: "RACKIO_LOG_DIR/agent.jsonl*, messages \"remote connection path changed\" and \"remote monitoring session established\"",
      note: "A reconnect that lands on the same path is a resumption, not a transition, and is recorded as a session-established event. Both kinds are required: a scenario whose path never changes must still show that monitoring resumed."}')"
  printf '%s' "$events"
}

# Read the viewer's monitored-machine registry and filter it on the host. The
# lab image carries the agent and capture tools, not a JSON toolchain.
lab_registry() {
  local viewer="$1" filter="$2"
  # Sorted keys so an unchanged registry never looks changed by field order.
  lab_exec "$viewer" cat /var/lib/rackio/data/monitored-machines.json |
    jq -r -S "$filter"
}

lab_relay_mode() {
  local service="$1"
  lab_exec "$service" sh -c 'cat /var/lib/rackio/log/agent.jsonl* 2>/dev/null || true' |
    jq -rs '[.[] | select(type == "object")
             | select(.fields.message == "agent started") | .fields.relay_mode]
            | last // "unknown"' 2>/dev/null || echo unknown
}

# --- the lab's self-hosted relay -------------------------------------------
#
# The image is `relay-package/` built unchanged: upstream `iroh-relay` pinned to
# 1.0.3, the same artefact an operator would run. Only the configuration
# differs, and every difference is recorded in the report and in README.md.
#
# The relay speaks plain HTTP. iroh 1.0.3 supports this explicitly — its relay
# client disables TLS exactly when the relay URL scheme is `http` — and it is
# the only option the lab has: the agent builds its endpoint with iroh's default
# `CaTlsConfig`, which trusts the compiled-in Mozilla root set and nothing else,
# so no certificate the lab can mint would be accepted. See README.md; the
# missing trust anchor is reported as a product finding rather than patched
# around here.
LAB_RELAY_ADDRESS="192.0.2.4"
LAB_RELAY_URL="http://192.0.2.4/"
lab_relay_config_dir="$lab_results_dir/relay"

# Render the relay's configuration, fail-closed on the endpoints named.
#
# The production package documents an `access.allowlist` holding the exact IDs
# printed by `rackio status`, so the lab writes exactly that. With no arguments
# the allowlist is empty, which is what the relay starts with: an unconfigured
# relay serves nobody.
lab_relay_render_config() {
  local ids=("$@") allowlist=""
  if [[ ${#ids[@]} -gt 0 ]]; then
    allowlist="$(printf '"%s", ' "${ids[@]}")"
    allowlist="${allowlist%, }"
  fi
  mkdir -p "$lab_relay_config_dir"
  cat >"$lab_relay_config_dir/config.toml" <<TOML
# Rendered by scripts/nat-lab/lib/lab.sh. Do not edit; it is overwritten.
#
# Differences from relay-package/config.example.toml, and only these:
#   * no [tls] section, so every relay service is served over plain HTTP. The
#     agent's endpoint trusts only iroh's compiled-in Mozilla roots, so a
#     lab-minted certificate could not be validated by it.
#   * enable_quic_addr_discovery = false, which iroh-relay requires when TLS is
#     absent. The lab therefore has no QUIC address discovery.
enable_relay = true
http_bind_addr = "0.0.0.0:80"
enable_quic_addr_discovery = false
enable_metrics = true
metrics_bind_addr = "0.0.0.0:9090"

access.allowlist = [$allowlist]
TOML
}

lab_endpoint_id() {
  lab_rackio "$1" status | jq -r '.data.endpoint_id'
}

# Point a machine at the lab relay. The daemon has to be restarted afterwards,
# exactly as `rackio relay set` says.
lab_use_relay() {
  local service="$1" response
  response="$(lab_rackio "$service" relay set "$LAB_RELAY_URL")"
  lab_assert_true "relay_set_on_$service" \
    "the operator's relay URL was accepted as configuration" \
    "$(jq -r '.ok' <<<"$response")"
  lab_assert_equal "relay_set_restart_required_on_$service" \
    "changing the relay asks for a restart rather than silently drifting" \
    "$(jq -r '.data.restart_required' <<<"$response")" "true"
}

# Authorise exactly these machines on the relay and restart it.
#
# The production package documents an allowlist holding the IDs printed by
# `rackio status`; the lab writes that same allowlist rather than opening the
# relay to everyone, so a relay report describes a relay configured the way the
# package tells an operator to configure one.
lab_relay_authorise() {
  local services=("$@") ids=() service
  for service in "${services[@]}"; do
    ids+=("$(lab_endpoint_id "$service")")
  done
  lab_relay_render_config "${ids[@]}"
  lab_relay_restart
  lab_observe relay_access "$(jq -n \
    --argjson ids "$(printf '%s\n' "${ids[@]}" | jq -R . | jq -s .)" \
    '{mode: "allowlist", authorised_endpoint_ids: $ids,
      note: "the relay was restarted with this fail-closed allowlist before the session was established, matching relay-package/README.md"}')"
}

lab_relay_rendered_config() {
  jq -Rs . <"$lab_relay_config_dir/config.toml"
}

lab_relay_wait_ready() {
  local attempt
  for attempt in $(seq 1 60); do
    if docker exec rackio-nat-lab-relay \
      curl --fail --silent --output /dev/null http://127.0.0.1:9090/metrics; then
      return 0
    fi
    sleep 0.5
  done
  lab_die "the relay did not become ready"
}

lab_relay_restart() {
  lab_compose restart relay >/dev/null
  lab_relay_wait_ready
}

lab_relay_stop() {
  lab_compose stop --timeout 3 relay >/dev/null
}

lab_relay_start() {
  lab_compose start relay >/dev/null
  lab_relay_wait_ready
}

lab_relay_running() {
  [[ "$(docker inspect --format '{{.State.Running}}' rackio-nat-lab-relay 2>/dev/null)" == "true" ]]
}

# The relay's own byte counters, which is the "relay byte count" the checklist
# asks every scenario to record. Reading them from the relay rather than
# inferring them from a capture keeps the number the relay's own statement
# about how much it carried.
lab_relay_byte_counters() {
  local text
  if ! lab_relay_running; then
    jq -n '{available: false, reason: "the relay was not running when this sample was taken",
            bytes_sent: null, bytes_recv: null}'
    return 0
  fi
  text="$(docker exec rackio-nat-lab-relay \
    curl --fail --silent http://127.0.0.1:9090/metrics 2>/dev/null || true)"
  local sent recv accepts
  sent="$(awk '$1 == "relayserver_bytes_sent_total" { print $2; exit }' <<<"$text")"
  recv="$(awk '$1 == "relayserver_bytes_recv_total" { print $2; exit }' <<<"$text")"
  accepts="$(awk '$1 == "relayserver_accepts_total" { print $2; exit }' <<<"$text")"
  if [[ -z "$sent" || -z "$recv" ]]; then
    jq -n '{available: false, reason: "the relay did not expose its byte counters",
            bytes_sent: null, bytes_recv: null}'
    return 0
  fi
  jq -n --argjson sent "$sent" --argjson recv "$recv" \
    --argjson accepts "${accepts:-0}" \
    '{available: true, source: "iroh-relay /metrics relayserver_bytes_sent_total and relayserver_bytes_recv_total",
      bytes_sent: $sent, bytes_recv: $recv, connections_accepted: $accepts}'
}

# --- packet capture --------------------------------------------------------

lab_capture_interface() {
  local service="$1" subnet="$2" prefix
  prefix="${subnet%/*}"
  prefix="${prefix%.*}."
  lab_exec "$service" sh -c \
    "ip -o -4 addr show | awk -v p='$prefix' '\$4 ~ \"^\"p { print \$2; exit }'"
}

# Capture on the interface that carries the path under test. Bounded by time,
# packet count and snaplen: headers are enough to prove which sockets carried
# the session, and a header-only capture keeps metric payloads out of evidence.
#
#   lab_capture_start <service> <subnet> [filter]
#
# The default filter is the path scenarios' one. `direct_only_isolation` widens
# it, because a scenario asking "did this machine send anything anywhere else?"
# cannot answer it from a filter that already excludes TCP — an HTTP request
# would be invisible to the very capture meant to rule it out.
LAB_CAPTURE_DEFAULT_FILTER='udp or icmp or icmp6'

lab_capture_start() {
  # `${3-...}`, not `${3:-...}`: an explicitly empty filter means "capture every
  # packet on the interface" and must not silently fall back to the UDP-only
  # default. It did exactly that once, and the isolation scenario then asserted
  # that no HTTP request was sent using a capture that could not have recorded
  # one.
  local service="$1" subnet="$2" filter="${3-$LAB_CAPTURE_DEFAULT_FILTER}"
  lab_capture_service="$service"
  lab_capture_filter="$filter"
  lab_capture_interface_name="$(lab_capture_interface "$service" "$subnet")"
  [[ -n "$lab_capture_interface_name" ]] ||
    lab_die "no interface on $service carries $subnet"
  lab_exec "$service" sh -c 'rm -f /tmp/capture.pcap'
  docker exec --detach "$(lab_container "$service")" sh -c \
    "exec timeout ${LAB_CAPTURE_SECONDS} tcpdump -i ${lab_capture_interface_name} \
       -n -s ${LAB_CAPTURE_SNAPLEN} -c ${LAB_CAPTURE_PACKETS} -U \
       -w /tmp/capture.pcap '${filter}' >/tmp/tcpdump.log 2>&1"
  # tcpdump needs a moment to attach before traffic starts, or the first
  # handshake would be missing from the evidence.
  sleep 1
}

# Turn tcpdump's text into one "<src>\t<dst>" line per packet.
#
# Shared by the flow summary and the egress analysis so there is exactly one
# implementation of "which address is which" in the lab. Getting this wrong
# invents peers that never existed, which both readers would then report.
lab_capture_endpoint_pairs() {
  awk '
    function host(token) {
      # tcpdump terminates the destination field with ":". Strip exactly that,
      # never the second colon of an IPv6 "::" — doing so turned the
      # unspecified address of a duplicate-address-detection probe into a
      # bogus peer named ":".
      if (token ~ /:$/ && token !~ /::$/) { sub(/:$/, "", token) }
      # IPv4 with a port has five dotted numbers; plain IPv4 has four.
      if (token ~ /^([0-9]+\.){4}[0-9]+$/) { sub(/\.[0-9]+$/, "", token) }
      # IPv6 with a port ends in ".<port>" after the colons.
      else if (token ~ /:/ && token ~ /\.[0-9]+$/) { sub(/\.[0-9]+$/, "", token) }
      return token
    }
    {
      src = ""; dst = ""
      for (i = 1; i <= NF; i++) {
        if ($i == "IP" || $i == "IP6") { src = $(i + 1); dst = $(i + 3); break }
      }
      if (src == "" || dst == "") next
      print host(src) "\t" host(dst)
    }'
}

# Summarise the capture from inside the container and copy the pcap out beside
# the JSON report.
#
#   lab_capture_finish <scenario> <required_a> <required_b> [also_allowed...]
#
# `required_a`/`required_b` are the two peer addresses the scenario claims
# carried the session; both must appear in the same flow for the direct claim
# to be credible. Every other *unicast* address seen on the wire is reported as
# an unexpected peer rather than ignored — that is what makes "no relay carried
# this" an observation instead of a restatement of the configuration.
# Multicast and link-local addresses are counted separately: mDNS pairing
# advertisement and neighbour discovery are link housekeeping, not a peer.
lab_capture_finish() {
  local scenario="$1" required_a="$2" required_b="$3"
  shift 3
  local allowed=("$required_a" "$required_b" "$@")
  local service="${lab_capture_service:-}"
  if [[ -z "$service" ]]; then
    echo 'null'
    return 0
  fi
  lab_exec "$service" sh -c 'pkill -x tcpdump 2>/dev/null || true' || true
  sleep 1

  local text flows total present peers unexpected housekeeping allow_pattern
  text="$(lab_exec "$service" sh -c \
    'tcpdump -nn -r /tmp/capture.pcap 2>/dev/null || true')"
  total="$(grep -c . <<<"$text" || true)"

  # tcpdump prints "IP <src>.<port> > <dst>.<port>:" for UDP and bare
  # "IP <src> > <dst>:" for ICMP; `lab_capture_endpoint_pairs` owns that
  # parsing for every reader of a capture.
  flows="$(lab_capture_endpoint_pairs <<<"$text" |
    awk -F'\t' '{
      key = ($1 < $2) ? $1 " <-> " $2 : $2 " <-> " $1
      if (!(key in seen)) { seen[key] = 1; print key }
    }' | sort)"

  present=false
  if grep -q -F -- "$required_a <-> $required_b" <<<"$flows" ||
    grep -q -F -- "$required_b <-> $required_a" <<<"$flows"; then
    present=true
  fi

  peers="$(tr ' ' '\n' <<<"$flows" | grep -v '<->' | grep . | sort -u || true)"
  # Link housekeeping rather than peers: IPv4/IPv6 multicast (mDNS pairing
  # advertisement), IPv6 link-local, and the unspecified source address a
  # duplicate-address-detection probe uses before an interface has one.
  local housekeeping_pattern='^(22[4-9]|23[0-9])\.|^ff[0-9a-f][0-9a-f]:|^fe80:|^::$'
  housekeeping="$(grep -E "$housekeeping_pattern" <<<"$peers" || true)"
  allow_pattern="$(printf '%s\n' "${allowed[@]}" |
    grep . | sed 's/[].[^$*\/]/\\&/g' | paste -sd '|' - || true)"
  unexpected="$(grep -v -E "^(${allow_pattern})\$" <<<"$peers" |
    grep -v -E "$housekeeping_pattern" || true)"

  mkdir -p "$lab_results_dir"
  docker cp "$(lab_container "$service"):/tmp/capture.pcap" \
    "$lab_results_dir/$scenario.pcap" >/dev/null 2>&1 || true

  jq -n \
    --arg file "$scenario.pcap" \
    --arg container "$(lab_container "$service")" \
    --arg interface "${lab_capture_interface_name:-}" \
    --arg required "$required_a <-> $required_b" \
    --argjson bounds "$(jq -n \
      --argjson seconds "$LAB_CAPTURE_SECONDS" \
      --argjson packets "$LAB_CAPTURE_PACKETS" \
      --argjson snaplen "$LAB_CAPTURE_SNAPLEN" \
      --arg filter "${lab_capture_filter:-(none: every packet on the interface)}" \
      '{max_seconds: $seconds, max_packets: $packets, snaplen_bytes: $snaplen,
        filter: $filter,
        note: "headers only; a header-only capture keeps metric payloads out of evidence"}')" \
    --argjson packets "${total:-0}" \
    --argjson flows "$(jq -R . <<<"$flows" | jq -s '[.[] | select(length > 0)]')" \
    --argjson expected_flow_present "$present" \
    --argjson unexpected_peers "$(jq -R . <<<"$unexpected" | jq -s '[.[] | select(length > 0)]')" \
    --argjson housekeeping "$(jq -R . <<<"$housekeeping" | jq -s '[.[] | select(length > 0)]')" \
    '{file: $file, container: $container, interface: $interface, bounds: $bounds,
      packets: $packets, host_flows: $flows, required_flow: $required,
      expected_direct_flow_present: $expected_flow_present,
      unexpected_unicast_peers: $unexpected_peers,
      multicast_and_link_local_peers: $housekeeping}'
}

# --- direct-only isolation (issue #19) -------------------------------------

# Every address the machine itself holds on the captured interface.
#
# The egress analysis needs to know which packets the machine *sent*, and its
# IPv4 address alone is not enough: neighbour discovery leaves from the
# interface's IPv6 link-local address, and a duplicate-address-detection probe
# leaves from the unspecified address. Missing those would classify the
# machine's own housekeeping as traffic it received.
lab_interface_addresses() {
  local service="$1" interface="$2"
  {
    lab_exec "$service" ip -o addr show dev "$interface" |
      awk '$3 == "inet" || $3 == "inet6" { split($4, a, "/"); print a[1] }'
    echo '::'
  } | grep . | sort -u | jq -R . | jq -s .
}

# Classify, from the capture, every destination this machine sent a packet to.
#
#   lab_capture_egress <self-addresses-json> <peer-addresses-json>
#
# `lab_capture_finish` answers "was the session carried between these two?".
# This answers the different question the isolation scenario asks: "of
# everything this machine put on the wire, did any of it go anywhere else?".
# It is directional on purpose — a bidirectional flow summary cannot tell a
# packet the machine sent from one somebody else sent at it, and only the first
# is the daemon's doing.
#
# Nothing is filtered away. Multicast, broadcast and link-local destinations are
# real traffic and are reported with their packet counts under their own
# heading; they are separated from the peer set rather than hidden, so a reader
# sees mDNS and neighbour discovery for what they are.
#
# Call it after `lab_capture_finish`, which stops tcpdump; this re-reads the
# same bounded capture file rather than taking a second one.
lab_capture_egress() {
  local self_json="$1" peers_json="$2"
  local service="${lab_capture_service:-}"
  [[ -n "$service" ]] || lab_die "lab_capture_egress was called without a capture"

  local text
  text="$(lab_exec "$service" sh -c \
    'tcpdump -nn -r /tmp/capture.pcap 2>/dev/null || true')"

  jq -r '.[]' <<<"$self_json" >"$lab_work/self-addresses"
  jq -r '.[]' <<<"$peers_json" >"$lab_work/peer-addresses"

  # Link housekeeping rather than a peer: IPv4 multicast, the subnet and global
  # broadcast addresses, IPv6 multicast (mDNS), IPv6 link-local (neighbour
  # discovery) and the unspecified address.
  local housekeeping_pattern='^(22[4-9]|23[0-9])\.|^255\.255\.255\.255$|\.255$|^ff[0-9a-f][0-9a-f]:|^fe80:|^::$'

  local table
  table="$(lab_capture_endpoint_pairs <<<"$text" |
    awk -F'\t' \
      -v self_file="$lab_work/self-addresses" \
      -v peer_file="$lab_work/peer-addresses" \
      -v housekeeping="$housekeeping_pattern" '
    BEGIN {
      while ((getline line < self_file) > 0) { if (line != "") is_self[line] = 1 }
      while ((getline line < peer_file) > 0) { if (line != "") is_peer[line] = 1 }
    }
    {
      total += 1
      if (!($1 in is_self)) { received += 1; next }
      sent += 1
      count[$2] += 1
    }
    END {
      printf "totals\t%d\t%d\t%d\n", total + 0, sent + 0, received + 0
      for (destination in count) {
        class = "unexpected"
        if (destination in is_peer) { class = "peer" }
        else if (destination ~ housekeeping) { class = "link_housekeeping" }
        printf "destination\t%s\t%d\t%s\n", destination, count[destination], class
      }
    }' | sort)"

  local totals destinations
  totals="$(awk -F'\t' '$1 == "totals" { print $2 "\t" $3 "\t" $4 }' <<<"$table")"

  # ARP is link-layer broadcast with no IP destination, so it cannot appear
  # under `destinations_sent_to` — and it must not simply vanish from the
  # accounting either. It is counted here, and the targets this machine asked
  # about are listed: an ARP for an off-path service would show intent to reach
  # it even if no IP packet ever followed.
  local capture_lines arp_targets
  capture_lines="$(grep -c . <<<"$text" || true)"
  arp_targets="$(awk -v self_file="$lab_work/self-addresses" '
    BEGIN {
      while ((getline line < self_file) > 0) { if (line != "") is_self[line] = 1 }
    }
    /ARP, Request who-has/ {
      target = ""; teller = ""
      for (i = 1; i <= NF; i += 1) {
        if ($i == "who-has") { target = $(i + 1) }
        if ($i == "tell") { teller = $(i + 1); sub(/,$/, "", teller) }
      }
      if (target != "" && (teller in is_self)) { count[target] += 1 }
    }
    END { for (target in count) { print target "\t" count[target] } }' <<<"$text" |
    sort |
    jq -R 'split("\t") | {address: .[0], arp_requests: (.[1] | tonumber)}' |
    jq -s .)"

  # The rows are turned into JSON by jq rather than by an awk `printf` of a JSON
  # literal. An awk program containing `{"a":1,"b":2}` is brace-expanded by the
  # shell into three separate commands before awk ever sees it, even inside
  # single quotes within a command substitution, which silently produced three
  # broken awk invocations and an empty result here.
  destinations="$(awk -F'\t' '$1 == "destination" { print $2 "\t" $3 "\t" $4 }' <<<"$table" |
    jq -R 'split("\t") | {address: .[0], packets: (.[1] | tonumber), classification: .[2]}' |
    jq -s 'sort_by(-.packets)')"

  jq -n \
    --argjson self "$self_json" \
    --argjson peers "$peers_json" \
    --argjson packets_total "$(cut -f1 <<<"$totals")" \
    --argjson packets_sent "$(cut -f2 <<<"$totals")" \
    --argjson packets_received "$(cut -f3 <<<"$totals")" \
    --argjson destinations "$destinations" \
    --argjson capture_lines "${capture_lines:-0}" \
    --argjson arp_targets "$arp_targets" \
    '{
      question: "of every packet this machine put on the wire, where did it go?",
      method: "directional read of the scenario capture: packets whose source is one of this machines own addresses, grouped by destination",
      self_addresses: $self, peer_addresses: $peers,
      packets_in_capture: $capture_lines,
      ip_packets_parsed: $packets_total,
      packets_sent_by_this_machine: $packets_sent,
      packets_received_by_this_machine: $packets_received,
      link_layer: {
        non_ip_packets: ($capture_lines - $packets_total),
        arp_requests_sent_by_this_machine: $arp_targets,
        note: "ARP and anything else without an IP header cannot appear under destinations_sent_to. It is counted here so the packet accounting adds up, and the ARP targets are listed because an ARP for an address is itself an attempt to reach it."
      },
      destinations_sent_to: $destinations,
      peer_destinations: [$destinations[] | select(.classification == "peer")],
      link_housekeeping_destinations: [$destinations[] | select(.classification == "link_housekeeping")],
      unexpected_destinations: [$destinations[] | select(.classification == "unexpected")],
      note: "link housekeeping is multicast, broadcast, IPv6 link-local and the unspecified address: mDNS advertisement during the pairing window and neighbour discovery. It is counted and listed rather than filtered out of the evidence."
    }'
}

# Prove from the machine itself that the off-path services are reachable.
#
#   lab_probe_offpath_services <service> <dns-name> <http-url>
#
# Run outside the capture window on purpose: these probes are packets to the
# resolver and the HTTP server, and inside the window they would be exactly the
# traffic the scenario claims is absent. Running them before the capture starts
# and again after it stops shows the services were up on both sides of it.
#
# `getent hosts` is glibc's own resolver path, so the DNS probe needs no tool
# the product does not already imply; `curl` is lab tooling in the agent image.
lab_probe_offpath_services() {
  local service="$1" name="$2" url="$3" resolved="" http_status=""
  resolved="$(lab_exec "$service" sh -c \
    "getent hosts '$name' 2>/dev/null | awk 'NR == 1 { print \$1 }'" || true)"
  http_status="$(lab_exec "$service" sh -c \
    "curl --silent --show-error --max-time 5 --output /dev/null --write-out '%{http_code}' '$url' 2>/dev/null" || true)"
  jq -n \
    --arg name "$name" --arg url "$url" \
    --arg resolved "$resolved" --arg status "$http_status" \
    '{dns: {name: $name, resolved_to: (if $resolved == "" then null else $resolved end),
            answered: ($resolved != "")},
      http: {url: $url, status_code: (if $status == "" then null else ($status | tonumber) end),
             answered: ($status == "200")},
      note: "run outside the capture window; inside it these probes would be the very traffic the scenario claims is absent"}'
}

# --- UDP blocking ----------------------------------------------------------
#
# A middlebox that passes TCP and drops UDP is the case the checklist's "UDP
# blocked" row is about. One rule at the head of the router's FORWARD chain is
# enough and applies in both directions, so nothing about the topology has to
# change to produce it. The relay is reached over TCP, so it stays reachable —
# which is exactly what makes the outcome interesting rather than a plain
# outage.
lab_block_udp() {
  local router="$1"
  lab_exec "$router" iptables -I FORWARD 1 -p udp -j DROP
  lab_assert_true "udp_block_installed_on_$router" \
    "the drop rule is really in the router's FORWARD chain" \
    "$(lab_exec "$router" iptables -S FORWARD | grep -q -- '-p udp -j DROP' && echo true || echo false)"
}

lab_unblock_udp() {
  local router="$1"
  lab_exec "$router" iptables -D FORWARD -p udp -j DROP
  lab_assert_equal "udp_block_removed_from_$router" \
    "the drop rule is gone, so recovery is not being measured against a still-blocked link" \
    "$(lab_exec "$router" iptables -S FORWARD | grep -c -- '-p udp -j DROP' || true)" "0"
}

# --- relay-side capture and payload opacity (issue #19) --------------------

# Capture inside the relay's own network namespace.
#
# The relay image is the production package built unchanged and carries no
# capture tools, so a sidecar built from the lab's router image joins the
# relay's namespace instead of the relay image being modified for the lab. What
# the sidecar sees is exactly what the relay's socket sees.
#
# Unlike the path captures this one keeps full packet payloads: the claim under
# test is that the payload is unreadable, and a header-only capture would prove
# nothing about payload bytes. It stays bounded by packet count and duration.
LAB_RELAY_CAPTURE_PACKETS="${LAB_RELAY_CAPTURE_PACKETS:-8000}"
lab_relay_sniffer="rackio-nat-lab-relay-sniffer"

lab_relay_capture_start() {
  docker rm --force "$lab_relay_sniffer" >/dev/null 2>&1 || true
  docker run --detach --name "$lab_relay_sniffer" \
    --network "container:$(lab_container relay)" \
    --cap-add NET_RAW --cap-add NET_ADMIN \
    --entrypoint sh "$LAB_ROUTER_IMAGE" -c \
    "interface=\$(ip -o -4 addr show | awk '\$4 ~ /^192\.0\.2\./ { print \$2; exit }'); \
     exec timeout ${LAB_CAPTURE_SECONDS} tcpdump -i \"\$interface\" -n -s 0 \
       -c ${LAB_RELAY_CAPTURE_PACKETS} -U -w /tmp/relay.pcap 'tcp port 80'" >/dev/null
  # tcpdump needs a moment to attach before the session starts, or the relay
  # handshake would be missing from the evidence.
  sleep 2
}

# Stop the relay-side capture, copy it out, and scan it.
#
#   lab_relay_capture_opacity <scenario> <needles-json>
#
# The needles are values the viewer actually read out of this session, so a hit
# would mean the relay could have read the same thing.
lab_relay_capture_opacity() {
  local scenario="$1" needles="$2"
  docker exec "$lab_relay_sniffer" sh -c 'pkill -x tcpdump 2>/dev/null || true' >/dev/null 2>&1 || true
  sleep 1
  mkdir -p "$lab_results_dir"
  local pcap="$lab_results_dir/$scenario-relay.pcap"
  if ! docker cp "$lab_relay_sniffer:/tmp/relay.pcap" "$pcap" >/dev/null 2>&1; then
    docker rm --force "$lab_relay_sniffer" >/dev/null 2>&1 || true
    jq -n '{available: false,
            reason: "the relay-side capture could not be retrieved, so nothing is claimed about payload opacity"}'
    return 0
  fi
  docker rm --force "$lab_relay_sniffer" >/dev/null 2>&1 || true
  local needle_file="$lab_work/relay-needles.json"
  printf '%s' "$needles" >"$needle_file"
  local scan
  scan="$(node "$lab_dir/lib/scan-relay-capture.mjs" "$pcap" "$needle_file" 2>&1)" || {
    jq -n --arg error "$scan" \
      '{available: false, reason: "the relay capture could not be scanned", detail: $error}'
    return 0
  }
  jq -n --argjson scan "$scan" --arg file "$scenario-relay.pcap" \
    '{available: true, file: $file} + $scan'
}

# The assertion block issue #19 cites.
#
# Three separate claims, each recorded on its own:
#   * the relay carried this session — otherwise the scan says nothing;
#   * no value the viewer read appears in the relay's bytes, and no framed
#     protobuf message can be decoded out of them;
#   * the metadata the relay *can* see is listed, because it is real.
lab_assert_relay_payload_opacity() {
  local opacity="$1"
  lab_assert_true "relay_capture_available" \
    "payload opacity is only claimed when the relay's own capture was read" \
    "$(jq -r '.available // false' <<<"$opacity")"
  lab_assert_true "relay_capture_carried_the_session" \
    "the relay capture contains payload bytes, so the scan looked at real traffic" \
    "$(jq -r '(.observable_metadata.payload_bytes_carried // 0) > 0' <<<"$opacity")"
  lab_assert_equal "relay_cannot_decode_a_protobuf_frame" \
    "no length-prefixed rackio-protocol frame can be parsed out of the relay's bytes" \
    "$(jq -r '.decodable_protobuf_frame_count // -1' <<<"$opacity")" "0"
  lab_assert_equal "no_hidden_value_appears_in_the_relay_capture" \
    "no display name, node id or metric value the viewer read appears in the relay's bytes" \
    "$(jq -r '[.needles[] | select(.expected_visible == false and .found == true)] | length' <<<"$opacity")" "0"
  lab_assert_equal "endpoint_identities_are_visible_to_the_relay" \
    "the relay routes by endpoint identity, so both identities must be found; this is metadata the relay really does see" \
    "$(jq -r '[.needles[] | select(.expected_visible == true and .found == false)] | length' <<<"$opacity")" "0"
}

# --- link packet loss ------------------------------------------------------

# The agent exposes no packet-loss metric — `rtt_ms` is the only connection
# quality number it reports. Loss is therefore measured at the link with ICMP
# and labelled as such. The report keeps `agent_reported_percent` explicitly
# null instead of presenting a link measurement as a product metric.
lab_packet_loss() {
  local service="$1" target="$2" output percent
  output="$(lab_exec "$service" ping -c 20 -i 0.2 -W 1 -q "$target" 2>&1 || true)"
  percent="$(sed -n 's/.*, \([0-9.]*\)% packet loss.*/\1/p' <<<"$output" | head -n 1)"
  if [[ -z "$percent" ]]; then
    jq -n --arg target "$target" --arg raw "$output" \
      '{source: "icmp_probe", target: $target, percent: null,
        agent_reported_percent: null, measured: false, raw: $raw,
        note: "the ICMP probe did not complete; loss is unknown, not zero"}'
    return 0
  fi
  jq -n --arg target "$target" --argjson percent "$percent" \
    '{source: "icmp_probe", target: $target, percent: $percent,
      agent_reported_percent: null, measured: true,
      note: "link-level ICMP loss on the lab path; the agent reports no per-connection packet loss"}'
}

# --- assertions and reporting ----------------------------------------------

lab_scenario_begin() {
  lab_scenario_id="$1"
  lab_expected_path="$2"
  lab_work="$(mktemp -d "${TMPDIR:-/tmp}/rackio-nat-lab.XXXXXX")"
  : >"$lab_work/assertions.jsonl"
  : >"$lab_work/observed.jsonl"
  lab_capture_service=""
  lab_capture_interface_name=""
  lab_capture_filter=""
  mkdir -p "$lab_results_dir"
}

lab_observe() {
  local key="$1" value="$2"
  jq -n --arg key "$key" --argjson value "$value" '{($key): $value}' \
    >>"$lab_work/observed.jsonl"
}

lab_observe_string() {
  lab_observe "$1" "$(jq -n --arg v "$2" '$v')"
}

# Record an assertion and fail the scenario if it does not hold. With `set -e`
# in the scenario, a false assertion aborts and the EXIT trap still writes the
# report — a scenario cannot fail silently or be retried into a pass.
lab_assert_equal() {
  local id="$1" detail="$2" actual="$3" expected="$4" ok=false
  [[ "$actual" == "$expected" ]] && ok=true
  jq -n --arg id "$id" --arg detail "$detail" --arg actual "$actual" \
    --arg expected "$expected" --argjson ok "$ok" \
    '{id: $id, ok: $ok, detail: $detail, actual: $actual, expected: $expected}' \
    >>"$lab_work/assertions.jsonl"
  if [[ "$ok" != true ]]; then
    echo "assertion failed: $id ($detail): expected '$expected', got '$actual'" >&2
    return 1
  fi
}

lab_assert_true() {
  lab_assert_equal "$1" "$2" "$3" "true"
}

# The check every scenario inherits.
#
# It was written while the lab had no relay, as a seam the relay scenarios
# would land on. They have landed on it: the relay scenarios drive the branch
# that was previously unreachable, and the direct-only scenarios still drive
# the other one.
#
#   * a direct claim requires the capture to show the session's own packets
#     between the two peer addresses and no unicast peer beyond the ones the
#     scenario declared, *and* the last path event not to be a relayed one;
#   * anything not reported as a direct path must be reported as `relayed`,
#     never as `lan_direct` or `wan_direct`;
#   * the relay configuration itself is checked against what the scenario set,
#     so a scenario cannot claim direct-only operation while a relay is
#     configured, or claim a relayed result on a machine that has no relay.
#
# `expected_relay_url` is the URL the scenario configured, or the string `null`
# for a direct-only machine.
lab_assert_relayed_never_reported_as_direct() {
  local reported_path="$1" relay_url="$2" relay_mode="$3" capture_json="$4" events_json="$5"
  local expected_relay_url="${6:-null}"
  local relayed_events last_path direct_claim capture_backs expected_mode

  expected_mode="direct_only"
  [[ "$expected_relay_url" != "null" ]] && expected_mode="self_hosted"
  lab_assert_equal "relay_configuration_is_what_the_scenario_set" \
    "the machine's own report of its relay must match the configuration under test" \
    "$relay_url/$relay_mode" "$expected_relay_url/$expected_mode"

  relayed_events="$(jq '[.[] | select(.current_path == "Relayed")] | length' <<<"$events_json")"
  # A migration scenario legitimately has relayed events in its history, so the
  # question for a direct claim is what the transport is *now*, not whether it
  # was ever relayed.
  last_path="$(jq -r '[.[] | select(.current_path != null)] | last | .current_path // "none"' <<<"$events_json")"
  direct_claim=false
  case "$reported_path" in
  lan_direct | wan_direct) direct_claim=true ;;
  esac

  if [[ "$direct_claim" == true ]]; then
    capture_backs="$(jq -r '.expected_direct_flow_present // false' <<<"$capture_json")"
    lab_assert_true "direct_claim_backed_by_capture" \
      "a direct path is only credible if the capture shows the peers' own packets" \
      "$capture_backs"
    lab_assert_equal "no_third_party_carried_the_session" \
      "the capture shows no unicast peer beyond the machines under test and any relay the scenario declared" \
      "$(jq -r '.unexpected_unicast_peers | length' <<<"$capture_json")" "0"
    lab_assert_true "transport_is_not_relayed_while_reporting_direct" \
      "the most recent path event must not classify the transport as relayed" \
      "$([[ "$last_path" != "Relayed" ]] && echo true || echo false)"
    if [[ "$expected_relay_url" == "null" ]]; then
      lab_assert_equal "no_relayed_transition_on_a_direct_only_machine" \
        "a machine with no relay configured can never have had a relayed transport" \
        "$relayed_events" "0"
    fi
  else
    # The branch the relay scenarios exercise: a transport that is not direct
    # must surface as `relayed`, and the reverse implication is checked above.
    lab_assert_equal "relayed_transport_reported_as_relayed" \
      "a transport that is not direct must be reported as relayed" \
      "$reported_path" "relayed"
    lab_assert_equal "a_relayed_result_requires_a_configured_relay" \
      "a relayed path is only possible on a machine the operator pointed at a relay" \
      "$expected_mode" "self_hosted"
  fi
}

# The last line of every scenario. Without it a scenario that dies early — a
# syntax error, a killed shell, an unset variable — could leave `$?` at zero
# and be reported as a pass. A pass has to be claimed explicitly.
lab_scenario_complete() {
  : >"$lab_work/complete"
}

lab_scenario_finish() {
  local status=$?
  trap - EXIT
  local result="pass" failure=null
  if [[ "$status" -ne 0 ]]; then
    result="fail"
    failure="$(jq -n --arg m "the scenario stopped before finishing; see the observed state and the daemon logs" '$m')"
  elif [[ ! -f "$lab_work/complete" ]]; then
    # Fail closed: the shell returned zero but never reached the end of the
    # scenario, so nothing here was actually proven.
    result="fail"
    status=1
    failure="$(jq -n --arg m "the scenario exited zero without reaching its final step, so its result is unknown rather than passing" '$m')"
  fi
  local assertions observed
  assertions="$(jq -s '.' "$lab_work/assertions.jsonl")"
  observed="$(jq -s 'add // {}' "$lab_work/observed.jsonl")"
  # An assertion recorded as false makes the scenario fail even if the shell
  # somehow returned zero.
  if [[ "$(jq '[.[] | select(.ok == false)] | length' <<<"$assertions")" != "0" ]]; then
    result="fail"
    status=1
    failure="$(jq -n --arg m "at least one assertion did not hold" '$m')"
  fi

  jq -n \
    --arg scenario "$lab_scenario_id" \
    --arg result "$result" \
    --arg expected_path "$lab_expected_path" \
    --argjson assertions "$assertions" \
    --argjson observed "$observed" \
    --argjson failure "$failure" \
    --arg generated_at "$(date -u +%Y-%m-%dT%H:%M:%SZ)" \
    '{schema: "rackio.nat-matrix/1", scenario: $scenario, result: $result,
      generated_at: $generated_at, expected_path: $expected_path}
     + $observed
     + {assertions: $assertions, failure: $failure}' \
    >"$lab_results_dir/$lab_scenario_id.json"

  echo "-- $lab_scenario_id: $result -> test-results/nat-matrix/$lab_scenario_id.json"
  rm -rf "$lab_work"
  exit "$status"
}
