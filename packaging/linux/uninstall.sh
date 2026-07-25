#!/bin/sh
set -eu

PURGE=0
if [ "${1:-}" = "--purge" ]; then
  PURGE=1
  shift
fi
[ "$#" -eq 0 ] || {
  printf 'usage: uninstall.sh [--purge]\n' >&2
  exit 2
}
[ "$(id -u)" -eq 0 ] || {
  printf 'rackio uninstall: run as root\n' >&2
  exit 1
}

if command -v systemctl >/dev/null 2>&1; then
  systemctl disable --now rackio.service >/dev/null 2>&1 || true
fi
rm -f /etc/systemd/system/rackio.service
rm -f /usr/local/bin/rackio
rm -f /usr/local/lib/rackio/uninstall.sh
rmdir /usr/local/lib/rackio 2>/dev/null || true
rm -rf /usr/local/share/doc/rackio
if command -v systemctl >/dev/null 2>&1; then
  systemctl daemon-reload
fi

if [ "$PURGE" -eq 1 ]; then
  rm -rf /etc/rackio /var/lib/rackio /var/log/rackio
  userdel rackio 2>/dev/null || true
  groupdel rackio-viewers 2>/dev/null || true
  printf 'Rackio removed, including local identity, pairing records, and history.\n'
else
  printf 'Rackio removed. /etc/rackio, /var/lib/rackio, and /var/log/rackio were preserved.\n'
fi
