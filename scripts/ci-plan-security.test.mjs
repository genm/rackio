import assert from "node:assert/strict";
import test from "node:test";
import { classifyChangedFiles, planForEvent } from "./ci-plan-lib.mjs";

for (const file of [
  "fuzz/Cargo.toml",
  "fuzz/Cargo.lock",
  "fuzz/fuzz_targets/pairing_bundle.rs",
]) {
  test(`${file} selects dependency policy and keeps Linux-only Rust coverage`, () => {
    const plan = classifyChangedFiles([file]);
    assert.equal(plan.full_run, false);
    assert.equal(plan.security_policy, true);
    assert.equal(plan.rust, true);
    assert.equal(plan.rust_linux, true);
    assert.equal(plan.rust_macos, false);
    assert.equal(plan.rust_windows, false);
    assert.equal(plan.codeql_rust, true);
    assert.equal(plan.codeql_javascript, false);
    assert.equal(plan.codeql_actions, false);
    assert.equal(plan.frontend, false);
  });
}

test("fuzz-only dependency updates select Security on every supported PR transition and push", () => {
  for (const event of [
    { eventName: "pull_request", eventAction: "opened" },
    { eventName: "pull_request", eventAction: "reopened" },
    { eventName: "pull_request", eventAction: "synchronize" },
    { eventName: "pull_request", eventAction: "ready_for_review" },
    { eventName: "push", eventAction: "" },
  ]) {
    const plan = planForEvent({ ...event, files: ["fuzz/Cargo.lock"] });
    assert.equal(plan.security_policy, true, JSON.stringify(event));
    assert.equal(plan.codeql_rust, true, JSON.stringify(event));
  }
});

test("corpus-only input does not select dependency policy or CodeQL", () => {
  const plan = classifyChangedFiles(["fuzz/corpus/pairing_bundle/crash-0000"]);
  assert.equal(plan.security_policy, false);
  assert.equal(plan.rust, false);
  assert.equal(plan.codeql_rust, false);
});

test("a corpus change cannot hide a simultaneous fuzz dependency change", () => {
  const plan = classifyChangedFiles([
    "fuzz/corpus/pairing_bundle/crash-0000",
    "fuzz/Cargo.lock",
  ]);
  assert.equal(plan.security_policy, true);
  assert.equal(plan.codeql_rust, true);
});

test("the security-routing regression suite itself selects every gate", () => {
  const plan = classifyChangedFiles(["scripts/ci-plan-security.test.mjs"]);
  assert.equal(plan.full_run, true);
  assert.equal(plan.security_policy, true);
  assert.equal(plan.codeql_actions, true);
  assert.equal(plan.codeql_javascript, true);
  assert.equal(plan.codeql_rust, true);
});
