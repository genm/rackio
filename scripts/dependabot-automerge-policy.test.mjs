import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { mkdtempSync, readFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import test from "node:test";
import { load } from "js-yaml";
import {
  evaluateDependabotAutomerge,
  preflightDependabotCommits,
} from "./dependabot-automerge-policy.mjs";

const HEAD = "5f8cf1679ea5a0912b58e1d564bf2eff6f22f1d4";
const EARLIER = "b2a150bf04e11ecba6739639c17e7daa73eabb8a";
// The shape GitHub reports for an untouched Dependabot commit (e.g. PR #189).
const UNTOUCHED = { sha: HEAD, author: "dependabot[bot]", committer: "web-flow" };

const SAFE_METADATA = {
  packageEcosystem: "npm_and_yarn",
  dependencyNames: "oxlint",
};

test("security patch and minor updates are eligible", () => {
  for (const updateType of ["version-update:semver-patch", "version-update:semver-minor"]) {
    assert.deepEqual(
      evaluateDependabotAutomerge({
        updateType,
        alertState: "OPEN",
        maintainerChanges: "false",
        ...SAFE_METADATA,
      }),
      { eligible: true, reason: "security_update" },
    );
  }
});

test("routine patch updates are eligible", () => {
  assert.deepEqual(
    evaluateDependabotAutomerge({
      updateType: "version-update:semver-patch",
      alertState: "",
      maintainerChanges: "false",
      packageEcosystem: "npm_and_yarn",
      dependencyNames: "oxlint",
    }),
    { eligible: true, reason: "routine_patch" },
  );
});

test("privileged ecosystems and critical product dependencies stay manual", () => {
  for (const input of [
    {
      packageEcosystem: "github_actions",
      dependencyNames: "actions/checkout",
    },
    {
      packageEcosystem: "docker",
      dependencyNames: "rust",
    },
    {
      packageEcosystem: "cargo",
      dependencyNames: "iroh",
    },
    {
      packageEcosystem: "cargo",
      dependencyNames: "serde, tauri-plugin-dialog",
    },
  ]) {
    assert.deepEqual(
      evaluateDependabotAutomerge({
        updateType: "version-update:semver-patch",
        alertState: "OPEN",
        maintainerChanges: "false",
        ...input,
      }),
      { eligible: false, reason: "privileged_or_product_critical_update" },
    );
  }
});

test("missing or unknown ecosystem and dependency metadata fails closed", () => {
  for (const input of [
    { packageEcosystem: "", dependencyNames: "serde" },
    { packageEcosystem: "unknown", dependencyNames: "serde" },
    { packageEcosystem: "cargo", dependencyNames: "" },
  ]) {
    assert.equal(
      evaluateDependabotAutomerge({
        updateType: "version-update:semver-patch",
        alertState: "",
        maintainerChanges: "false",
        ...input,
      }).eligible,
      false,
    );
  }
});

test("major and routine minor updates stay manual", () => {
  for (const input of [
    {
      updateType: "version-update:semver-major",
      alertState: "OPEN",
      maintainerChanges: "false",
      ...SAFE_METADATA,
    },
    {
      updateType: "version-update:semver-minor",
      alertState: "",
      maintainerChanges: "false",
      ...SAFE_METADATA,
    },
  ]) {
    assert.equal(evaluateDependabotAutomerge(input).eligible, false);
  }
});

test("maintainer changes and non-open alerts fail closed", () => {
  for (const input of [
    {
      updateType: "version-update:semver-patch",
      alertState: "",
      maintainerChanges: "true",
      ...SAFE_METADATA,
    },
    {
      updateType: "version-update:semver-patch",
      alertState: "FIXED",
      maintainerChanges: "false",
      ...SAFE_METADATA,
    },
    {
      updateType: "version-update:semver-patch",
      alertState: "DISMISSED",
      maintainerChanges: "false",
      ...SAFE_METADATA,
    },
  ]) {
    assert.equal(evaluateDependabotAutomerge(input).eligible, false);
  }
});

test("missing or unknown metadata fails closed", () => {
  for (const input of [
    { updateType: "", alertState: "", maintainerChanges: "false", ...SAFE_METADATA },
    { updateType: "unexpected", alertState: "", maintainerChanges: "false", ...SAFE_METADATA },
    {
      updateType: "version-update:semver-patch",
      alertState: "UNKNOWN",
      maintainerChanges: "false",
      ...SAFE_METADATA,
    },
    {
      updateType: "version-update:semver-patch",
      alertState: "",
      maintainerChanges: "",
      ...SAFE_METADATA,
    },
  ]) {
    assert.equal(evaluateDependabotAutomerge(input).eligible, false);
  }
});

test("an untouched Dependabot pull request proceeds to verified metadata", () => {
  assert.deepEqual(preflightDependabotCommits({ commits: [UNTOUCHED], headSha: HEAD }), {
    proceed: true,
    reason: "untouched_dependabot_commits",
  });
});

test("maintainer-modified pull requests stop as manual review, not as errors", () => {
  for (const commits of [
    // PR #176: the Dependabot commit rebased by a maintainer, then more commits.
    [
      { sha: EARLIER, author: "dependabot[bot]", committer: "genm" },
      { sha: HEAD, author: "genm", committer: "genm" },
    ],
    // A bot-authored follow-up such as the license-notice refresh.
    [
      { ...UNTOUCHED, sha: EARLIER },
      { sha: HEAD, author: "github-actions[bot]", committer: "web-flow" },
    ],
    // An author GitHub cannot map to an account reports no login at all.
    [{ sha: HEAD, author: null, committer: "web-flow" }],
  ]) {
    assert.deepEqual(preflightDependabotCommits({ commits, headSha: HEAD }), {
      proceed: false,
      eligible: false,
      reason: "maintainer_modified",
    });
  }
});

test("a head that moved after the event defers to the newer run", () => {
  assert.deepEqual(preflightDependabotCommits({ commits: [UNTOUCHED], headSha: EARLIER }), {
    proceed: false,
    eligible: false,
    reason: "head_changed",
  });
});

test("an unusable commit listing is an error, not a manual-review outcome", () => {
  for (const [input, message] of [
    [{ commits: [], headSha: HEAD }, /empty/],
    [{ commits: undefined, headSha: HEAD }, /empty/],
    [
      { commits: [{ author: "dependabot[bot]", committer: "web-flow" }], headSha: HEAD },
      /malformed/,
    ],
    [{ commits: [UNTOUCHED], headSha: "" }, /head SHA is unavailable/],
  ]) {
    assert.throws(() => preflightDependabotCommits(input), message);
  }
});

test("the preflight CLI reports outputs and a summary, and fails on bad input", () => {
  const directory = mkdtempSync(join(tmpdir(), "rackio-dependabot-preflight-"));
  const outputPath = join(directory, "output");
  const summaryPath = join(directory, "summary");
  const run = (stdin) =>
    spawnSync(process.execPath, ["scripts/dependabot-automerge-policy.mjs", "preflight"], {
      input: stdin,
      encoding: "utf8",
      env: {
        ...process.env,
        GITHUB_OUTPUT: outputPath,
        GITHUB_STEP_SUMMARY: summaryPath,
        PR_HEAD_SHA: HEAD,
      },
    });

  let result = run(`${JSON.stringify({ sha: HEAD, author: "genm", committer: "genm" })}\n`);
  assert.equal(result.status, 0, result.stderr);
  assert.match(readFileSync(outputPath, "utf8"), /^proceed=false$/m);
  assert.match(readFileSync(outputPath, "utf8"), /^eligible=false$/m);
  assert.match(readFileSync(outputPath, "utf8"), /^reason=maintainer_modified$/m);
  assert.match(readFileSync(summaryPath, "utf8"), /eligible=false: maintainer_modified/);

  result = run("");
  assert.notEqual(result.status, 0);
  assert.match(result.stderr, /commit listing is empty/);

  result = run("not json\n");
  assert.notEqual(result.status, 0);
});

test("workflow keeps the privileged boundary narrow and immutable", () => {
  const path = resolve(".github/workflows/dependabot-automerge.yml");
  const source = readFileSync(path, "utf8");
  const workflow = load(source);
  const job = workflow.jobs.dependabot;

  assert.deepEqual(workflow.permissions, {});
  assert.deepEqual(job.permissions, {
    contents: "write",
    "pull-requests": "write",
    "security-events": "read",
  });
  assert.deepEqual(workflow.on.pull_request.types, [
    "opened",
    "reopened",
    "synchronize",
    "ready_for_review",
  ]);
  for (const boundary of [
    "github.event.pull_request.user.login == 'dependabot[bot]'",
    "github.repository == 'genm/rackio'",
    "github.event.pull_request.base.ref == 'main'",
    "github.event.pull_request.head.repo.full_name == github.repository",
    "github.event.pull_request.draft == false",
  ]) {
    assert.match(job.if, new RegExp(boundary.replace(/[.*+?^${}()|[\]\\]/g, "\\$&")));
  }
  assert.doesNotMatch(source, /pull_request_target/);

  const checkout = job.steps.find((step) => step.name === "Check out trusted policy");
  assert.equal(checkout.uses, "actions/checkout@3d3c42e5aac5ba805825da76410c181273ba90b1");
  assert.equal(checkout.with.ref, "${{ github.event.pull_request.base.sha }}");
  assert.equal(checkout.with["persist-credentials"], false);

  // The preflight is read-only and ahead of verification; it may only gate the
  // verified path off, never replace it.
  const stepIds = job.steps.map((step) => step.id ?? step.name);
  const preflight = job.steps.find((step) => step.id === "preflight");
  assert.ok(stepIds.indexOf("preflight") < stepIds.indexOf("metadata"));
  assert.equal(preflight.uses, undefined);
  assert.match(
    preflight.run,
    /gh api --paginate "repos\/\$\{GITHUB_REPOSITORY\}\/pulls\/\$\{PR_NUMBER\}\/commits"/,
  );
  assert.match(preflight.run, /node scripts\/dependabot-automerge-policy\.mjs preflight/);
  assert.equal(preflight.env.PR_HEAD_SHA, "${{ github.event.pull_request.head.sha }}");
  assert.equal(preflight["continue-on-error"], undefined);

  const metadata = job.steps.find((step) => step.id === "metadata");
  assert.equal(metadata.uses, "dependabot/fetch-metadata@25dd0e34f4fe68f24cc83900b1fe3fe149efef98");
  assert.equal(metadata.with["alert-lookup"], true);
  assert.equal(metadata.if, "steps.preflight.outputs.proceed == 'true'");
  // Verification is never switched off to make the manual path quiet.
  for (const key of ["skip-verification", "skip-commit-verification"]) {
    assert.equal(metadata.with[key], undefined, key);
  }
  for (const step of job.steps) {
    assert.equal(step["continue-on-error"], undefined, step.name);
  }

  const policy = job.steps.find((step) => step.id === "policy");
  assert.equal(policy.if, "steps.preflight.outputs.proceed == 'true'");
  assert.equal(
    policy.env.DEPENDABOT_PACKAGE_ECOSYSTEM,
    "${{ steps.metadata.outputs.package-ecosystem }}",
  );
  assert.equal(
    policy.env.DEPENDABOT_DEPENDENCY_NAMES,
    "${{ steps.metadata.outputs.dependency-names }}",
  );

  const merge = job.steps.find((step) => step.name === "Enable native auto-merge");
  assert.equal(merge.if, "steps.policy.outputs.eligible == 'true'");
  assert.match(
    merge.run,
    /gh pr merge --auto --merge --match-head-commit "\$PR_HEAD_SHA" "\$PR_URL"/,
  );
  assert.equal(merge.env.PR_HEAD_SHA, "${{ github.event.pull_request.head.sha }}");
});
