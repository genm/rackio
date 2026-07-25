#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
binary="${1:-}"
version="${2:-}"
target="${3:-}"
output_dir="${4:-$repo_root/dist}"

if [[ ! -x "$binary" || -z "$version" || -z "$target" ]]; then
  echo "usage: package-release.sh BINARY VERSION TARGET [OUTPUT_DIR]" >&2
  exit 2
fi
if [[ ! "$version" =~ ^[0-9A-Za-z._-]+$ || ! "$target" =~ ^[0-9A-Za-z._-]+$ ]]; then
  echo "version and target must contain only release-safe characters" >&2
  exit 2
fi

staging="$(mktemp -d "${TMPDIR:-/tmp}/rackio-release.XXXXXX")"
trap 'rm -rf "$staging"' EXIT
install -m 0755 "$binary" "$staging/rackio"
install -m 0644 "$repo_root/packaging/linux/rackio.service" "$staging/rackio.service"
install -m 0755 "$repo_root/packaging/linux/uninstall.sh" "$staging/uninstall.sh"
install -m 0644 "$repo_root/LICENSE-MIT" "$staging/LICENSE-MIT"
install -m 0644 "$repo_root/LICENSE-APACHE" "$staging/LICENSE-APACHE"

mkdir -p "$output_dir"
asset="rackio-v${version}-${target}.tar.gz"
tar -C "$staging" -czf "$output_dir/$asset" \
  rackio rackio.service uninstall.sh LICENSE-MIT LICENSE-APACHE
if command -v sha256sum >/dev/null 2>&1; then
  digest="$(sha256sum "$output_dir/$asset" | awk '{ print $1 }')"
else
  digest="$(shasum -a 256 "$output_dir/$asset" | awk '{ print $1 }')"
fi
printf '%s  %s\n' "$digest" "$asset" >"$output_dir/$asset.sha256"
printf '%s\n' "$output_dir/$asset"
