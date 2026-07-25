#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
test_root="$(mktemp -d "${TMPDIR:-/tmp}/rackio-install-test.XXXXXX")"
trap 'rm -rf "$test_root"' EXIT
mkdir -p "$test_root/bin" "$test_root/releases/v0.1.0"

fake_binary="$test_root/bin/rackio"
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

printf 'invalid checksum\n' \
  >"$test_root/releases/v0.1.0/rackio-v0.1.0-x86_64-unknown-linux-gnu.tar.gz.sha256"
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

printf '{"ok":true,"normal_install":true,"checksum_rejection":true}\n'
