import assert from "node:assert/strict";
import { mkdtempSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { resolve, join } from "node:path";
import { spawnSync } from "node:child_process";
import test from "node:test";

const helper = resolve("scripts/reject-matches.sh");

function rejectMatches(...args) {
  return spawnSync(
    "bash",
    [
      "-c",
      'source "$1"; shift; rackio_reject_matches "$@"',
      "rackio-reject-matches-test",
      helper,
      ...args,
    ],
    {
      cwd: process.cwd(),
      encoding: "utf8",
    },
  );
}

test("a clean scan succeeds", () => {
  const directory = mkdtempSync(join(tmpdir(), "rackio-reject-matches-"));
  const fixture = join(directory, "clean.log");
  writeFileSync(fixture, "pairing completed without sensitive fields\n");

  const result = rejectMatches(
    "sensitive content found",
    "fixture scan",
    "grep",
    "-q",
    "-E",
    "one_time_secret",
    fixture,
  );

  assert.equal(result.status, 0);
  assert.equal(result.stderr, "");
});

test("a forbidden match fails and remains observable", () => {
  const directory = mkdtempSync(join(tmpdir(), "rackio-reject-matches-"));
  const fixture = join(directory, "leaked.log");
  writeFileSync(fixture, "one_time_secret=redacted\n");

  const result = rejectMatches(
    "sensitive content found",
    "fixture scan",
    "grep",
    "-q",
    "-E",
    "one_time_secret",
    fixture,
  );

  assert.equal(result.status, 1);
  assert.equal(result.stdout, "");
  assert.match(result.stderr, /sensitive content found/);
  assert.doesNotMatch(result.stderr, /one_time_secret/);
});

test("an unavailable scanner fails closed instead of looking clean", () => {
  const result = rejectMatches(
    "sensitive content found",
    "unavailable fixture scan",
    "rackio-missing-pattern-scanner",
  );

  assert.equal(result.status, 127);
  assert.match(result.stderr, /pattern scan failed before it could prove absence/);
  assert.match(result.stderr, /unavailable fixture scan/);
});

test("an unreadable scan target fails closed", () => {
  const result = rejectMatches(
    "sensitive content found",
    "missing fixture scan",
    "grep",
    "-E",
    "one_time_secret",
    "/definitely/missing/rackio-fixture.log",
  );

  assert.equal(result.status, 2);
  assert.match(result.stderr, /pattern scan failed before it could prove absence/);
  assert.match(result.stderr, /missing fixture scan/);
});
