import assert from "node:assert/strict";
import { mkdtempSync, readFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { spawnSync } from "node:child_process";
import test from "node:test";
import { classifyChangedFiles, planForEvent } from "./ci-plan-lib.mjs";

test("docs-only updates select no heavy gates", () => {
  assert.deepEqual(
    classifyChangedFiles([
      "README.md",
      ".agents/skills/rackio-oss-governance/SKILL.md",
      "docs/operations.md",
      "packaging/README.md",
      "relay-package/README.md",
    ]),
    {
      full_run: false,
      rust: false,
      rust_linux: false,
      rust_macos: false,
      rust_windows: false,
      frontend: false,
      security_policy: false,
      security_source: false,
      reason: "affected",
    },
  );
});

test("frontend updates do not select the Rust or dependency-policy gates", () => {
  const plan = classifyChangedFiles(["apps/desktop/src/App.tsx"]);
  assert.equal(plan.frontend, true);
  assert.equal(plan.rust, false);
  assert.equal(plan.security_policy, false);
  assert.equal(plan.security_source, true);
});

test("Rust updates select the cross-platform and security gates", () => {
  const plan = classifyChangedFiles(["crates/rackio-iroh/src/transport.rs"]);
  assert.equal(plan.rust, true);
  assert.equal(plan.rust_linux, true);
  assert.equal(plan.rust_macos, true);
  assert.equal(plan.rust_windows, true);
  assert.equal(plan.frontend, false);
  assert.equal(plan.security_policy, true);
  assert.equal(plan.security_source, true);
});

test("platform packaging selects only its owning Rust runner", () => {
  for (const [file, selected] of [
    ["install.sh", "rust_linux"],
    ["packaging/linux/systemd-install.test.sh", "rust_linux"],
    ["packaging/macos/package-release.sh", "rust_macos"],
    ["packaging/windows/install.ps1", "rust_windows"],
  ]) {
    const plan = classifyChangedFiles([file]);
    assert.equal(plan.rust, true, file);
    assert.equal(plan.rust_linux, selected === "rust_linux", file);
    assert.equal(plan.rust_macos, selected === "rust_macos", file);
    assert.equal(plan.rust_windows, selected === "rust_windows", file);
  }
});

test("relay packaging selects only dependency policy", () => {
  const plan = classifyChangedFiles(["relay-package/Dockerfile"]);
  assert.equal(plan.rust, false);
  assert.equal(plan.frontend, false);
  assert.equal(plan.security_policy, true);
});

test("CI routing changes force every gate", () => {
  for (const file of [".github/workflows/ci.yml", "mise.toml"]) {
    const plan = classifyChangedFiles([file]);
    assert.equal(plan.full_run, true);
    assert.equal(plan.rust, true);
    assert.equal(plan.rust_linux, true);
    assert.equal(plan.rust_macos, true);
    assert.equal(plan.rust_windows, true);
    assert.equal(plan.frontend, true);
    assert.equal(plan.security_policy, true);
  }
});

test("Tauri JSON configuration selects both desktop owners", () => {
  const plan = classifyChangedFiles(["apps/desktop/src-tauri/tauri.conf.json"]);
  assert.equal(plan.rust, true);
  assert.equal(plan.frontend, true);
});

test("recognized pull request actions select only affected gates", () => {
  for (const eventAction of ["opened", "ready_for_review", "reopened", "synchronize"]) {
    const plan = planForEvent({
      eventName: "pull_request",
      eventAction,
      files: ["docs/operations.md"],
    });
    assert.equal(plan.full_run, false);
    assert.equal(plan.rust, false);
    assert.equal(plan.frontend, false);
    assert.equal(plan.security_policy, false);
  }
});

test("unknown events fail closed to every gate", () => {
  const plan = planForEvent({ eventName: "workflow_dispatch", eventAction: "" });
  assert.equal(plan.full_run, true);
  assert.equal(plan.reason, "unknown_event");
});

test("unavailable comparison SHAs fail closed in the CLI", () => {
  const directory = mkdtempSync(join(tmpdir(), "rackio-ci-plan-"));
  const outputPath = join(directory, "github-output");
  const result = spawnSync(process.execPath, ["scripts/ci-plan.mjs"], {
    cwd: process.cwd(),
    encoding: "utf8",
    env: {
      ...process.env,
      CI_EVENT_NAME: "push",
      CI_BASE_SHA: "0000000000000000000000000000000000000000",
      CI_HEAD_SHA: "1111111111111111111111111111111111111111",
      GITHUB_OUTPUT: outputPath,
    },
  });

  assert.equal(result.status, 0);
  assert.match(result.stderr, /Affected detection failed; running every gate/);
  assert.match(readFileSync(outputPath, "utf8"), /^full_run=true$/m);
  assert.match(readFileSync(outputPath, "utf8"), /^rust_linux=true$/m);
  assert.match(readFileSync(outputPath, "utf8"), /^rust_macos=true$/m);
  assert.match(readFileSync(outputPath, "utf8"), /^rust_windows=true$/m);
  assert.match(readFileSync(outputPath, "utf8"), /^reason=selector_error$/m);
});
