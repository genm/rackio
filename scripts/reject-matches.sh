#!/usr/bin/env bash

rackio_reject_matches() {
  if [[ "$#" -lt 3 ]]; then
    echo "rackio_reject_matches requires a violation message, scan name and command" >&2
    return 2
  fi

  local violation_message="$1"
  local scan_name="$2"
  local scan_status
  shift 2

  # Pattern tools use 1 for a clean no-match. Any other non-zero status means
  # the scan could not prove absence and must not become synthetic success.
  if "$@"; then
    printf '%s\n' "$violation_message" >&2
    return 1
  else
    scan_status=$?
  fi

  if [[ "$scan_status" -eq 1 ]]; then
    return 0
  fi

  printf 'pattern scan failed before it could prove absence (%s, exit %d)\n' \
    "$scan_name" "$scan_status" >&2
  return "$scan_status"
}
