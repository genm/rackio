import { appendFileSync, readFileSync } from "node:fs";
import { pathToFileURL } from "node:url";

const PATCH = "version-update:semver-patch";
const MINOR = "version-update:semver-minor";
const MAJOR = "version-update:semver-major";
const KNOWN_UPDATE_TYPES = new Set([PATCH, MINOR, MAJOR]);
const AUTO_MERGE_ECOSYSTEMS = new Set(["cargo", "npm_and_yarn"]);
const MANUAL_REVIEW_ECOSYSTEMS = new Set(["docker", "github_actions"]);
const DEPENDABOT_LOGIN = "dependabot[bot]";
// Dependabot creates its commits through the API, so GitHub's `web-flow`
// commits and signs them. A rebase, amend or push by anyone else replaces it.
const DEPENDABOT_COMMITTER = "web-flow";
const COMMIT_SHA = /^[0-9a-f]{40}$/;

function isProductCriticalDependency(name) {
  return (
    name === "iroh" ||
    name.startsWith("iroh-") ||
    name === "swarm-discovery" ||
    name === "tauri" ||
    name.startsWith("tauri-") ||
    name.startsWith("@tauri-apps/") ||
    name === "windows-sys"
  );
}

export function evaluateDependabotAutomerge({
  updateType,
  alertState,
  maintainerChanges,
  packageEcosystem,
  dependencyNames,
}) {
  if (maintainerChanges !== "false") {
    return { eligible: false, reason: "maintainer_changes_or_missing_metadata" };
  }
  if (!KNOWN_UPDATE_TYPES.has(updateType)) {
    return { eligible: false, reason: "unknown_update_type" };
  }
  const dependencies = dependencyNames
    .split(",")
    .map((name) => name.trim())
    .filter(Boolean);
  if (
    dependencies.length === 0 ||
    (!AUTO_MERGE_ECOSYSTEMS.has(packageEcosystem) &&
      !MANUAL_REVIEW_ECOSYSTEMS.has(packageEcosystem))
  ) {
    return { eligible: false, reason: "unknown_ecosystem_or_dependency_metadata" };
  }
  if (
    MANUAL_REVIEW_ECOSYSTEMS.has(packageEcosystem) ||
    dependencies.some(isProductCriticalDependency)
  ) {
    return { eligible: false, reason: "privileged_or_product_critical_update" };
  }
  if (alertState !== "" && alertState !== "OPEN") {
    return { eligible: false, reason: "security_alert_not_open" };
  }
  if (alertState === "OPEN") {
    if (updateType === PATCH || updateType === MINOR) {
      return { eligible: true, reason: "security_update" };
    }
    return { eligible: false, reason: "security_major_requires_review" };
  }
  if (updateType === PATCH) {
    return { eligible: true, reason: "routine_patch" };
  }
  return { eligible: false, reason: "routine_non_patch_requires_review" };
}

// dependabot/fetch-metadata only reports on a pull request whose commits are all
// Dependabot's own, and fails the job on anything else. A maintainer-modified
// update is an ordinary manual review, not a broken workflow, so it is
// recognised here first. This can only deny eligibility: an untouched pull
// request still has to pass fetch-metadata's identity and signature checks and
// the bounded update policy before anything is merged.
export function preflightDependabotCommits({ commits, headSha }) {
  if (!Array.isArray(commits) || commits.length === 0) {
    throw new Error("the pull request commit listing is empty");
  }
  for (const commit of commits) {
    if (typeof commit?.sha !== "string" || !COMMIT_SHA.test(commit.sha)) {
      throw new Error(`the pull request commit listing is malformed: ${JSON.stringify(commit)}`);
    }
  }
  if (!COMMIT_SHA.test(headSha ?? "")) {
    throw new Error("the event's pull request head SHA is unavailable");
  }
  if (
    commits.some(
      (commit) => commit.author !== DEPENDABOT_LOGIN || commit.committer !== DEPENDABOT_COMMITTER,
    )
  ) {
    return { proceed: false, eligible: false, reason: "maintainer_modified" };
  }
  // The listing is read after the event fired. A different tip means a newer
  // push, whose own run supersedes this one through the concurrency group.
  if (commits.at(-1).sha !== headSha) {
    return { proceed: false, eligible: false, reason: "head_changed" };
  }
  return { proceed: true, reason: "untouched_dependabot_commits" };
}

export function parseCommitListing(ndjson) {
  return ndjson
    .split("\n")
    .filter((line) => line.trim() !== "")
    .map((line) => JSON.parse(line));
}

function writeOutputs(result) {
  const outputPath = process.env.GITHUB_OUTPUT;
  if (!outputPath) {
    throw new Error("GITHUB_OUTPUT is required");
  }
  appendFileSync(
    outputPath,
    `${Object.entries(result)
      .map(([key, value]) => `${key}=${String(value)}`)
      .join("\n")}\n`,
    "utf8",
  );
  if (process.env.GITHUB_STEP_SUMMARY && result.eligible === false) {
    appendFileSync(
      process.env.GITHUB_STEP_SUMMARY,
      `Dependabot auto-merge: \`eligible=false: ${result.reason}\`. Left for manual review.\n`,
      "utf8",
    );
  }
  process.stdout.write(`${JSON.stringify(result)}\n`);
}

function preflightMain() {
  writeOutputs(
    preflightDependabotCommits({
      commits: parseCommitListing(readFileSync(0, "utf8")),
      headSha: process.env.PR_HEAD_SHA ?? "",
    }),
  );
}

function main() {
  const result = evaluateDependabotAutomerge({
    updateType: process.env.DEPENDABOT_UPDATE_TYPE ?? "",
    alertState: process.env.DEPENDABOT_ALERT_STATE ?? "",
    maintainerChanges: process.env.DEPENDABOT_MAINTAINER_CHANGES ?? "",
    packageEcosystem: process.env.DEPENDABOT_PACKAGE_ECOSYSTEM ?? "",
    dependencyNames: process.env.DEPENDABOT_DEPENDENCY_NAMES ?? "",
  });
  writeOutputs(result);
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  if (process.argv[2] === "preflight") {
    preflightMain();
  } else if (process.argv[2] === undefined) {
    main();
  } else {
    throw new Error(`unknown mode: ${process.argv[2]}`);
  }
}
