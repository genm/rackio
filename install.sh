#!/bin/sh
set -eu

DEFAULT_RELEASES_URL="https://rackio.genm.dev/releases"
VERSION="${RACKIO_VERSION:-latest}"
RELEASES_URL="${RACKIO_RELEASES_URL:-$DEFAULT_RELEASES_URL}"
INSTALL_ROOT="${RACKIO_INSTALL_ROOT:-}"
SKIP_SERVICE="${RACKIO_SKIP_SERVICE:-0}"

usage() {
  cat <<'EOF'
Install the Rackio headless agent and systemd service.

Usage:
  install.sh [--version VERSION] [--releases-url URL]

Environment:
  RACKIO_VERSION       Version to install (default: latest)
  RACKIO_RELEASES_URL  Static release root

The installer downloads and verifies an immutable release archive before it
requests root privileges. RACKIO_INSTALL_ROOT and RACKIO_SKIP_SERVICE are only
for repository integration tests.
EOF
}

fail() {
  printf 'rackio install: %s\n' "$*" >&2
  exit 1
}

while [ "$#" -gt 0 ]; do
  case "$1" in
    --version)
      [ "$#" -ge 2 ] || fail "--version requires a value"
      VERSION="$2"
      shift 2
      ;;
    --releases-url)
      [ "$#" -ge 2 ] || fail "--releases-url requires a value"
      RELEASES_URL="$2"
      shift 2
      ;;
    --help|-h)
      usage
      exit 0
      ;;
    *)
      fail "unknown argument: $1"
      ;;
  esac
done

command -v curl >/dev/null 2>&1 || fail "curl is required"
command -v tar >/dev/null 2>&1 || fail "tar is required"

if [ -n "$INSTALL_ROOT" ]; then
  TEST_OS="${_RACKIO_TEST_OS:-$(uname -s)}"
  TEST_ARCH="${_RACKIO_TEST_ARCH:-$(uname -m)}"
else
  TEST_OS="$(uname -s)"
  TEST_ARCH="$(uname -m)"
fi

[ "$TEST_OS" = "Linux" ] || [ "$TEST_OS" = "linux" ] || fail "the headless installer supports Linux only"
case "$TEST_ARCH" in
  x86_64|amd64) TARGET="x86_64-unknown-linux-gnu" ;;
  aarch64|arm64) TARGET="aarch64-unknown-linux-gnu" ;;
  *) fail "unsupported Linux architecture: $TEST_ARCH" ;;
esac

RELEASES_URL="${RELEASES_URL%/}"
if [ "$VERSION" = "latest" ]; then
  VERSION="$(curl --proto '=https' --tlsv1.2 -fsSL "$RELEASES_URL/latest.txt" 2>/dev/null || true)"
  [ -n "$VERSION" ] || fail "could not resolve the latest Rackio version"
fi
case "$VERSION" in
  *[!0-9A-Za-z._-]*|'') fail "invalid version: $VERSION" ;;
esac

ASSET="rackio-v${VERSION}-${TARGET}.tar.gz"
BASE_URL="$RELEASES_URL/v${VERSION}"
TEMP_DIR="$(mktemp -d "${TMPDIR:-/tmp}/rackio-install.XXXXXX")"
cleanup() {
  rm -rf "$TEMP_DIR"
}
trap cleanup EXIT HUP INT TERM

download() {
  source_url="$1"
  destination="$2"
  case "$source_url" in
    https://*) curl --proto '=https' --tlsv1.2 -fsSL "$source_url" -o "$destination" ;;
    file://*)
      [ -n "$INSTALL_ROOT" ] || fail "file releases are allowed only in repository tests"
      curl -fsSL "$source_url" -o "$destination"
      ;;
    *) fail "release URL must use HTTPS" ;;
  esac
}

download "$BASE_URL/$ASSET" "$TEMP_DIR/$ASSET"
download "$BASE_URL/$ASSET.sha256" "$TEMP_DIR/$ASSET.sha256"

verify_checksum() {
  expected="$(awk 'NR == 1 { print $1 }' "$TEMP_DIR/$ASSET.sha256")"
  printf '%s\n' "$expected" | grep -Eq '^[0-9a-fA-F]{64}$' ||
    fail "release checksum must contain one SHA-256 digest"
  if command -v sha256sum >/dev/null 2>&1; then
    actual="$(sha256sum "$TEMP_DIR/$ASSET" | awk '{ print $1 }')"
  elif command -v shasum >/dev/null 2>&1; then
    actual="$(shasum -a 256 "$TEMP_DIR/$ASSET" | awk '{ print $1 }')"
  else
    fail "sha256sum or shasum is required"
  fi
  [ "$actual" = "$expected" ] || fail "release archive checksum does not match"
}

verify_checksum
tar -tzf "$TEMP_DIR/$ASSET" | awk '
  /^\// || /(^|\/)\.\.($|\/)/ { invalid = 1 }
  END { exit invalid }
' || fail "release archive contains an unsafe path"
mkdir "$TEMP_DIR/extracted"
tar -xzf "$TEMP_DIR/$ASSET" -C "$TEMP_DIR/extracted"
[ -f "$TEMP_DIR/extracted/rackio" ] || fail "release archive does not contain rackio"
[ -f "$TEMP_DIR/extracted/rackio.service" ] || fail "release archive does not contain rackio.service"
[ -f "$TEMP_DIR/extracted/uninstall.sh" ] || fail "release archive does not contain uninstall.sh"
[ ! -L "$TEMP_DIR/extracted/rackio" ] || fail "release binary must not be a symbolic link"
[ ! -L "$TEMP_DIR/extracted/rackio.service" ] || fail "release service must not be a symbolic link"
[ ! -L "$TEMP_DIR/extracted/uninstall.sh" ] || fail "release uninstaller must not be a symbolic link"
chmod 0755 "$TEMP_DIR/extracted/rackio" "$TEMP_DIR/extracted/uninstall.sh"
"$TEMP_DIR/extracted/rackio" --version >/dev/null 2>&1 ||
  fail "downloaded Rackio binary cannot run on this machine"

root_path() {
  printf '%s%s' "$INSTALL_ROOT" "$1"
}

run_root() {
  if [ -n "$INSTALL_ROOT" ] || [ "$(id -u)" -eq 0 ]; then
    "$@"
  elif command -v sudo >/dev/null 2>&1; then
    sudo "$@"
  else
    fail "root privileges are required and sudo is unavailable"
  fi
}

BIN_PATH="$(root_path /usr/local/bin/rackio)"
LIB_DIR="$(root_path /usr/local/lib/rackio)"
UNIT_PATH="$(root_path /etc/systemd/system/rackio.service)"
run_root install -d -m 0755 "$(dirname "$BIN_PATH")" "$LIB_DIR" "$(dirname "$UNIT_PATH")"
run_root install -m 0755 "$TEMP_DIR/extracted/rackio" "$BIN_PATH"
run_root install -m 0755 "$TEMP_DIR/extracted/uninstall.sh" "$LIB_DIR/uninstall.sh"
run_root install -m 0644 "$TEMP_DIR/extracted/rackio.service" "$UNIT_PATH"

if [ "$SKIP_SERVICE" = "1" ]; then
  [ -n "$INSTALL_ROOT" ] || fail "RACKIO_SKIP_SERVICE is restricted to repository tests"
  printf 'Rackio %s installed into %s for repository verification.\n' "$VERSION" "$INSTALL_ROOT"
  exit 0
fi

command -v systemctl >/dev/null 2>&1 || fail "systemd is required by this installer"
command -v getent >/dev/null 2>&1 || fail "getent is required by this installer"
if ! getent group rackio-viewers >/dev/null 2>&1; then
  run_root groupadd --system rackio-viewers
fi
if ! id rackio >/dev/null 2>&1; then
  run_root useradd \
    --system \
    --gid rackio-viewers \
    --home-dir /var/lib/rackio \
    --shell /usr/sbin/nologin \
    rackio
fi

VIEWER_USER="${RACKIO_VIEWER_USER:-$(id -un)}"
if [ "$VIEWER_USER" != "root" ] && id "$VIEWER_USER" >/dev/null 2>&1; then
  run_root usermod --append --groups rackio-viewers "$VIEWER_USER"
fi

run_root systemctl daemon-reload
run_root systemctl enable --now rackio.service
run_root systemctl is-active --quiet rackio.service ||
  fail "rackio.service did not become active; inspect: journalctl -u rackio.service"

run_root env RACKIO_SOCKET=/run/rackio/agent.sock "$BIN_PATH" status >/dev/null ||
  fail "rackio.service is active but its local health check failed"

cat <<EOF
Rackio $VERSION installed successfully.

Service: active
Mode: direct-only

Create a five-minute pairing bundle:
  sudo rackio pairing create

Check status:
  sudo rackio status

Uninstall while preserving identity and history:
  sudo /usr/local/lib/rackio/uninstall.sh
EOF

if [ "$VIEWER_USER" != "root" ]; then
  printf '\nLog out and back in before using Rackio without sudo (rackio-viewers membership).\n'
fi
