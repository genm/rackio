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

# The agent logs path changes as JSON lines under RACKIO_LOG_DIR with the
# message "remote connection path changed" (apps/agent/src/remote.rs). They are
# the only structured path-transition record the product emits, so the report
# carries them verbatim rather than a re-derived summary.
lab_path_transition_events() {
  local viewer="$1"
  lab_exec "$viewer" sh -c 'cat /var/lib/rackio/log/agent.jsonl* 2>/dev/null || true' |
    jq -c 'select(type == "object")' 2>/dev/null |
    jq -s '[.[] | select(.fields.message == "remote connection path changed")
            | {timestamp, endpoint_id: .fields.endpoint_id,
               previous_path: .fields.previous_path,
               current_path: .fields.current_path,
               rtt_ms: .fields.rtt_ms}]' 2>/dev/null || echo '[]'
}

# Path transitions are logged from a five-second refresh loop, so a transition
# can land after the first metric sample arrives. Give the loop a full cycle
# before reading the log, or a report would claim "no events" when it only
# looked too early.
lab_settle_path_events() {
  sleep 7
}

# What the agent emits, plus what it does not.
#
# Measured, not assumed. In apps/agent/src/remote.rs the session-start path is
# written straight into the snapshot (`snapshot.path = path`, in the function
# that opens the metric stream) with no logging. Only `refresh_remote_state`
# compares before assigning, and it logs "remote connection path changed" just
# when the newly classified path differs from the snapshot it already holds.
#
# Two consequences the lab observed directly:
#   * establishing a session emits nothing, so a stable path records no event;
#   * a reconnect emits nothing either, because the reconnect path is written
#     by the silent session-start assignment, not by the comparing refresh.
#
# So an event can only ever fire when the path changes while one session stays
# alive — a mid-session migration, which direct-only mode cannot produce. In
# practice the agent emits no path-transition events at all today.
#
# docs/release-checklist.md requires structured path-transition events for every
# NAT-matrix scenario. Reporting the empty list with the reason is the honest
# outcome; synthesising an "initial path" event here would manufacture evidence
# the product does not produce.
lab_observe_path_events() {
  local viewer="$1" events
  events="$(lab_path_transition_events "$viewer")"
  lab_observe path_transition_events "$events"
  lab_observe path_transition_event_coverage "$(jq -n \
    --argjson count "$(jq 'length' <<<"$events")" \
    '{events_recorded: $count,
      initial_path_selection_logged: false,
      reconnect_path_selection_logged: false,
      source: "RACKIO_LOG_DIR/agent.jsonl*, message \"remote connection path changed\"",
      gap: "apps/agent/src/remote.rs assigns the session-start path to the snapshot silently, and only the five-second refresh compares before assigning. Establishing or re-establishing a session therefore emits no structured event; one can fire only if the path changes while a single session stays alive, which direct-only mode cannot produce. docs/release-checklist.md requires structured path-transition events for every NAT-matrix scenario, so this is an open product gap, not a limitation of the container lab."}')"
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
lab_capture_start() {
  local service="$1" subnet="$2"
  lab_capture_service="$service"
  lab_capture_interface_name="$(lab_capture_interface "$service" "$subnet")"
  [[ -n "$lab_capture_interface_name" ]] ||
    lab_die "no interface on $service carries $subnet"
  lab_exec "$service" sh -c 'rm -f /tmp/capture.pcap'
  docker exec --detach "$(lab_container "$service")" sh -c \
    "exec timeout ${LAB_CAPTURE_SECONDS} tcpdump -i ${lab_capture_interface_name} \
       -n -s ${LAB_CAPTURE_SNAPLEN} -c ${LAB_CAPTURE_PACKETS} -U \
       -w /tmp/capture.pcap 'udp or icmp or icmp6' >/tmp/tcpdump.log 2>&1"
  # tcpdump needs a moment to attach before traffic starts, or the first
  # handshake would be missing from the evidence.
  sleep 1
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
  # "IP <src> > <dst>:" for ICMP, so the port can only be stripped when one is
  # actually there. Getting this wrong turns 192.168.101.10 into 192.168.101
  # and invents a third party that never existed.
  flows="$(awk '
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
      src = host(src); dst = host(dst)
      key = (src < dst) ? src " <-> " dst : dst " <-> " src
      if (!(key in seen)) { seen[key] = 1; print key }
    }' <<<"$text" | sort)"

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
      '{max_seconds: $seconds, max_packets: $packets, snaplen_bytes: $snaplen,
        filter: "udp or icmp or icmp6",
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

# The check the relay scenarios inherit.
#
# Here it holds trivially — the lab has no relay and every network is internal
# — but it is asserted anyway so that when relay scenarios are added they get a
# check that is already wired into every report, rather than a new one written
# under pressure to make a relayed run look green.
#
#   * a direct claim requires relaying to be off *and* the capture to show the
#     session's own packets between the two peer addresses;
#   * any transport classified `Relayed` must be reported as `relayed`, never
#     as `lan_direct` or `wan_direct`.
lab_assert_relayed_never_reported_as_direct() {
  local reported_path="$1" relay_url="$2" relay_mode="$3" capture_json="$4" events_json="$5"
  local relayed_events direct_claim capture_backs

  lab_assert_equal "relay_disabled" \
    "the lab runs direct-only, so no relay can carry this session" \
    "$relay_url/$relay_mode" "null/direct_only"

  relayed_events="$(jq '[.[] | select(.current_path == "Relayed")] | length' <<<"$events_json")"
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
      "the capture shows no unicast peer other than the two machines under test" \
      "$(jq -r '.unexpected_unicast_peers | length' <<<"$capture_json")" "0"
    lab_assert_equal "no_relayed_transition_while_reporting_direct" \
      "no path-transition event classified the transport as relayed" \
      "$relayed_events" "0"
  else
    # Seam for the relay scenarios: a relayed transport must surface as
    # `relayed`, and the reverse implication is checked above.
    lab_assert_equal "relayed_transport_reported_as_relayed" \
      "a transport classified Relayed must be reported as relayed" \
      "$reported_path" "relayed"
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
