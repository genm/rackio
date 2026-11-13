import assert from "node:assert/strict";
import test from "node:test";
import {
  GovernanceValidationError,
  parseYaml,
  validateGovernance,
} from "./governance-check-lib.mjs";

function validInput() {
  return {
    labels: [{ name: "type:bug", color: "d73a4a", description: "A bug" }],
    issueForms: new Map([
      [
        ".github/ISSUE_TEMPLATE/bug.yml",
        {
          name: "Bug",
          description: "Report a bug",
          title: "[Bug] ",
          labels: ["type:bug"],
          body: [{ type: "textarea", id: "summary" }],
        },
      ],
    ]),
    config: {
      blank_issues_enabled: false,
      contact_links: [
        { url: "https://github.com/genm/rackio/security/advisories/new" },
        { url: "https://github.com/genm/rackio/blob/main/SUPPORT.md" },
      ],
    },
    backlog: "Suggested labels: `type:bug`",
    codeowners: "* @genm\n",
  };
}

test("valid governance metadata produces machine-readable counts", () => {
  assert.deepEqual(validateGovernance(validInput()), {
    labels: 1,
    issue_forms: 1,
    backlog_labels: 1,
    codeowners: true,
  });
});

test("an issue form cannot reference a label outside the catalog", () => {
  const input = validInput();
  input.issueForms.get(".github/ISSUE_TEMPLATE/bug.yml").labels = ["status:unknown"];
  assert.throws(
    () => validateGovernance(input),
    (error) =>
      error instanceof GovernanceValidationError &&
      error.errors.includes(
        ".github/ISSUE_TEMPLATE/bug.yml: references unknown label status:unknown",
      ),
  );
});

test("malformed YAML fails closed with the owning path", () => {
  assert.throws(
    () => parseYaml("body: [", ".github/ISSUE_TEMPLATE/broken.yml"),
    (error) =>
      error instanceof GovernanceValidationError &&
      error.message.includes(".github/ISSUE_TEMPLATE/broken.yml: invalid YAML"),
  );
});
