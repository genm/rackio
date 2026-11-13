import { readFileSync, readdirSync } from "node:fs";
import { resolve } from "node:path";
import {
  GovernanceValidationError,
  parseYaml,
  validateGovernance,
} from "./governance-check-lib.mjs";

const root = resolve(import.meta.dirname, "..");
const issueTemplateDirectory = resolve(root, ".github/ISSUE_TEMPLATE");

try {
  const labels = JSON.parse(readFileSync(resolve(root, ".github/labels.json"), "utf8"));
  const issueForms = new Map();
  let config;
  for (const filename of readdirSync(issueTemplateDirectory)
    .filter((name) => name.endsWith(".yml"))
    .sort()) {
    const path = `.github/ISSUE_TEMPLATE/${filename}`;
    const document = parseYaml(readFileSync(resolve(root, path), "utf8"), path);
    if (filename === "config.yml") {
      config = document;
    } else {
      issueForms.set(path, document);
    }
  }
  if (!config) {
    throw new GovernanceValidationError([".github/ISSUE_TEMPLATE/config.yml: file is required"]);
  }

  const result = validateGovernance({
    labels,
    issueForms,
    config,
    backlog: readFileSync(resolve(root, "docs/backlog.md"), "utf8"),
    codeowners: readFileSync(resolve(root, ".github/CODEOWNERS"), "utf8"),
  });
  process.stdout.write(`${JSON.stringify({ status: "pass", ...result })}\n`);
} catch (error) {
  const errors = error instanceof GovernanceValidationError ? error.errors : [error.message];
  process.stderr.write(`${JSON.stringify({ status: "fail", errors })}\n`);
  process.exitCode = 1;
}
