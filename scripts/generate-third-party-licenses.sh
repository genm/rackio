#!/bin/sh
set -eu

unset CDPATH
repo_root="$(cd -- "$(dirname -- "$0")/.." && pwd)"
output="${1:-$repo_root/THIRDPARTY.html}"
config="$(mktemp "${TMPDIR:-/tmp}/rackio-about.XXXXXX")"
trap 'rm -f "$config"' EXIT HUP INT TERM

# deny.toml remains the license-policy SSOT. Generate cargo-about's equivalent
# config so dependency notices cannot silently diverge from the enforced policy.
# The translation parses TOML (smol-toml, a root devDependency), so it needs
# `pnpm install` to have run first.
node "$repo_root/scripts/cargo-about-config.mjs" "$repo_root/deny.toml" "$config"

cd "$repo_root"
cargo about generate \
  --workspace \
  --locked \
  --fail \
  --config "$config" \
  about.hbs \
  --output-file "$output"
