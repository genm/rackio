import assert from "node:assert/strict";
import { existsSync, mkdtempSync, realpathSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { spawnSync } from "node:child_process";
import test from "node:test";

// The two-daemon scripts source this helper instead of writing bare `[[ ... ]]`
// assertions, because Apple Bash 3.2.57 does not honour `set -e` for a failing
// compound conditional that is not the final command. These tests pin that a
// failed assertion is fatal, that a passing one is quiet, and that an assertion
// failure still runs the script's exit trap.
const helper = resolve("scripts/shell-assert.sh");

// Cover every distinct `bash` binary this host offers, so the guarantee holds on
// the macOS 3.2 shell and the Linux CI shell alike.
function shells() {
  const seen = new Map();
  for (const candidate of ["bash", "/bin/bash"]) {
    const found = spawnSync("sh", ["-c", 'command -v "$0"', candidate], {
      encoding: "utf8",
    });
    if (found.status !== 0) {
      continue;
    }
    const path = found.stdout.trim();
    const identity = realpathSync(path);
    if (!seen.has(identity)) {
      seen.set(identity, path);
    }
  }
  return [...seen.values()];
}

function runAssertion(shell, script, args = []) {
  return spawnSync(shell, ["-c", script, "shell-assert-test", helper, ...args], {
    cwd: process.cwd(),
    encoding: "utf8",
  });
}

for (const shell of shells()) {
  test(`[${shell}] a passing assertion succeeds quietly`, () => {
    const result = runAssertion(
      shell,
      'set -euo pipefail\nsource "$1"\nrackio_assert_eq "control" "value" "value"\n',
    );
    assert.equal(result.status, 0);
    assert.equal(result.stderr, "");
  });

  test(`[${shell}] a failing equality assertion is fatal and names the contract`, () => {
    const result = runAssertion(
      shell,
      'set -euo pipefail\nsource "$1"\nrackio_assert_eq "distinct ports" "5" "7"\n',
    );
    assert.equal(result.status, 1);
    assert.equal(result.stdout, "");
    assert.match(result.stderr, /assertion failed: distinct ports/);
    assert.match(result.stderr, /expected: 7/);
    assert.match(result.stderr, /actual:   5/);
  });

  test(`[${shell}] a failing inequality assertion is fatal`, () => {
    const result = runAssertion(
      shell,
      'set -euo pipefail\nsource "$1"\nrackio_assert_ne "distinct ports" "5" "5"\n',
    );
    assert.equal(result.status, 1);
    assert.match(result.stderr, /assertion failed: distinct ports/);
    assert.match(result.stderr, /did not expect: 5/);
  });

  test(`[${shell}] a failed assertion aborts the rest of the script`, () => {
    const result = runAssertion(
      shell,
      'set -euo pipefail\nsource "$1"\nrackio_assert_eq "guard" "a" "b"\necho unreachable\n',
    );
    assert.equal(result.status, 1);
    assert.doesNotMatch(result.stdout, /unreachable/);
  });

  test(`[${shell}] a failed assertion still runs the exit trap and cleans up`, () => {
    const root = mkdtempSync(join(tmpdir(), "rackio-shell-assert-"));
    const result = runAssertion(
      shell,
      [
        "set -euo pipefail",
        'root="$2"',
        'cleanup() { status="${1:-$?}"; rm -rf "$root"; exit "$status"; }',
        "trap 'cleanup $?' EXIT",
        'source "$1"',
        'rackio_assert_eq "cleanup demonstration" "a" "b"',
        "echo unreachable",
        "",
      ].join("\n"),
      [root],
    );
    assert.equal(result.status, 1);
    assert.match(result.stderr, /assertion failed: cleanup demonstration/);
    assert.doesNotMatch(result.stdout, /unreachable/);
    assert.equal(existsSync(root), false);
  });
}
