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
if [[ "$target" != "aarch64-apple-darwin" && "$target" != "x86_64-apple-darwin" ]]; then
  echo "target must be a supported macOS Rust target" >&2
  exit 2
fi
: "${RACKIO_INSTALLER_IDENTITY:?RACKIO_INSTALLER_IDENTITY is required for a release package}"
: "${RACKIO_APPLICATION_IDENTITY:?RACKIO_APPLICATION_IDENTITY is required for a release package}"

staging="$(mktemp -d "${TMPDIR:-/tmp}/rackio-macos-package.XXXXXX")"
trap 'rm -rf "$staging"' EXIT
payload="$staging/payload"
scripts="$staging/scripts"
mkdir -p \
  "$payload/usr/local/bin" \
  "$payload/usr/local/lib/rackio" \
  "$payload/usr/local/share/doc/rackio" \
  "$payload/Library/LaunchDaemons" \
  "$scripts" \
  "$output_dir"
install -m 0755 "$binary" "$payload/usr/local/bin/rackio"
codesign \
  --force \
  --options runtime \
  --timestamp \
  --sign "$RACKIO_APPLICATION_IDENTITY" \
  "$payload/usr/local/bin/rackio"
codesign --verify --strict --verbose=2 "$payload/usr/local/bin/rackio"
install -m 0755 "$repo_root/packaging/macos/uninstall.sh" \
  "$payload/usr/local/lib/rackio/uninstall.sh"
install -m 0644 "$repo_root/LICENSE-MIT" "$payload/usr/local/share/doc/rackio/LICENSE-MIT"
install -m 0644 "$repo_root/LICENSE-APACHE" "$payload/usr/local/share/doc/rackio/LICENSE-APACHE"
install -m 0644 "$repo_root/THIRDPARTY.html" "$payload/usr/local/share/doc/rackio/THIRDPARTY.html"
install -m 0644 "$repo_root/packaging/macos/dev.rackio.agent.plist" \
  "$payload/Library/LaunchDaemons/dev.rackio.agent.plist"
install -m 0755 "$repo_root/packaging/macos/scripts/preinstall" "$scripts/preinstall"
install -m 0755 "$repo_root/packaging/macos/scripts/postinstall" "$scripts/postinstall"

component="$staging/rackio-component.pkg"
unsigned="$staging/rackio-unsigned.pkg"
asset="rackio-v${version}-${target}.pkg"
pkgbuild \
  --root "$payload" \
  --scripts "$scripts" \
  --identifier dev.rackio.agent \
  --version "$version" \
  "$component"
productbuild --package "$component" "$unsigned"
productsign --sign "$RACKIO_INSTALLER_IDENTITY" "$unsigned" "$output_dir/$asset"
pkgutil --check-signature "$output_dir/$asset"

if [[ -n "${RACKIO_NOTARY_PROFILE:-}" ]]; then
  xcrun notarytool submit "$output_dir/$asset" \
    --keychain-profile "$RACKIO_NOTARY_PROFILE" \
    --wait
  xcrun stapler staple "$output_dir/$asset"
  xcrun stapler validate "$output_dir/$asset"
fi

digest="$(shasum -a 256 "$output_dir/$asset" | awk '{ print $1 }')"
printf '%s  %s\n' "$digest" "$asset" >"$output_dir/$asset.sha256"
printf '%s\n' "$output_dir/$asset"
