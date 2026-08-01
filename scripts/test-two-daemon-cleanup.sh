#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
# Keep the nested Unix-domain socket path below macOS's SUN_LEN limit even when
# the caller's TMPDIR expands to a long /var/folders path.
test_root="$(mktemp -d /tmp/rackio-two-daemon-cleanup.XXXXXX)"
test_pid=""

cleanup() {
  status="${1:-$?}"
  trap - EXIT HUP INT TERM
  if [[ -n "$test_pid" ]]; then
    kill "$test_pid" 2>/dev/null || true
    wait "$test_pid" 2>/dev/null || true
  fi
  rm -rf "$test_root" || status=1
  exit "$status"
}
trap 'cleanup $?' EXIT
trap 'cleanup 129' HUP
trap 'cleanup 130' INT
trap 'cleanup 143' TERM

# The timeout below owns daemon readiness, not compilation latency. Build before
# starting the child so a cold worktree cannot consume the entire readiness
# window while the integration script is still compiling rackio-agent.
cargo build --manifest-path "$repo_root/Cargo.toml" -p rackio-agent >/dev/null

TMPDIR="$test_root" "$repo_root/scripts/test-two-daemon-pairing.sh" \
  >"$test_root/stdout.log" 2>"$test_root/stderr.log" &
test_pid=$!

integration_root=""
for _ in {1..300}; do
  candidate="$(find "$test_root" -maxdepth 1 -type d -name 'rackio-two-daemon.*' -print -quit)"
  if [[ -n "$candidate" && -S "$candidate/viewer.sock" && -S "$candidate/server.sock" ]]; then
    integration_root="$candidate"
    break
  fi
  if ! kill -0 "$test_pid" 2>/dev/null; then
    echo "two-daemon test exited before both daemon sockets were ready" >&2
    sed -n '1,160p' "$test_root/stderr.log" >&2
    exit 1
  fi
  sleep 0.1
done

if [[ -z "$integration_root" ]]; then
  echo "timed out waiting for two-daemon sockets" >&2
  sed -n '1,160p' "$test_root/stderr.log" >&2
  exit 1
fi

kill -TERM "$test_pid"
set +e
wait "$test_pid"
status=$?
set -e
test_pid=""

if [[ "$status" -eq 0 ]]; then
  echo "a terminated two-daemon test reported synthetic success" >&2
  exit 1
fi
if [[ -e "$integration_root" ]]; then
  echo "two-daemon integration root survived process cleanup" >&2
  exit 1
fi

printf '%s\n' '{"ok":true,"signal_failed_closed":true,"integration_root_removed":true}'
