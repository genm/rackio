import assert from "node:assert/strict";
import test from "node:test";

import { evaluateEnvironment } from "./environment-doctor-lib.mjs";

const requiredChecks = [
  { name: "node", required: true, ok: true, detail: "v24.15.0" },
  { name: "pnpm", required: true, ok: true, detail: "11.17.0" },
  { name: "rust", required: true, ok: true, detail: "rustc 1.97.1" },
  { name: "git_hook", required: true, ok: true, detail: "installed" },
  { name: "playwright_chromium", required: true, ok: true, detail: "installed" },
];

test("reports ready when required and optional checks pass", () => {
  const result = evaluateEnvironment([
    ...requiredChecks,
    { name: "docker", required: false, ok: true, detail: "ready" },
  ]);

  assert.equal(result.status, "ready");
  assert.equal(result.exitCode, 0);
  assert.deepEqual(result.failures, []);
});

test("fails when a required development dependency is missing", () => {
  const checks = requiredChecks.map((check) =>
    check.name === "git_hook" ? { ...check, ok: false, detail: "missing" } : check,
  );
  const result = evaluateEnvironment(checks);

  assert.equal(result.status, "failed");
  assert.equal(result.exitCode, 1);
  assert.deepEqual(result.failures, ["git_hook"]);
});

test("surfaces an unavailable optional relay runtime as degraded", () => {
  const result = evaluateEnvironment([
    ...requiredChecks,
    { name: "docker", required: false, ok: false, detail: "daemon unavailable" },
  ]);

  assert.equal(result.status, "degraded");
  assert.equal(result.exitCode, 0);
  assert.deepEqual(result.degraded, ["docker"]);
});
