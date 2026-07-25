#!/bin/sh
set -eu

unset CDPATH
repo_root="$(cd -- "$(dirname -- "$0")/.." && pwd)"
output="${1:-$repo_root/THIRDPARTY.html}"
config="$(mktemp "${TMPDIR:-/tmp}/rackio-about.XXXXXX")"
trap 'rm -f "$config"' EXIT HUP INT TERM

# deny.toml remains the license-policy SSOT. Generate cargo-about's equivalent
# config so dependency notices cannot silently diverge from the enforced policy.
awk '
  /^\[graph\]$/ {
    section = "graph"
    next
  }
  /^\[licenses\]$/ {
    section = "licenses"
    next
  }
  /^\[/ {
    section = ""
    capture = ""
  }
  section == "graph" && /^targets = \[/ {
    print "targets = ["
    capture = "targets"
    found_targets = 1
    next
  }
  section == "licenses" && /^allow = \[/ {
    print "accepted = ["
    capture = "licenses"
    found_licenses = 1
    next
  }
  capture != "" && /^\]$/ {
    print "]"
    capture = ""
    next
  }
  capture != "" {
    print
  }
  END {
    if (!found_targets || !found_licenses) {
      print "deny.toml must define graph.targets and licenses.allow" > "/dev/stderr"
      exit 2
    }
  }
' "$repo_root/deny.toml" >"$config"

cd "$repo_root"
cargo about generate \
  --workspace \
  --locked \
  --fail \
  --config "$config" \
  about.hbs \
  --output-file "$output"
