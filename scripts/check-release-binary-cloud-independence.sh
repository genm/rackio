#!/bin/sh
set -eu

binary_path="${1:-target/release/rackio}"
report_path="${RACKIO_RELEASE_REPORT:-test-results/release-binary-cloud-independence.json}"
diagnostic_path="${report_path%.json}.stderr"
mkdir -p "$(dirname "$report_path")"
rm -f "$diagnostic_path"

report() {
  status="$1"
  reason="${2:-}"
  if [ -n "$reason" ]; then
    printf '{"check":"release_binary_cloud_independence","status":"%s","reason":"%s","path":"%s"}\n' \
      "$status" "$reason" "$binary_path" >"$report_path"
  else
    printf '{"check":"release_binary_cloud_independence","status":"%s","path":"%s"}\n' \
      "$status" "$binary_path" >"$report_path"
  fi
}

if [ ! -f "$binary_path" ]; then
  report error binary_not_found
  cat "$report_path" >&2
  exit 2
fi

matches="$(
  LC_ALL=C strings "$binary_path" |
    grep -Eo 'relay\.n0\.iroh\.link|staging-relay\.n0\.iroh\.link|dns\.iroh\.link' |
    sort -u ||
    true
)"

if [ -n "$matches" ]; then
  report failed vendor_network_hostname_present
  {
    cat "$report_path"
    printf '%s\n' "$matches"
  } >"$diagnostic_path"
  cat "$diagnostic_path" >&2
  exit 1
fi

report passed
cat "$report_path"
