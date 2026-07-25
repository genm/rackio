#!/bin/sh
set -eu

binary_path="${1:-target/release/rackio}"

if [ ! -f "$binary_path" ]; then
  printf '{"check":"release_binary_cloud_independence","status":"error","reason":"binary_not_found","path":"%s"}\n' "$binary_path" >&2
  exit 2
fi

matches="$(
  LC_ALL=C strings "$binary_path" |
    grep -Eo 'relay\.n0\.iroh\.link|staging-relay\.n0\.iroh\.link|dns\.iroh\.link' |
    sort -u ||
    true
)"

if [ -n "$matches" ]; then
  printf '{"check":"release_binary_cloud_independence","status":"failed","reason":"vendor_network_hostname_present","path":"%s"}\n' "$binary_path" >&2
  printf '%s\n' "$matches" >&2
  exit 1
fi

printf '{"check":"release_binary_cloud_independence","status":"passed","path":"%s"}\n' "$binary_path"
