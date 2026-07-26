#!/usr/bin/env node

import { appendFileSync } from "node:fs";
import { spawnSync } from "node:child_process";
import { fullPlan, planForEvent } from "./ci-plan-lib.mjs";

function changedFiles(baseSha, headSha) {
  if (!baseSha || !headSha || /^0+$/.test(baseSha)) {
    throw new Error("comparison SHAs are unavailable");
  }

  const result = spawnSync(
    "git",
    ["diff", "--name-only", "--diff-filter=ACMR", `${baseSha}...${headSha}`],
    { encoding: "utf8" },
  );
  if (result.status !== 0) {
    throw new Error(result.stderr.trim() || "git diff failed");
  }
  return result.stdout.split("\n").filter(Boolean);
}

const eventName = process.env.CI_EVENT_NAME ?? "";
const eventAction = process.env.CI_EVENT_ACTION ?? "";
let files = [];
let plan;

try {
  if (eventName === "push" || eventName === "pull_request") {
    files = changedFiles(process.env.CI_BASE_SHA, process.env.CI_HEAD_SHA);
  }
  plan = planForEvent({ eventName, eventAction, files });
} catch (error) {
  // A narrow selector must never turn an uncertain change into synthetic success.
  console.error(`::warning::Affected detection failed; running every gate: ${error.message}`);
  plan = fullPlan("selector_error");
}

const output = Object.fromEntries(Object.entries(plan).map(([key, value]) => [key, String(value)]));
if (process.env.GITHUB_OUTPUT) {
  appendFileSync(
    process.env.GITHUB_OUTPUT,
    `${Object.entries(output)
      .map(([key, value]) => `${key}=${value}`)
      .join("\n")}\n`,
  );
}

console.log(JSON.stringify({ ...plan, files }, null, 2));
