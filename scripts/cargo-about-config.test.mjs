import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { mkdtempSync, readFileSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import test from "node:test";
import { parse } from "smol-toml";
import { cargoAboutConfigFromDenyPolicy } from "./cargo-about-config.mjs";

const EXPECTED = {
  targets: ["x86_64-unknown-linux-gnu", "x86_64-pc-windows-msvc"],
  accepted: ["Apache-2.0", "MIT", "Unicode-3.0"],
};

const CANONICAL = `[graph]
targets = [
  "x86_64-unknown-linux-gnu",
  "x86_64-pc-windows-msvc",
]

[advisories]
yanked = "deny"

[licenses]
confidence-threshold = 0.93
allow = [
  "Apache-2.0",
  # A comment inside the list is not a license.
  "MIT",
  "Unicode-3.0",
]
`;

function generated(source) {
  return parse(cargoAboutConfigFromDenyPolicy(source));
}

test("the canonical layout yields the policy's targets and accepted licenses", () => {
  assert.deepEqual(generated(CANONICAL), EXPECTED);
});

test("equivalent TOML spellings of the same policy yield identical config", () => {
  const variants = {
    "no spaces around =": CANONICAL.replace("allow = [", "allow=[").replace(
      "targets = [",
      "targets=[",
    ),
    "indented keys and closing brackets": CANONICAL.replace("allow = [", "  allow = [").replaceAll(
      "\n]\n",
      "\n    ]\n",
    ),
    "single-line arrays": `[graph]
targets = ["x86_64-unknown-linux-gnu", "x86_64-pc-windows-msvc"] # shipped

[licenses]
allow = [ 'Apache-2.0', "MIT", "Unicode-3.0" ]
`,
    "dotted keys without section headers": `graph.targets = ["x86_64-unknown-linux-gnu", "x86_64-pc-windows-msvc"]
licenses.allow = ["Apache-2.0", "MIT", "Unicode-3.0"]
`,
    "reordered sections with a trailing comment on the header": `[licenses] # policy
allow = ["Apache-2.0", "MIT", "Unicode-3.0",]
[graph]
targets = ["x86_64-unknown-linux-gnu", "x86_64-pc-windows-msvc"]
`,
  };
  for (const [name, source] of Object.entries(variants)) {
    // The variant must really be the same policy, or this proves nothing.
    assert.deepEqual(parse(source).licenses.allow, parse(CANONICAL).licenses.allow, name);
    assert.deepEqual(generated(source), EXPECTED, name);
    assert.equal(
      cargoAboutConfigFromDenyPolicy(source),
      cargoAboutConfigFromDenyPolicy(CANONICAL),
      name,
    );
  }
});

test("missing, mistyped or empty policy values fail with the offending key", () => {
  for (const [source, message] of [
    ["[licenses]\nallow = ['MIT']\n", /must define graph\.targets/],
    ["[graph]\ntargets = ['x86_64-unknown-linux-gnu']\n", /must define licenses\.allow/],
    [
      "[graph]\ntargets = 'x86_64-unknown-linux-gnu'\n[licenses]\nallow = ['MIT']\n",
      /graph\.targets must be an array/,
    ],
    [
      "[graph]\ntargets = ['x86_64-unknown-linux-gnu']\n[licenses]\nallow = []\n",
      /licenses\.allow must not be empty/,
    ],
    [
      "[graph]\ntargets = ['x86_64-unknown-linux-gnu']\n[licenses]\nallow = ['MIT', 1]\n",
      /licenses\.allow\[1\] must be a non-empty string/,
    ],
    [
      "[graph]\ntargets = ['x86_64-unknown-linux-gnu']\n[licenses]\nallow = ['MIT', ' ']\n",
      /licenses\.allow\[1\] must be a non-empty string/,
    ],
    ["graph = 'x86_64-unknown-linux-gnu'\n[licenses]\nallow = ['MIT']\n", /graph must be a table/],
  ]) {
    assert.throws(() => cargoAboutConfigFromDenyPolicy(source), message, source);
  }
});

test("malformed TOML is rejected as such rather than read as an empty policy", () => {
  assert.throws(
    () => cargoAboutConfigFromDenyPolicy('[licenses]\nallow = ["MIT"\n'),
    /deny\.toml is not valid TOML/,
  );
  assert.throws(
    () => cargoAboutConfigFromDenyPolicy('[licenses]\nallow = ["MIT"]\nallow = ["ISC"]\n'),
    /deny\.toml is not valid TOML/,
  );
});

test("the repository's deny.toml translates into a usable cargo-about config", () => {
  const policy = parse(readFileSync(resolve("deny.toml"), "utf8"));
  assert.deepEqual(generated(readFileSync(resolve("deny.toml"), "utf8")), {
    targets: policy.graph.targets,
    accepted: policy.licenses.allow,
  });
});

test("the CLI writes the config and exits non-zero with a diagnostic on a bad policy", () => {
  const directory = mkdtempSync(join(tmpdir(), "rackio-cargo-about-config-"));
  const denyPath = join(directory, "deny.toml");
  const outputPath = join(directory, "about.toml");

  writeFileSync(denyPath, CANONICAL.replace("allow = [", "allow=["));
  let result = spawnSync(
    process.execPath,
    ["scripts/cargo-about-config.mjs", denyPath, outputPath],
    {
      encoding: "utf8",
    },
  );
  assert.equal(result.status, 0, result.stderr);
  assert.deepEqual(parse(readFileSync(outputPath, "utf8")), EXPECTED);

  writeFileSync(denyPath, "[licenses]\nallow = ['MIT']\n");
  result = spawnSync(process.execPath, ["scripts/cargo-about-config.mjs", denyPath, outputPath], {
    encoding: "utf8",
  });
  assert.equal(result.status, 2);
  assert.match(result.stderr, /deny\.toml must define graph\.targets/);
});
