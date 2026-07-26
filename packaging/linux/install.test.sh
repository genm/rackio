#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
test_root="$(mktemp -d "${TMPDIR:-/tmp}/rackio-install-test.XXXXXX")"
trap 'rm -rf "$test_root"' EXIT
mkdir -p "$test_root/bin" "$test_root/releases/v0.1.0"

fake_binary="$test_root/bin/rackio"
# shellcheck disable=SC2016 # The generated fixture must evaluate $1 at runtime.
printf '#!/bin/sh\n[ "${1:-}" = "--version" ] && { echo "rackio 0.1.0"; exit 0; }\nexit 1\n' \
  >"$fake_binary"
chmod 0755 "$fake_binary"
"$repo_root/packaging/linux/package-release.sh" \
  "$fake_binary" \
  0.1.0 \
  x86_64-unknown-linux-gnu \
  "$test_root/releases/v0.1.0" >/dev/null

install_root="$test_root/root"
RACKIO_INSTALL_ROOT="$install_root" \
RACKIO_SKIP_SERVICE=1 \
_RACKIO_TEST_OS=Linux \
_RACKIO_TEST_ARCH=x86_64 \
  sh "$repo_root/install.sh" \
  --version 0.1.0 \
  --releases-url "file://$test_root/releases"

test -x "$install_root/usr/local/bin/rackio"
test -x "$install_root/usr/local/lib/rackio/uninstall.sh"
test -f "$install_root/etc/systemd/system/rackio.service"
test -f "$install_root/usr/local/share/doc/rackio/THIRDPARTY.html"

# A second install is the normal upgrade/recovery path and must be idempotent.
RACKIO_INSTALL_ROOT="$install_root" \
RACKIO_SKIP_SERVICE=1 \
_RACKIO_TEST_OS=Linux \
_RACKIO_TEST_ARCH=x86_64 \
  sh "$repo_root/install.sh" \
  --version 0.1.0 \
  --releases-url "file://$test_root/releases" >/dev/null

asset="$test_root/releases/v0.1.0/rackio-v0.1.0-x86_64-unknown-linux-gnu.tar.gz"
local_install_root="$test_root/local-root"
RACKIO_INSTALL_ROOT="$local_install_root" \
RACKIO_SKIP_SERVICE=1 \
_RACKIO_TEST_OS=Linux \
_RACKIO_TEST_ARCH=x86_64 \
  sh "$repo_root/install.sh" \
  --archive "$asset" \
  --checksum "$asset.sha256" >/dev/null
test -x "$local_install_root/usr/local/bin/rackio"

missing_notice_dir="$test_root/missing-notice"
missing_notice_asset="$test_root/rackio-missing-notice.tar.gz"
mkdir "$missing_notice_dir"
tar -xzf "$asset" -C "$missing_notice_dir"
rm -f "$missing_notice_dir/THIRDPARTY.html"
tar -C "$missing_notice_dir" -czf "$missing_notice_asset" .
missing_notice_digest="$(shasum -a 256 "$missing_notice_asset" | awk '{ print $1 }')"
printf '%s  %s\n' "$missing_notice_digest" "$(basename "$missing_notice_asset")" \
  >"$missing_notice_asset.sha256"
if RACKIO_INSTALL_ROOT="$test_root/missing-notice-root" \
  RACKIO_SKIP_SERVICE=1 \
  _RACKIO_TEST_OS=Linux \
  _RACKIO_TEST_ARCH=x86_64 \
  sh "$repo_root/install.sh" \
  --archive "$missing_notice_asset" \
  --checksum "$missing_notice_asset.sha256" \
  2>"$test_root/missing-notice.stderr"; then
  echo "installer accepted an archive without third-party license notices" >&2
  exit 1
fi
grep -Fq "release archive does not contain third-party license notices" \
  "$test_root/missing-notice.stderr"

printf 'invalid checksum\n' \
  >"$asset.sha256"
if RACKIO_INSTALL_ROOT="$test_root/rejected" \
  RACKIO_SKIP_SERVICE=1 \
  _RACKIO_TEST_OS=Linux \
  _RACKIO_TEST_ARCH=x86_64 \
  sh "$repo_root/install.sh" \
  --version 0.1.0 \
  --releases-url "file://$test_root/releases"; then
  echo "installer accepted an invalid checksum" >&2
  exit 1
fi
test ! -e "$test_root/rejected/usr/local/bin/rackio"

if GITHUB_ACTIONS=false \
  RACKIO_SYSTEM_INSTALL_TEST=1 \
  bash "$repo_root/packaging/linux/systemd-install.test.sh" \
  "$asset" \
  "$asset.sha256" \
  2>"$test_root/system-install-guard.stderr"; then
  echo "system install test ran outside an opted-in GitHub-hosted runner" >&2
  exit 1
fi
grep -Fq "restricted to an explicitly opted-in GitHub-hosted runner" \
  "$test_root/system-install-guard.stderr"

printf '{"ok":true,"normal_install":true,"idempotent_reinstall":true,"local_archive_install":true,"missing_notice_rejection":true,"checksum_rejection":true,"system_install_guard":true}\n'
