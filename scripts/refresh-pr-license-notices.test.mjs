import assert from "node:assert/strict";
import test from "node:test";
import { readFileSync } from "node:fs";
import { load } from "js-yaml";
import {
  validateTarget,
  noticeEntries,
  dependencyPaths,
  noticeFailure,
} from "./refresh-pr-license-notices.mjs";

const repository = "owner/rackio";
const head = "a".repeat(40);
const pull = {
  state: "open",
  head: { sha: head, ref: "dependabot/cargo/patches", repo: { full_name: repository } },
  base: { ref: "main", repo: { full_name: repository } },
};

test("accepts a same-repository open PR at the generated commit", () => {
  assert.equal(validateTarget(pull, repository, "main", head), head);
});

test("refuses forks, closed PRs, another base, and concurrent head changes", () => {
  for (const changed of [
    { ...pull, state: "closed" },
    { ...pull, head: { ...pull.head, repo: { full_name: "fork/rackio" } } },
    { ...pull, base: { ...pull.base, ref: "release" } },
    { ...pull, base: { ...pull.base, repo: { full_name: "fork/rackio" } } },
    { ...pull, head: { ...pull.head, sha: "b".repeat(40) } },
  ]) {
    assert.throws(() => validateTarget(changed, repository, "main", head));
  }
});

test("publishes exactly the two notice paths as regular UTF-8 files", () => {
  const files = new Map([
    ["THIRDPARTY.html", Buffer.from("Rust notices")],
    ["THIRDPARTY-JAVASCRIPT.html", Buffer.from("JavaScript notices")],
    [".github/workflows/ci.yml", Buffer.from("untrusted")],
  ]);
  assert.deepEqual(
    noticeEntries((path) => files.get(path)),
    [
      { path: "THIRDPARTY.html", mode: "100644", type: "blob", content: "Rust notices" },
      {
        path: "THIRDPARTY-JAVASCRIPT.html",
        mode: "100644",
        type: "blob",
        content: "JavaScript notices",
      },
    ],
  );
});

test("missing, empty, or malformed generated notices fail closed", () => {
  for (const content of [undefined, Buffer.alloc(0), Buffer.from([0xff])]) {
    assert.throws(() => noticeEntries(() => content));
  }
});

test("generation has no write token and publication checks out trusted code", () => {
  const workflow = load(
    readFileSync(
      new URL("../.github/workflows/refresh-license-notices.yml", import.meta.url),
      "utf8",
    ),
  );
  assert.deepEqual(workflow.permissions, { contents: "read", "pull-requests": "read" });
  assert.equal(workflow.jobs.generate.permissions, undefined);
  assert.equal(workflow.jobs.publish.needs, "generate");
  for (const step of [...workflow.jobs.generate.steps, ...workflow.jobs.publish.steps].filter(
    (step) => step.uses?.startsWith("actions/checkout@"),
  )) {
    assert.equal(step.with.ref, "${{ github.sha }}");
    assert.equal(step.with["persist-credentials"], false);
  }
  const upload = workflow.jobs.generate.steps.find((step) =>
    step.uses?.startsWith("actions/upload-artifact@"),
  );
  assert.equal(upload.with["retention-days"], 1);
  assert.equal(upload.with["if-no-files-found"], "error");
});

test("the completed Security run selects without a write token", () => {
  const workflow = load(
    readFileSync(
      new URL("../.github/workflows/refresh-license-notices.yml", import.meta.url),
      "utf8",
    ),
  );
  assert.deepEqual(workflow.on.workflow_run, {
    workflows: ["Security"],
    types: ["completed"],
  });
  assert.ok(workflow.jobs.select);
  for (const permission of Object.values(workflow.jobs.select.permissions)) {
    assert.equal(permission, "read");
  }
  for (const step of workflow.jobs.select.steps.filter((step) =>
    step.uses?.startsWith("actions/checkout@"),
  )) {
    assert.equal(step.with.ref, "${{ github.sha }}");
    assert.equal(step.with["persist-credentials"], false);
  }
  assert.match(workflow.jobs.generate.if, /needs\.select\.outputs\.pr != ''/u);
});

const blockedRun = { event: "pull_request", conclusion: "failure" };
const noticeJob = (step) => ({
  name: "dependency-policy",
  conclusion: "failure",
  steps: [{ name: step, conclusion: "failure" }],
});

test("a failed license notice step selects the run", () => {
  for (const step of [
    "Verify bundled third-party license notices",
    "Verify bundled JavaScript license notices",
  ]) {
    assert.equal(noticeFailure(blockedRun, [noticeJob(step)]), true);
  }
});

test("other failures, events, and conclusions do not select the run", () => {
  assert.equal(noticeFailure(blockedRun, [noticeJob("Compile")]), false);
  assert.equal(
    noticeFailure({ ...blockedRun, conclusion: "success" }, [
      noticeJob("Verify bundled third-party license notices"),
    ]),
    false,
  );
  assert.equal(
    noticeFailure({ ...blockedRun, event: "push" }, [
      noticeJob("Verify bundled third-party license notices"),
    ]),
    false,
  );
  assert.equal(
    noticeFailure(blockedRun, [{ name: "other", conclusion: "failure", steps: [] }]),
    false,
  );
  assert.equal(noticeFailure(blockedRun, null), false);
});

test("only existing dependency manifests and locks cross the execution boundary", () => {
  const tracked = ["Cargo.toml", "Cargo.lock", "apps/desktop/package.json", "pnpm-lock.yaml"];
  assert.deepEqual(dependencyPaths(["Cargo.lock", "THIRDPARTY.html"], tracked), ["Cargo.lock"]);
  for (const path of [
    "scripts/generate-third-party-licenses.sh",
    "rust-toolchain.toml",
    ".cargo/config.toml",
    ".pnpmfile.cjs",
    "pnpm-workspace.yaml",
    "new/package.json",
  ]) {
    assert.throws(() => dependencyPaths([path], tracked));
  }
});
