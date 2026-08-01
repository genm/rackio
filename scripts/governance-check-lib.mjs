import { load } from "js-yaml";

export class GovernanceValidationError extends Error {
  constructor(errors) {
    super(errors.join("\n"));
    this.name = "GovernanceValidationError";
    this.errors = errors;
  }
}

export function parseYaml(source, path) {
  try {
    const document = load(source);
    if (document === null || typeof document !== "object" || Array.isArray(document)) {
      throw new Error("top-level value must be a mapping");
    }
    return document;
  } catch (error) {
    throw new GovernanceValidationError([`${path}: invalid YAML: ${error.message}`]);
  }
}

function validateLabelCatalog(labels, errors) {
  if (!Array.isArray(labels)) {
    errors.push(".github/labels.json: top-level value must be an array");
    return new Set();
  }

  const names = new Set();
  for (const [index, label] of labels.entries()) {
    const location = `.github/labels.json[${index}]`;
    if (label === null || typeof label !== "object" || Array.isArray(label)) {
      errors.push(`${location}: label must be an object`);
      continue;
    }
    if (typeof label.name !== "string" || label.name.trim() === "") {
      errors.push(`${location}: name must be a non-empty string`);
      continue;
    }
    if (names.has(label.name)) {
      errors.push(`${location}: duplicate label ${label.name}`);
    }
    names.add(label.name);
    if (typeof label.description !== "string" || label.description.trim() === "") {
      errors.push(`${location}: description must be a non-empty string`);
    }
    if (typeof label.color !== "string" || !/^[0-9a-fA-F]{6}$/.test(label.color)) {
      errors.push(`${location}: color must be a six-digit hexadecimal value`);
    }
  }
  return names;
}

function validateIssueForm(path, form, labelNames, errors) {
  for (const field of ["name", "description", "title"]) {
    if (typeof form[field] !== "string") {
      errors.push(`${path}: ${field} must be a string`);
    }
  }

  if (!Array.isArray(form.labels)) {
    errors.push(`${path}: labels must be an array`);
  } else {
    for (const label of form.labels) {
      if (!labelNames.has(label)) {
        errors.push(`${path}: references unknown label ${label}`);
      }
    }
  }

  if (!Array.isArray(form.body) || form.body.length === 0) {
    errors.push(`${path}: body must be a non-empty array`);
    return;
  }

  const ids = new Set();
  for (const [index, item] of form.body.entries()) {
    if (item === null || typeof item !== "object" || Array.isArray(item)) {
      errors.push(`${path}: body[${index}] must be an object`);
      continue;
    }
    if (typeof item.type !== "string") {
      errors.push(`${path}: body[${index}].type must be a string`);
    }
    if (item.id !== undefined) {
      if (typeof item.id !== "string" || item.id.trim() === "") {
        errors.push(`${path}: body[${index}].id must be a non-empty string`);
      } else if (ids.has(item.id)) {
        errors.push(`${path}: duplicate body id ${item.id}`);
      } else {
        ids.add(item.id);
      }
    }
  }
}

function validateConfig(config, errors) {
  if (config.blank_issues_enabled !== false) {
    errors.push(".github/ISSUE_TEMPLATE/config.yml: blank_issues_enabled must be false");
  }
  if (!Array.isArray(config.contact_links)) {
    errors.push(".github/ISSUE_TEMPLATE/config.yml: contact_links must be an array");
    return;
  }

  const urls = new Set(config.contact_links.map((link) => link?.url));
  for (const requiredUrl of [
    "https://github.com/genm/rackio/security/advisories/new",
    "https://github.com/genm/rackio/blob/main/SUPPORT.md",
  ]) {
    if (!urls.has(requiredUrl)) {
      errors.push(`.github/ISSUE_TEMPLATE/config.yml: missing contact link ${requiredUrl}`);
    }
  }
}

function validateBacklogLabels(backlog, labelNames, errors) {
  const referenced = new Set(
    [...backlog.matchAll(/`([^`]+)`/g)]
      .flatMap((match) => match[1].split(","))
      .map((label) => label.trim())
      .filter(
        (label) => labelNames.has(label) || /^(?:type|area|platform|priority|status):/.test(label),
      ),
  );
  for (const label of referenced) {
    if (!labelNames.has(label)) {
      errors.push(`docs/backlog.md: references unknown label ${label}`);
    }
  }
  return referenced.size;
}

function validateCodeowners(codeowners, errors) {
  const globalRule = codeowners
    .split("\n")
    .map((line) => line.trim())
    .find((line) => line.startsWith("* "));
  if (!globalRule || !/\s@[A-Za-z0-9-]+/.test(globalRule)) {
    errors.push(".github/CODEOWNERS: a global rule with an accountable owner is required");
  }
}

export function validateGovernance({ labels, issueForms, config, backlog, codeowners }) {
  const errors = [];
  const labelNames = validateLabelCatalog(labels, errors);
  for (const [path, form] of issueForms) {
    validateIssueForm(path, form, labelNames, errors);
  }
  validateConfig(config, errors);
  const backlogLabelCount = validateBacklogLabels(backlog, labelNames, errors);
  validateCodeowners(codeowners, errors);

  if (errors.length > 0) {
    throw new GovernanceValidationError(errors);
  }
  return {
    labels: labelNames.size,
    issue_forms: issueForms.size,
    backlog_labels: backlogLabelCount,
    codeowners: true,
  };
}
