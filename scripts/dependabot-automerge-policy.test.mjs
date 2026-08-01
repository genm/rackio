import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import test from "node:test";
import { load } from "js-yaml";
import { evaluateDependabotAutomerge } from "./dependabot-automerge-policy.mjs";

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

  const metadata = job.steps.find((step) => step.id === "metadata");
  assert.equal(metadata.uses, "dependabot/fetch-metadata@25dd0e34f4fe68f24cc83900b1fe3fe149efef98");
  assert.equal(metadata.with["alert-lookup"], true);

  const policy = job.steps.find((step) => step.id === "policy");
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
  assert.match(merge.run, /gh pr merge --auto --merge "\$PR_URL"/);
});
