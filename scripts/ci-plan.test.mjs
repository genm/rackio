import assert from "node:assert/strict";
import { mkdirSync, mkdtempSync, readFileSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { spawnSync } from "node:child_process";
import test from "node:test";
import { classifyChangedFiles, fullPlan, parseChangedFiles, planForEvent } from "./ci-plan-lib.mjs";

const plannerPath = resolve("scripts/ci-plan.mjs");

function git(directory, ...args) {
  const result = spawnSync("git", args, {
    cwd: directory,
    encoding: "utf8",
  });
  assert.equal(result.status, 0, result.stderr);
  return result.stdout.trim();
}

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
      codeql_actions: false,
      codeql_javascript: false,
      codeql_rust: false,
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

test("Windows-only IPC crate changes select the Windows cross-compile check", () => {
  const plan = classifyChangedFiles(["crates/rackio-windows-ipc/src/lib.rs"]);
  assert.equal(plan.rust, true);
  assert.equal(plan.rust_windows, true);
  assert.equal(plan.rust_linux, true);
  assert.equal(plan.rust_macos, true);
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

test("fuzz target changes select only the Linux Rust runner", () => {
  const plan = classifyChangedFiles(["fuzz/fuzz_targets/pairing_bundle.rs"]);
  assert.equal(plan.rust, true);
  assert.equal(plan.rust_linux, true);
  assert.equal(plan.rust_macos, false);
  assert.equal(plan.rust_windows, false);
});

test("fuzz corpus additions select no build gate", () => {
  // A corpus file is fuzzer input, never compiled, so a new reproducer must not
  // charge a full Rust matrix to the pull request that records it.
  const plan = classifyChangedFiles(["fuzz/corpus/pairing_bundle/crash-0000"]);
  assert.equal(plan.rust, false);
  assert.equal(plan.frontend, false);
  assert.equal(plan.security_policy, false);
});

test("relay packaging selects only dependency policy", () => {
  const plan = classifyChangedFiles(["relay-package/Dockerfile"]);
  assert.equal(plan.rust, false);
  assert.equal(plan.frontend, false);
  assert.equal(plan.security_policy, true);
});

test("CI routing changes force every gate", () => {
  for (const file of [
    ".github/workflows/ci.yml",
    "mise.toml",
    "scripts/reject-matches.sh",
    "scripts/reject-matches.test.mjs",
  ]) {
    const plan = classifyChangedFiles([file]);
    assert.equal(plan.full_run, true);
    assert.equal(plan.rust, true);
    assert.equal(plan.rust_linux, true);
    assert.equal(plan.rust_macos, true);
    assert.equal(plan.rust_windows, true);
    assert.equal(plan.frontend, true);
    assert.equal(plan.security_policy, true);
    assert.equal(plan.codeql_actions, true);
    assert.equal(plan.codeql_javascript, true);
    assert.equal(plan.codeql_rust, true);
  }
});

test("CodeQL languages are selected only by their own source", () => {
  const rustOnly = classifyChangedFiles(["crates/rackio-iroh/src/transport.rs"]);
  assert.equal(rustOnly.codeql_rust, true);
  assert.equal(rustOnly.codeql_javascript, false);
  assert.equal(rustOnly.codeql_actions, false);

  const frontendOnly = classifyChangedFiles(["apps/desktop/src/App.tsx"]);
  assert.equal(frontendOnly.codeql_javascript, true);
  assert.equal(frontendOnly.codeql_rust, false);
  assert.equal(frontendOnly.codeql_actions, false);
});

test("shell and packaging changes build no CodeQL database", () => {
  // These select the Rust gate because they gate a Rust runner's platform
  // steps, but they hold no Rust, TypeScript or workflow source, so charging a
  // whole-program CodeQL database to them buys nothing.
  for (const file of [
    "install.sh",
    "packaging/linux/systemd-install.test.sh",
    "packaging/windows/install.ps1",
  ]) {
    const plan = classifyChangedFiles([file]);
    assert.equal(plan.rust, true, file);
    assert.equal(plan.codeql_rust, false, file);
    assert.equal(plan.codeql_javascript, false, file);
    assert.equal(plan.codeql_actions, false, file);
  }
});

test("documentation changes build no CodeQL database", () => {
  const plan = classifyChangedFiles(["README.md", "docs/operations.md"]);
  assert.equal(plan.codeql_actions, false);
  assert.equal(plan.codeql_javascript, false);
  assert.equal(plan.codeql_rust, false);
});

test("Dependabot configuration selects the Actions CodeQL language", () => {
  // .github/dependabot.yml is not a workflow, so it escapes the global CI
  // routing rule, but it is still Actions-owned configuration that the
  // `actions` query pack reads.
  const plan = classifyChangedFiles([".github/dependabot.yml"]);
  assert.equal(plan.full_run, false);
  assert.equal(plan.codeql_actions, true);
  assert.equal(plan.codeql_rust, false);
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

test("workflow wiring compares every pull request from its protected base", () => {
  const pullRequestBase =
    "github.event_name == 'pull_request' && github.event.pull_request.base.sha || github.event.before";
  const ciWorkflow = readFileSync(resolve(".github/workflows/ci.yml"), "utf8");
  const securityWorkflow = readFileSync(resolve(".github/workflows/security.yml"), "utf8");
  const codeqlWorkflow = readFileSync(resolve(".github/workflows/codeql.yml"), "utf8");

  assert.equal(ciWorkflow.split(pullRequestBase).length - 1, 1);
  assert.equal(securityWorkflow.split(pullRequestBase).length - 1, 2);
  assert.equal(codeqlWorkflow.split(pullRequestBase).length - 1, 1);
  assert.doesNotMatch(
    `${ciWorkflow}\n${securityWorkflow}\n${codeqlWorkflow}`,
    /github\.event\.before \|\| github\.event\.pull_request\.base\.sha/,
  );
});

test("CodeQL analysis never uploads for an unaffected language", () => {
  // Uploading an empty or partial SARIF for a language nobody scanned would
  // resolve that language's live alerts as fixed. Every CodeQL step must
  // therefore carry the same gate as the checkout it depends on.
  const codeqlWorkflow = readFileSync(resolve(".github/workflows/codeql.yml"), "utf8");
  const codeqlSteps = codeqlWorkflow
    .split("\n")
    .reduce((steps, line) => {
      if (/^ {6}- name: /.test(line)) {
        steps.push([]);
      }
      steps.at(-1)?.push(line);
      return steps;
    }, [])
    .map((lines) => lines.join("\n"));

  const guarded = codeqlSteps.filter((step) => step.includes("github/codeql-action/"));
  assert.equal(guarded.length, 2);
  for (const step of guarded) {
    assert.match(step, /if: env\.RUN_CODEQL == 'true'/);
  }
});

test("every CodeQL language gate is a statically checkable planner output", () => {
  // A dynamic `needs.plan.outputs[matrix.gate]` lookup is invisible to
  // actionlint and resolves to an empty string when it is wrong, which
  // `!= 'false'` reads as "run" — so a typo would scan every language forever
  // and still look healthy. Static references are validated before the workflow
  // runs, and this pins them to the planner keys that actually exist.
  const codeqlWorkflow = readFileSync(resolve(".github/workflows/codeql.yml"), "utf8");
  const plannerKeys = Object.keys(fullPlan("test")).filter((key) => key.startsWith("codeql_"));

  assert.deepEqual(plannerKeys.sort(), ["codeql_actions", "codeql_javascript", "codeql_rust"]);
  assert.doesNotMatch(codeqlWorkflow, /needs\.plan\.outputs\[/);
  for (const key of plannerKeys) {
    // Declared as a job output, and consumed as the matrix gate.
    assert.match(
      codeqlWorkflow,
      new RegExp(`^ {6}${key}: \\$\\{\\{ steps\\.plan\\.outputs\\.${key} \\}\\}$`, "m"),
    );
    assert.match(
      codeqlWorkflow,
      new RegExp(`^ {12}gate: \\$\\{\\{ needs\\.plan\\.outputs\\.${key} \\}\\}$`, "m"),
    );
  }
});

test("NUL-delimited paths preserve newlines and reject malformed diff output", () => {
  assert.deepEqual(parseChangedFiles(Buffer.from("docs/line\nbreak.md\0deny.toml\0")), [
    "docs/line\nbreak.md",
    "deny.toml",
  ]);
  assert.throws(() => parseChangedFiles(Buffer.from("deny.toml")), /NUL terminator/);
  assert.throws(() => parseChangedFiles(Buffer.from([0xff, 0x00])), /encoded data/);
});

test("deletions and rename sources still select their original owners", () => {
  const directory = mkdtempSync(join(tmpdir(), "rackio-ci-plan-git-"));
  const outputPath = join(directory, "github-output");
  mkdirSync(join(directory, "crates"), { recursive: true });
  mkdirSync(join(directory, "docs"), { recursive: true });
  git(directory, "init", "--quiet");
  git(directory, "config", "user.name", "Rackio CI");
  git(directory, "config", "user.email", "ci@example.test");
  writeFileSync(join(directory, "deny.toml"), "[advisories]\n");
  writeFileSync(join(directory, "crates", "guard.rs"), "pub fn guarded() {}\n");
  git(directory, "add", "--all");
  git(directory, "commit", "--quiet", "-m", "test: add owned fixtures");
  const baseSha = git(directory, "rev-parse", "HEAD");

  git(directory, "rm", "--quiet", "deny.toml");
  git(directory, "mv", "crates/guard.rs", "docs/guard.rs");
  writeFileSync(join(directory, "docs", "line\nbreak.md"), "special path\n");
  git(directory, "add", "--all");
  git(directory, "commit", "--quiet", "-m", "test: remove owned fixtures");
  const headSha = git(directory, "rev-parse", "HEAD");

  const result = spawnSync(process.execPath, [plannerPath], {
    cwd: directory,
    encoding: "utf8",
    env: {
      ...process.env,
      CI_EVENT_NAME: "pull_request",
      CI_EVENT_ACTION: "synchronize",
      CI_BASE_SHA: baseSha,
      CI_HEAD_SHA: headSha,
      GITHUB_OUTPUT: outputPath,
    },
  });

  assert.equal(result.status, 0, result.stderr);
  const plan = JSON.parse(result.stdout);
  assert.equal(plan.full_run, false);
  assert.equal(plan.rust, true);
  assert.equal(plan.security_policy, true);
  assert.equal(plan.security_source, true);
  assert.ok(plan.files.includes("deny.toml"));
  assert.ok(plan.files.includes("crates/guard.rs"));
  assert.ok(plan.files.includes("docs/guard.rs"));
  assert.ok(plan.files.includes("docs/line\nbreak.md"));
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
