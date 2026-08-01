import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import test from "node:test";
import { load } from "js-yaml";
import { evaluateDependabotAutomerge } from "./dependabot-automerge-policy.mjs";

test("security patch and minor updates are eligible", () => {
  for (const updateType of ["version-update:semver-patch", "version-update:semver-minor"]) {
    assert.deepEqual(
      evaluateDependabotAutomerge({
        updateType,
        alertState: "OPEN",
        maintainerChanges: "false",
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
    }),
    { eligible: true, reason: "routine_patch" },
  );
});

test("major and routine minor updates stay manual", () => {
  for (const input of [
    {
      updateType: "version-update:semver-major",
      alertState: "OPEN",
      maintainerChanges: "false",
    },
    {
      updateType: "version-update:semver-minor",
      alertState: "",
      maintainerChanges: "false",
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
    },
    {
      updateType: "version-update:semver-patch",
      alertState: "FIXED",
      maintainerChanges: "false",
    },
    {
      updateType: "version-update:semver-patch",
      alertState: "DISMISSED",
      maintainerChanges: "false",
    },
  ]) {
    assert.equal(evaluateDependabotAutomerge(input).eligible, false);
  }
});

test("missing or unknown metadata fails closed", () => {
  for (const input of [
    { updateType: "", alertState: "", maintainerChanges: "false" },
    { updateType: "unexpected", alertState: "", maintainerChanges: "false" },
    {
      updateType: "version-update:semver-patch",
      alertState: "UNKNOWN",
      maintainerChanges: "false",
    },
    {
      updateType: "version-update:semver-patch",
      alertState: "",
      maintainerChanges: "",
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

  assert.deepEqual(workflow.permissions, {
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

  const merge = job.steps.find((step) => step.name === "Enable native auto-merge");
  assert.equal(merge.if, "steps.policy.outputs.eligible == 'true'");
  assert.match(merge.run, /gh pr merge --auto --merge "\$PR_URL"/);
});
