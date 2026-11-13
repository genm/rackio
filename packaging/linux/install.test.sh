#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
test_root="$(mktemp -d "${TMPDIR:-/tmp}/rackio-install-test.XXXXXX")"
trap 'rm -rf "$test_root"' EXIT
mkdir -p "$test_root/bin" "$test_root/releases/v0.1.0"

fake_binary="$test_root/bin/rackio"
# shellcheck disable=SC2016 # The generated fixture must evaluate variables at runtime.
printf '%s\n' \
  '#!/bin/sh' \
  'if [ "${1:-}" = "--version" ]; then echo "rackio 0.1.0"; exit 0; fi' \
  'if [ "${1:-}" = "status" ] && [ -n "${RACKIO_FAKE_HEALTH_COUNT:-}" ]; then' \
  '  count="$(cat "$RACKIO_FAKE_HEALTH_COUNT" 2>/dev/null || echo 0)"' \
  '  count=$((count + 1))' \
  '  printf "%s\n" "$count" >"$RACKIO_FAKE_HEALTH_COUNT"' \
  '  [ "${RACKIO_FAKE_HEALTH_ALWAYS_FAIL:-0}" != "1" ] && [ "$count" -ge 3 ] && exit 0' \
  'fi' \
  'exit 1' \
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

# "latest" resolves through a pointer file. A static release root publishes it at
# `<root>/latest.txt`; a GitHub Release publishes it outside the versioned root,
# so `--latest-url` must be honoured independently of `--releases-url`.
printf '0.1.0\n' >"$test_root/releases/latest.txt"
pointer_install_root="$test_root/pointer-root"
RACKIO_INSTALL_ROOT="$pointer_install_root" \
RACKIO_SKIP_SERVICE=1 \
_RACKIO_TEST_OS=Linux \
_RACKIO_TEST_ARCH=x86_64 \
  sh "$repo_root/install.sh" \
  --releases-url "file://$test_root/releases" >/dev/null
test -x "$pointer_install_root/usr/local/bin/rackio"

printf '0.1.0\r\n' >"$test_root/moved-pointer.txt"
moved_pointer_install_root="$test_root/moved-pointer-root"
RACKIO_INSTALL_ROOT="$moved_pointer_install_root" \
RACKIO_SKIP_SERVICE=1 \
_RACKIO_TEST_OS=Linux \
_RACKIO_TEST_ARCH=x86_64 \
  sh "$repo_root/install.sh" \
  --releases-url "file://$test_root/releases" \
  --latest-url "file://$test_root/moved-pointer.txt" >/dev/null
test -x "$moved_pointer_install_root/usr/local/bin/rackio"

# A release root without a pointer must fail closed and name the way out, which
# is the state every unadvertised pre-release is in.
if RACKIO_INSTALL_ROOT="$test_root/pointerless-root" \
  RACKIO_SKIP_SERVICE=1 \
  _RACKIO_TEST_OS=Linux \
  _RACKIO_TEST_ARCH=x86_64 \
  sh "$repo_root/install.sh" \
  --releases-url "file://$test_root/pointerless" \
  2>"$test_root/pointerless.stderr"; then
  echo "installer resolved a version without a pointer" >&2
  exit 1
fi
grep -Fq "install an exact version with --version VERSION" "$test_root/pointerless.stderr"
test ! -e "$test_root/pointerless-root/usr/local/bin/rackio"

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
# Same fallback as `install.sh` and `package-release.sh`: a Debian-family host
# ships `sha256sum` and no `shasum`, and hardcoding either one is what kept this
# test from running anywhere but a macOS development machine.
if command -v sha256sum >/dev/null 2>&1; then
  missing_notice_digest="$(sha256sum "$missing_notice_asset" | awk '{ print $1 }')"
elif command -v shasum >/dev/null 2>&1; then
  missing_notice_digest="$(shasum -a 256 "$missing_notice_asset" | awk '{ print $1 }')"
else
  echo 'rackio install test: sha256sum or shasum is required' >&2
  exit 1
fi
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

fake_system_bin="$test_root/system-bin"
mkdir "$fake_system_bin"
for command_name in systemctl getent groupadd useradd usermod; do
  printf '#!/bin/sh\nexit 0\n' >"$fake_system_bin/$command_name"
  chmod 0755 "$fake_system_bin/$command_name"
done

service_releases="$test_root/service-releases"
mkdir -p "$service_releases/v0.1.0"
"$repo_root/packaging/linux/package-release.sh" \
  "$fake_binary" \
  0.1.0 \
  x86_64-unknown-linux-gnu \
  "$service_releases/v0.1.0" >/dev/null
service_asset="$service_releases/v0.1.0/rackio-v0.1.0-x86_64-unknown-linux-gnu.tar.gz"
health_count="$test_root/health-count"
printf '0\n' >"$health_count"

PATH="$fake_system_bin:$PATH" \
RACKIO_INSTALL_ROOT="$test_root/service-root" \
RACKIO_FAKE_HEALTH_COUNT="$health_count" \
_RACKIO_HEALTH_ATTEMPTS=3 \
_RACKIO_HEALTH_RETRY_DELAY=0 \
_RACKIO_TEST_OS=Linux \
_RACKIO_TEST_ARCH=x86_64 \
  sh "$repo_root/install.sh" \
  --archive "$service_asset" \
  --checksum "$service_asset.sha256" >/dev/null
test "$(cat "$health_count")" -eq 3

printf '0\n' >"$health_count"
if PATH="$fake_system_bin:$PATH" \
  RACKIO_INSTALL_ROOT="$test_root/unhealthy-service-root" \
  RACKIO_FAKE_HEALTH_COUNT="$health_count" \
  RACKIO_FAKE_HEALTH_ALWAYS_FAIL=1 \
  _RACKIO_HEALTH_ATTEMPTS=3 \
  _RACKIO_HEALTH_RETRY_DELAY=0 \
  _RACKIO_TEST_OS=Linux \
  _RACKIO_TEST_ARCH=x86_64 \
  sh "$repo_root/install.sh" \
  --archive "$service_asset" \
  --checksum "$service_asset.sha256" \
  2>"$test_root/unhealthy-service.stderr"; then
  echo "installer accepted a service that never became healthy" >&2
  exit 1
fi
grep -Fq "did not become healthy within 3 seconds" \
  "$test_root/unhealthy-service.stderr"
test "$(cat "$health_count")" -eq 3

printf '{"ok":true,"normal_install":true,"idempotent_reinstall":true,"default_pointer_install":true,"relocated_pointer_install":true,"missing_pointer_rejection":true,"local_archive_install":true,"missing_notice_rejection":true,"checksum_rejection":true,"system_install_guard":true,"delayed_service_health":true,"unhealthy_service_rejection":true}\n'
