#!/usr/bin/env bash
# Run the Rackio NAT laboratory.
#
# Builds the lab images from this repository's source, runs every scenario in a
# freshly created topology, writes one JSON report and one packet capture per
# scenario under test-results/nat-matrix/, tears the topology down, and exits
# non-zero if any scenario failed.
#
#   scripts/nat-lab/run.sh                 # every scenario
#   scripts/nat-lab/run.sh same_lan_direct # one scenario
#
# This is opt-in release evidence, not a per-change gate: it builds a container
# image and runs real daemons for minutes. It is deliberately absent from
# `mise run check`.
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
source "$repo_root/scripts/nat-lab/lib/lab.sh"

# Scenario id -> script. Adding the relay and symmetric-NAT scenarios means
# adding a row here and a script beside the others; nothing else changes.
scenario_ids=(
  same_lan_direct
  port_forwarded_direct
  address_change
  symmetric_nat_relay_fallback
  relay_outage
  udp_blocked
  path_migration
  cone_nat_hole_punch
  direct_only_isolation
)
scenario_script() {
  case "$1" in
  same_lan_direct) echo "$lab_dir/scenarios/same-lan-direct.sh" ;;
  port_forwarded_direct) echo "$lab_dir/scenarios/port-forwarded-direct.sh" ;;
  address_change) echo "$lab_dir/scenarios/address-change.sh" ;;
  symmetric_nat_relay_fallback) echo "$lab_dir/scenarios/symmetric-nat-relay-fallback.sh" ;;
  relay_outage) echo "$lab_dir/scenarios/relay-outage.sh" ;;
  udp_blocked) echo "$lab_dir/scenarios/udp-blocked.sh" ;;
  path_migration) echo "$lab_dir/scenarios/path-migration.sh" ;;
  cone_nat_hole_punch) echo "$lab_dir/scenarios/cone-nat-hole-punch.sh" ;;
  direct_only_isolation) echo "$lab_dir/scenarios/direct-only-isolation.sh" ;;
  *) return 1 ;;
  esac
}

for tool in docker jq node; do
  command -v "$tool" >/dev/null 2>&1 || {
    echo "the NAT laboratory requires $tool" >&2
    exit 2
  }
done
docker compose version >/dev/null 2>&1 || {
  echo "the NAT laboratory requires the docker compose plugin" >&2
  exit 2
}

requested=("$@")
if [[ ${#requested[@]} -eq 0 ]]; then
  requested=("${scenario_ids[@]}")
fi
for id in "${requested[@]}"; do
  scenario_script "$id" >/dev/null || {
    echo "unknown scenario: $id (known: ${scenario_ids[*]})" >&2
    exit 2
  }
done

# Leaving a lab running would poison the next run's evidence with state from
# this one, so teardown happens on every exit path including interruption.
runner_cleanup() {
  local status="${1:-$?}"
  trap - EXIT HUP INT TERM
  lab_down
  exit "$status"
}
trap 'runner_cleanup $?' EXIT
trap 'runner_cleanup 129' HUP
trap 'runner_cleanup 130' INT
trap 'runner_cleanup 143' TERM

mkdir -p "$lab_results_dir"
lab_down
lab_build_images
provenance="$(lab_agent_build_provenance)"

failed=()
passed=()
for id in "${requested[@]}"; do
  echo
  echo "== scenario $id"
  # Each scenario gets a freshly created topology. Scenarios must be
  # reproducible one at a time from the checked-in compose file, which they are
  # not if they inherit another scenario's pairing state or NAT conntrack.
  lab_down
  # The relay starts with an empty allowlist, which denies everyone. A relay
  # scenario authorises exactly its own machines once their endpoint IDs exist;
  # a direct-only scenario leaves the relay serving nobody.
  lab_relay_render_config
  lab_up
  if bash "$(scenario_script "$id")"; then
    passed+=("$id")
  else
    failed+=("$id")
    echo "!! scenario $id failed; its report records the state that was observed" >&2
  fi
done
lab_down

jq -n \
  --argjson provenance "$provenance" \
  --arg generated_at "$(date -u +%Y-%m-%dT%H:%M:%SZ)" \
  --argjson passed "$(printf '%s\n' "${passed[@]+"${passed[@]}"}" | jq -R . | jq -s '[.[] | select(length > 0)]')" \
  --argjson failed "$(printf '%s\n' "${failed[@]+"${failed[@]}"}" | jq -R . | jq -s '[.[] | select(length > 0)]')" \
  --arg topology "scripts/nat-lab/compose.yaml" \
  '{schema: "rackio.nat-matrix-summary/1", generated_at: $generated_at,
    topology: $topology, build: $provenance,
    passed: $passed, failed: $failed,
    result: (if ($failed | length) == 0 then "pass" else "fail" end)}' \
  >"$lab_results_dir/summary.json"

echo
echo "== NAT matrix: ${#passed[@]} passed, ${#failed[@]} failed"
echo "   reports: test-results/nat-matrix/"
if [[ ${#failed[@]} -ne 0 ]]; then
  printf '   failed: %s\n' "${failed[*]}"
  exit 1
fi
