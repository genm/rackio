#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
# The build directory is shared across checkouts of this repository, so it is
# not always `$repo_root/target`. Cargo's own variable is the authority.
target_dir="${CARGO_TARGET_DIR:-$repo_root/target}"
binary="${1:-$target_dir/release/rackio}"
result_path="${RACKIO_BENCHMARK_RESULT:-$repo_root/test-results/resource-benchmark.json}"
benchmark_root="$(mktemp -d "${TMPDIR:-/tmp}/rackio-resource-benchmark.XXXXXX")"
agent_pid=""

cleanup() {
  exit_code=$?
  if [[ -n "$agent_pid" ]]; then
    kill "$agent_pid" 2>/dev/null || true
    wait "$agent_pid" 2>/dev/null || true
  fi
  if [[ "$exit_code" -ne 0 ]]; then
    mkdir -p "$(dirname "$result_path")"
    cp "$benchmark_root/agent.log" "$result_path.log" 2>/dev/null || true
  fi
  rm -rf "$benchmark_root"
  exit "$exit_code"
}
trap cleanup EXIT HUP INT TERM

if [[ ! -x "$binary" ]]; then
  printf '{"check":"agent_resources","status":"error","reason":"binary_not_executable","path":"%s"}\n' "$binary" >&2
  exit 2
fi

for command_name in awk jq ps; do
  if ! command -v "$command_name" >/dev/null 2>&1; then
    printf '{"check":"agent_resources","status":"error","reason":"missing_command","command":"%s"}\n' "$command_name" >&2
    exit 2
  fi
done

mkdir -p \
  "$benchmark_root/config" \
  "$benchmark_root/data" \
  "$benchmark_root/state" \
  "$benchmark_root/log"

agent_env=(
  env
  RACKIO_CONFIG_DIR="$benchmark_root/config"
  RACKIO_DATA_DIR="$benchmark_root/data"
  RACKIO_STATE_DIR="$benchmark_root/state"
  RACKIO_LOG_DIR="$benchmark_root/log"
  RACKIO_SOCKET="$benchmark_root/agent.sock"
)

rackio() {
  "${agent_env[@]}" "$binary" "$@"
}

# Started as a simple command rather than through the `rackio` helper on
# purpose. Backgrounding a shell *function* makes `$!` the subshell that wraps
# it, and sampling that subshell measures bash — an idle wrapper that reports
# roughly 1.5 MiB and no CPU whatever the daemon is doing, which is a passing
# number for the wrong process. `env` execs in place, so this `$!` is the agent.
"${agent_env[@]}" "$binary" daemon >"$benchmark_root/agent.log" 2>&1 &
agent_pid=$!

ready=false
for _ in {1..40}; do
  if rackio status >/dev/null 2>&1; then
    ready=true
    break
  fi
  if ! kill -0 "$agent_pid" 2>/dev/null; then
    printf '{"check":"agent_resources","status":"failed","reason":"daemon_exited_during_startup"}\n' >&2
    exit 1
  fi
  sleep 0.25
done

if [[ "$ready" != true ]]; then
  printf '{"check":"agent_resources","status":"failed","reason":"daemon_startup_timeout"}\n' >&2
  exit 1
fi

# A budget measured against the wrong process is worse than no budget: it
# passes for reasons that have nothing to do with the agent. Refuse to sample
# anything that is not the binary under test.
expected_command="$(basename "$binary")"
sampled_command="$(basename "$(ps -p "$agent_pid" -o comm= | awk 'NR == 1 { print $1 }')")"
if [[ "$sampled_command" != "$expected_command" ]]; then
  printf '{"check":"agent_resources","status":"failed","reason":"sampled_process_is_not_the_agent","expected":"%s","actual":"%s"}\n' \
    "$expected_command" "$sampled_command" >&2
  exit 1
fi

# Startup allocation and collector warm-up are not steady-state resource use.
sleep "${RACKIO_BENCHMARK_WARMUP_SECONDS:-5}"
samples_file="$benchmark_root/samples.tsv"
sample_count="${RACKIO_BENCHMARK_SAMPLES:-10}"
for ((sample = 0; sample < sample_count; sample += 1)); do
  if ! kill -0 "$agent_pid" 2>/dev/null; then
    printf '{"check":"agent_resources","status":"failed","reason":"daemon_exited_during_sampling"}\n' >&2
    exit 1
  fi
  ps -p "$agent_pid" -o %cpu= -o rss= |
    awk 'NF == 2 { print $1 "\t" $2 }' >>"$samples_file"
  sleep 1
done

actual_samples="$(wc -l <"$samples_file" | tr -d ' ')"
if [[ "$actual_samples" -ne "$sample_count" ]]; then
  printf '{"check":"agent_resources","status":"failed","reason":"incomplete_samples","expected":%s,"actual":%s}\n' \
    "$sample_count" "$actual_samples" >&2
  exit 1
fi

average_cpu_percent="$(
  awk '{ total += $1 } END { printf "%.3f", total / NR }' "$samples_file"
)"
peak_rss_kib="$(
  awk 'BEGIN { max = 0 } { if ($2 > max) max = $2 } END { print max }' "$samples_file"
)"
cpu_limit_percent="${RACKIO_BENCHMARK_CPU_LIMIT_PERCENT:-1}"
rss_limit_kib="${RACKIO_BENCHMARK_RSS_LIMIT_KIB:-40960}"

mkdir -p "$(dirname "$result_path")"
jq -n \
  --arg binary "$binary" \
  --argjson samples "$sample_count" \
  --argjson average_cpu_percent "$average_cpu_percent" \
  --argjson peak_rss_kib "$peak_rss_kib" \
  --argjson cpu_limit_percent "$cpu_limit_percent" \
  --argjson rss_limit_kib "$rss_limit_kib" \
  '{
    check: "agent_resources",
    status: (
      if $average_cpu_percent < $cpu_limit_percent and $peak_rss_kib < $rss_limit_kib
      then "passed"
      else "failed"
      end
    ),
    profile: "idle_direct_only",
    binary: $binary,
    samples: $samples,
    average_cpu_percent: $average_cpu_percent,
    peak_rss_kib: $peak_rss_kib,
    limits: {
      average_cpu_percent_exclusive: $cpu_limit_percent,
      peak_rss_kib_exclusive: $rss_limit_kib
    }
  }' | tee "$result_path"

if ! jq -e '.status == "passed"' "$result_path" >/dev/null; then
  exit 1
fi
