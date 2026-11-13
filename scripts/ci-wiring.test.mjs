import assert from "node:assert/strict";
import { readdirSync, readFileSync } from "node:fs";
import { resolve } from "node:path";
import test from "node:test";
import { load } from "js-yaml";
import { parse } from "smol-toml";

const WORKFLOW_DIRECTORY = resolve(".github/workflows");

function workflowSteps() {
  return readdirSync(WORKFLOW_DIRECTORY)
    .filter((name) => /\.ya?ml$/.test(name))
    .flatMap((name) => {
      const workflow = load(readFileSync(resolve(WORKFLOW_DIRECTORY, name), "utf8"));
      return Object.entries(workflow.jobs ?? {}).flatMap(([jobId, job]) =>
        (job.steps ?? []).map((step) => ({ workflow: name, jobId, step })),
      );
    });
}

// Every shell line CI runs, folded the way the runner sees it.
function workflowCommandLines() {
  return workflowSteps()
    .filter(({ step }) => typeof step.run === "string")
    .flatMap(({ step }) => step.run.split("\n"))
    .map((line) => line.trim())
    .filter(Boolean);
}

const packageScripts = JSON.parse(readFileSync(resolve("package.json"), "utf8")).scripts;

// A pnpm script counts as run by CI when a workflow line invokes it, directly
// or through another script that CI runs.
function pnpmScriptsRunByCi(lines) {
  const reached = new Set();
  const visit = (command) => {
    for (const match of command.matchAll(/\bpnpm (?:run )?([\w:-]+)/g)) {
      const name = match[1];
      if (packageScripts[name] && !reached.has(name)) {
        reached.add(name);
        visit(packageScripts[name]);
      }
    }
  };
  lines.forEach(visit);
  return reached;
}

test("every local quality-gate command also runs in hosted CI", () => {
  // `mise run check` is the documented pre-handoff gate. A command that only
  // runs there can rot in CI unnoticed, as the desktop updater suite did.
  const localGate = parse(readFileSync(resolve("mise.toml"), "utf8")).tasks.check.run;
  const lines = new Set(workflowCommandLines());
  const missing = localGate.filter((command) => !lines.has(command));
  assert.deepEqual(missing, [], "add these to a workflow step, or drop them from tasks.check");
});

test("every Node test suite under scripts/ is reached by a CI-run pnpm script", () => {
  const reached = pnpmScriptsRunByCi(workflowCommandLines());
  const covered = new Set(
    [...reached].flatMap(
      (name) => packageScripts[name].match(/scripts\/[\w.-]+\.test\.mjs/g) ?? [],
    ),
  );
  const suites = readdirSync(resolve("scripts"))
    .filter((name) => name.endsWith(".test.mjs"))
    .map((name) => `scripts/${name}`);
  assert.ok(suites.length > 0);
  assert.deepEqual(
    suites.filter((suite) => !covered.has(suite)),
    [],
    "register these suites in a package.json script that a workflow runs",
  );
});

test("the wiring checks see the updater suite only through its CI step", () => {
  // Guards the guard: the updater suite is wired by exactly one CI step, so a
  // helper that stopped reading workflows would fail here rather than pass.
  const lines = workflowCommandLines();
  assert.ok(lines.includes("pnpm test:desktop-updater"));
  assert.ok(pnpmScriptsRunByCi(lines).has("test:desktop-updater"));
  assert.equal(
    pnpmScriptsRunByCi(lines.filter((line) => line !== "pnpm test:desktop-updater")).has(
      "test:desktop-updater",
    ),
    false,
  );
});

test("the Windows GNU gate keeps its full cross-compilation scope in one Clippy pass", () => {
  const ci = load(readFileSync(resolve(WORKFLOW_DIRECTORY, "ci.yml"), "utf8"));
  const commands = ci.jobs["rust-windows-cross-check"].steps
    .filter((step) => typeof step.run === "string" && /\bcargo\b/.test(step.run))
    .map((step) => step.run.replace(/\s+/g, " ").trim());
  const clippy = commands.filter((command) => command.startsWith("cargo clippy "));

  assert.equal(clippy.length, 1, JSON.stringify(commands));
  const [command] = clippy;
  const [cargoArgs, lintArgs = ""] = command.split(" -- ");
  for (const flag of ["--target x86_64-pc-windows-gnu", "--all-targets", "--all-features"]) {
    assert.ok(cargoArgs.includes(flag), flag);
  }
  for (const crate of [
    "rackio-windows-ipc",
    "rackio-core",
    "rackio-protocol",
    "rackio-iroh",
    "rackio-agent",
  ]) {
    assert.match(cargoArgs, new RegExp(`(?:^| )-p ${crate}(?: |$)`), crate);
  }
  assert.match(lintArgs, /(?:^| )-D warnings(?: |$)/);
  // A standalone `cargo check` of the same scope is subsumed by the pass above.
  assert.deepEqual(
    commands.filter((command) => command.startsWith("cargo check ")),
    [],
  );
});
