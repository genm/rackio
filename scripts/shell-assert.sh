#!/usr/bin/env bash

# Assertions for the two-daemon integration scripts.
#
# Apple Bash 3.2.57 — what `/usr/bin/env bash` resolves to on the supported
# macOS host — does not abort on a failing `[[ ... ]]` compound conditional
# unless the conditional is the final command. With only `set -euo pipefail`, a
# bare `[[ "$a" == "$b" ]]` in the middle of a script can fail while the script
# still runs to completion and exits 0, so the exit status cannot be trusted to
# say the assertion passed.
#
# Running the comparison inside these functions makes each assertion an ordinary
# command. Every supported shell treats a non-zero return from a simple command
# as fatal under `set -e`, and the failure names the contract that broke instead
# of reporting a bare exit status.
#
# Call these as ordinary commands, never as the condition of `if`, `&&` or `||`:
# `set -e` is suspended for a command used as a condition, which reintroduces
# exactly the silent success this file exists to prevent.

rackio_assert_eq() {
  local description="$1"
  local actual="$2"
  local expected="$3"

  if [ "$actual" = "$expected" ]; then
    return 0
  fi

  printf 'assertion failed: %s\n  expected: %s\n  actual:   %s\n' \
    "$description" "$expected" "$actual" >&2
  return 1
}

rackio_assert_ne() {
  local description="$1"
  local actual="$2"
  local unexpected="$3"

  if [ "$actual" != "$unexpected" ]; then
    return 0
  fi

  printf 'assertion failed: %s\n  did not expect: %s\n' \
    "$description" "$unexpected" >&2
  return 1
}
