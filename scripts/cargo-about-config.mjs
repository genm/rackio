#!/usr/bin/env node

// deny.toml remains the license-policy SSOT. This derives cargo-about's
// equivalent config from its parsed values, not from its text layout, so any
// TOML spelling of the same policy yields the same notices — and a missing or
// malformed policy stops notice generation instead of producing a partial one.

import { readFileSync, writeFileSync } from "node:fs";
import { pathToFileURL } from "node:url";
import { parse, stringify } from "smol-toml";

function requireStringList(value, key) {
  if (value === undefined) {
    throw new Error(`deny.toml must define ${key}`);
  }
  if (!Array.isArray(value)) {
    throw new Error(`deny.toml ${key} must be an array of strings`);
  }
  if (value.length === 0) {
    throw new Error(`deny.toml ${key} must not be empty`);
  }
  for (const [index, entry] of value.entries()) {
    if (typeof entry !== "string" || entry.trim() === "") {
      throw new Error(`deny.toml ${key}[${index}] must be a non-empty string`);
    }
  }
  return value;
}

function requireTable(value, key) {
  if (value === undefined) {
    return {};
  }
  if (value === null || typeof value !== "object" || Array.isArray(value)) {
    throw new Error(`deny.toml ${key} must be a table`);
  }
  return value;
}

export function cargoAboutConfigFromDenyPolicy(denyToml) {
  let policy;
  try {
    policy = parse(denyToml);
  } catch (error) {
    throw new Error(`deny.toml is not valid TOML: ${error.message}`, { cause: error });
  }
  const targets = requireStringList(requireTable(policy.graph, "graph").targets, "graph.targets");
  const accepted = requireStringList(
    requireTable(policy.licenses, "licenses").allow,
    "licenses.allow",
  );
  // Order is preserved as written: it is the policy author's, and a stable
  // serialization keeps the generated config diffable across runs.
  return stringify({ targets, accepted });
}

function main([denyPath, outputPath]) {
  if (!denyPath || !outputPath) {
    throw new Error("usage: cargo-about-config.mjs <deny.toml> <output.toml>");
  }
  writeFileSync(outputPath, `${cargoAboutConfigFromDenyPolicy(readFileSync(denyPath, "utf8"))}\n`);
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  try {
    main(process.argv.slice(2));
  } catch (error) {
    console.error(error.message);
    process.exit(2);
  }
}
