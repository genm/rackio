// Reports and reclaims the local Cargo build cache. See the "Build directory"
// section of docs/development.md for the cache contract.
//
//   node scripts/build-cache.mjs report [--json]
//   node scripts/build-cache.mjs clean [--dry-run] [--all]
//
// `report` is read-only. `clean` only ever runs `cargo clean`, never deletes
// paths itself, and never stops a running Rackio process.
import { spawnSync } from "node:child_process";
import { readdirSync, readlinkSync } from "node:fs";
import { homedir } from "node:os";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";

import {
  cleanupRefusal,
  formatBytes,
  processesUnder,
  renderSummary,
  summarizeBuildCache,
} from "./build-cache-lib.mjs";

const repositoryRoot = resolve(dirname(fileURLToPath(import.meta.url)), "..");
// The profiles a normal development loop writes to: `dev` (which `test` shares)
// and the full-debug-info `debugging` profile from Cargo.toml.
const developmentProfiles = ["dev", "debugging"];
const useShell = process.platform === "win32";

function fail(message) {
  process.stderr.write(`build-cache: ${message}\n`);
  process.exit(1);
}

function cacheDirectories() {
  const result = spawnSync("cargo", ["metadata", "--format-version=1", "--no-deps", "--locked"], {
    cwd: repositoryRoot,
    encoding: "utf8",
    maxBuffer: 64 * 1024 * 1024,
    shell: useShell,
  });
  if (result.status !== 0) {
    fail(`cargo metadata failed:\n${result.stderr ?? ""}`);
  }
  const metadata = JSON.parse(result.stdout);
  if (typeof metadata.target_directory !== "string") {
    fail("cargo metadata did not report a target directory");
  }
  return {
    targetDirectory: metadata.target_directory,
    // Older Cargo omits the field when no build directory is configured.
    buildDirectory: metadata.build_directory ?? metadata.target_directory,
  };
}

// Best effort: Linux via /proc, macOS via ps (whose `comm` is the full
// executable path). Returns null when this host cannot be inspected.
function runningProcesses() {
  if (process.platform === "linux") {
    const processes = [];
    for (const entry of readdirSync("/proc")) {
      if (!/^\d+$/.test(entry)) {
        continue;
      }
      try {
        processes.push({ pid: Number(entry), executable: readlinkSync(`/proc/${entry}/exe`) });
      } catch {
        // Another user's process or one that already exited.
      }
    }
    return processes;
  }
  if (process.platform === "darwin") {
    const result = spawnSync("ps", ["-axo", "pid=,comm="], { encoding: "utf8" });
    if (result.status !== 0) {
      return null;
    }
    return result.stdout
      .split("\n")
      .map((line) => line.trim().match(/^(\d+)\s+(.+)$/))
      .filter(Boolean)
      .map(([, pid, executable]) => ({ pid: Number(pid), executable }));
  }
  return null;
}

function report(json) {
  const summary = summarizeBuildCache(cacheDirectories());
  process.stdout.write(json ? `${JSON.stringify(summary, null, 2)}\n` : renderSummary(summary));
}

function clean({ dryRun, all }) {
  const directories = cacheDirectories();
  const paths = [...new Set([directories.targetDirectory, directories.buildDirectory])];
  for (const path of paths) {
    const refusal = cleanupRefusal(path, { repositoryRoot, homeDirectory: homedir() });
    if (refusal) {
      fail(`refusing to clean: ${refusal}`);
    }
  }

  const processes = runningProcesses();
  if (processes === null) {
    process.stderr.write(
      "build-cache: cannot list running processes on this OS; stop any Rackio agent or desktop started from this cache first\n",
    );
  } else {
    const running = processesUnder(processes, paths);
    if (running.length > 0) {
      const list = running.map((entry) => `  pid ${entry.pid}: ${entry.executable}`).join("\n");
      // Deliberately not stopped here: the operator decides when the daemon
      // or tray goes down.
      fail(`refusing to clean while processes run from the build cache; stop them first:\n${list}`);
    }
  }

  const scope = all
    ? "every profile and target"
    : `development profiles (${developmentProfiles.join(", ")})`;
  process.stdout.write(
    `${dryRun ? "Would clean" : "Cleaning"} ${scope} in:\n${paths.map((path) => `  ${path}`).join("\n")}\n`,
  );
  const before = summarizeBuildCache(directories).totalBytes;
  const invocations = all
    ? [["clean"]]
    : developmentProfiles.map((profile) => ["clean", "--profile", profile]);
  for (const args of invocations) {
    const result = spawnSync("cargo", [...args, ...(dryRun ? ["--dry-run"] : [])], {
      cwd: repositoryRoot,
      stdio: "inherit",
      shell: useShell,
    });
    if (result.status !== 0) {
      fail(`cargo ${args.join(" ")} failed`);
    }
  }
  if (!dryRun) {
    const after = summarizeBuildCache(directories).totalBytes;
    process.stdout.write(
      `Build cache: ${formatBytes(before)} -> ${formatBytes(after)} (freed ${formatBytes(Math.max(0, before - after))})\n`,
    );
  }
}

const [command, ...options] = process.argv.slice(2);
const known = new Set(["--json", "--dry-run", "--all"]);
const unknown = options.filter((option) => !known.has(option));
if (unknown.length > 0) {
  fail(`unknown option ${unknown.join(" ")}`);
}
if (command === "report") {
  if (options.some((option) => option !== "--json")) {
    fail("report accepts only --json");
  }
  report(options.includes("--json"));
} else if (command === "clean") {
  if (options.includes("--json")) {
    fail("clean does not accept --json");
  }
  clean({ dryRun: options.includes("--dry-run"), all: options.includes("--all") });
} else {
  fail("usage: build-cache.mjs report [--json] | clean [--dry-run] [--all]");
}
