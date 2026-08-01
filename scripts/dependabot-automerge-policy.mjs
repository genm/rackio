import { appendFileSync } from "node:fs";
import { pathToFileURL } from "node:url";

const PATCH = "version-update:semver-patch";
const MINOR = "version-update:semver-minor";
const MAJOR = "version-update:semver-major";
const KNOWN_UPDATE_TYPES = new Set([PATCH, MINOR, MAJOR]);

export function evaluateDependabotAutomerge({ updateType, alertState, maintainerChanges }) {
  if (maintainerChanges !== "false") {
    return { eligible: false, reason: "maintainer_changes_or_missing_metadata" };
  }
  if (!KNOWN_UPDATE_TYPES.has(updateType)) {
    return { eligible: false, reason: "unknown_update_type" };
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

function main() {
  const result = evaluateDependabotAutomerge({
    updateType: process.env.DEPENDABOT_UPDATE_TYPE ?? "",
    alertState: process.env.DEPENDABOT_ALERT_STATE ?? "",
    maintainerChanges: process.env.DEPENDABOT_MAINTAINER_CHANGES ?? "",
  });
  const outputPath = process.env.GITHUB_OUTPUT;
  if (!outputPath) {
    throw new Error("GITHUB_OUTPUT is required");
  }
  appendFileSync(
    outputPath,
    `eligible=${String(result.eligible)}\nreason=${result.reason}\n`,
    "utf8",
  );
  process.stdout.write(`${JSON.stringify(result)}\n`);
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  main();
}
