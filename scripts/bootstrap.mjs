import { mkdirSync } from "node:fs";
import { dirname, resolve } from "node:path";
import { spawnSync } from "node:child_process";
import { fileURLToPath } from "node:url";

const repositoryRoot = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const useShell = process.platform === "win32";

function run(name, command, args) {
  process.stdout.write(`bootstrap: ${name}\n`);
  const result = spawnSync(command, args, {
    cwd: repositoryRoot,
    stdio: "inherit",
    shell: useShell,
  });

  if (result.status !== 0) {
    process.stderr.write(
      `${JSON.stringify({
        operation: "bootstrap",
        status: "failed",
        step: name,
        exitCode: result.status ?? 1,
      })}\n`,
    );
    process.exit(result.status ?? 1);
  }
}

mkdirSync(resolve(repositoryRoot, "test-results"), { recursive: true });

run("install workspace dependencies", "pnpm", ["install", "--frozen-lockfile"]);
run("fetch locked Rust dependencies", "cargo", ["fetch", "--locked"]);
run("install Playwright Chromium", "pnpm", [
  "--filter",
  "@tray-monitor/desktop",
  "exec",
  "playwright",
  "install",
  "chromium",
]);
run("validate Lefthook configuration", "lefthook", ["validate"]);
run("install Git hooks", "lefthook", ["install"]);
run("verify environment", "node", ["scripts/environment-doctor.mjs", "--json"]);

process.stdout.write('{"operation":"bootstrap","status":"ready"}\n');
