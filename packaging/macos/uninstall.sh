#!/bin/sh
set -eu

purge=0
if [ "${1:-}" = "--purge" ]; then
  purge=1
elif [ "$#" -ne 0 ]; then
  echo "usage: uninstall.sh [--purge]" >&2
  exit 2
fi

[ "$(id -u)" -eq 0 ] || {
  echo "Rackio uninstall must run as root" >&2
  exit 1
}

launchctl bootout system/dev.rackio.agent >/dev/null 2>&1 || true
rm -f /Library/LaunchDaemons/dev.rackio.agent.plist /usr/local/bin/rackio
rm -rf /usr/local/lib/rackio

if [ "$purge" -eq 1 ]; then
  rm -rf \
    "/Library/Preferences/Rackio" \
    "/Library/Application Support/Rackio" \
    "/Library/Logs/Rackio"
  dscl . -delete /Users/_rackio >/dev/null 2>&1 || true
  dscl . -delete /Groups/_rackio-viewers >/dev/null 2>&1 || true
  echo "Rackio binaries, identity, configuration and history removed."
else
  echo "Rackio binaries removed; identity, configuration and history preserved."
fi
