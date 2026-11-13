import assert from "node:assert/strict";
import test from "node:test";

import { evaluateEnvironment, resolvedLockfileDocument } from "./environment-doctor-lib.mjs";

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

test("resolvedLockfileDocument passes a single-document lockfile through", () => {
  const lockfile = "lockfileVersion: '9.0'\n\nimporters:\n\n  .:\n    dependencies: {}\n";

  assert.equal(resolvedLockfileDocument(lockfile), lockfile);
});

test("resolvedLockfileDocument returns the resolved lockfile from a pnpm 12 lockfile", () => {
  const resolved = "lockfileVersion: '9.0'\n\nsettings:\n  autoInstallPeers: true\n";
  const lockfile =
    "---\npackageManagerDependencies:\n  pnpm:\n    version: 12.5.1\n\n---\n" + resolved;

  assert.equal(resolvedLockfileDocument(lockfile), resolved);
});
