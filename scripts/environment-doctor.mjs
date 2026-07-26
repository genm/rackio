import { existsSync, readFileSync } from "node:fs";
import { dirname, resolve } from "node:path";
import { spawnSync } from "node:child_process";
import { fileURLToPath } from "node:url";

import { evaluateEnvironment } from "./environment-doctor-lib.mjs";

const repositoryRoot = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const jsonOutput = process.argv.includes("--json");
const requireRelay = process.argv.includes("--require-relay");
const useShell = process.platform === "win32";

function run(command, args = []) {
  const result = spawnSync(command, args, {
    cwd: repositoryRoot,
    encoding: "utf8",
    shell: useShell,
  });
  const output = `${result.stdout ?? ""}${result.stderr ?? ""}`.trim();
  return {
    ok: result.status === 0,
    output,
  };
}

function loadToolInventory() {
  const result = run("mise", ["ls", "--current", "--json"]);
  if (!result.ok) {
    return {};
  }

  try {
    return JSON.parse(result.output);
  } catch {
    return {};
  }
}

function managedToolCheck(inventory, name, key, command, args) {
  const expected = inventory[key]?.find((entry) => {
    const sourcePath = entry.source?.path ? resolve(entry.source.path) : "";
    return entry.active && entry.installed && sourcePath === resolve(repositoryRoot, "mise.toml");
  });
  const result = run(command, args);
  const ok = Boolean(expected) && result.ok && result.output.includes(expected.version);
  return {
    name,
    required: true,
    ok,
    detail: ok
      ? result.output.split("\n")[0]
      : result.output || `active repo-pinned ${key} tool unavailable`,
  };
}

function platformDependencyCheck() {
  if (process.platform === "darwin") {
    const result = run("xcode-select", ["--print-path"]);
    return {
      name: "tauri_system_dependencies",
      required: true,
      ok: result.ok,
      detail: result.output || "Xcode Command Line Tools unavailable",
    };
  }

  if (process.platform === "linux") {
    const webkit = run("pkg-config", ["--exists", "webkit2gtk-4.1"]);
    const rsvg = run("pkg-config", ["--exists", "librsvg-2.0"]);
    const indicator =
      run("pkg-config", ["--exists", "ayatana-appindicator3-0.1"]).ok ||
      run("pkg-config", ["--exists", "appindicator3-0.1"]).ok;
    const ok = webkit.ok && rsvg.ok && indicator;
    return {
      name: "tauri_system_dependencies",
      required: true,
      ok,
      detail: ok
        ? "webkit2gtk-4.1, librsvg-2.0 and appindicator available"
        : "install the Linux packages documented in docs/development.md",
    };
  }

  if (process.platform === "win32") {
    const host = run("rustc", ["--version", "--verbose"]);
    const webview = run("reg", [
      "query",
      "HKLM\\SOFTWARE\\Microsoft\\EdgeUpdate\\Clients",
      "/s",
      "/f",
      "WebView2 Runtime",
    ]);
    const ok = host.ok && host.output.includes("-pc-windows-msvc") && webview.ok;
    return {
      name: "tauri_system_dependencies",
      required: true,
      ok,
      detail: ok
        ? "MSVC Rust host and WebView2 Runtime available"
        : "install Microsoft C++ Build Tools and WebView2 Runtime",
    };
  }

  return {
    name: "tauri_system_dependencies",
    required: true,
    ok: false,
    detail: `unsupported development host: ${process.platform}`,
  };
}

function gitHookCheck() {
  const hookPath = run("git", ["rev-parse", "--git-path", "hooks/pre-commit"]);
  const resolvedHook = hookPath.ok ? resolve(repositoryRoot, hookPath.output) : "";
  const ok = hookPath.ok && existsSync(resolvedHook);
  return {
    name: "git_hook",
    required: true,
    ok,
    detail: ok ? resolvedHook : "run `mise run bootstrap` to install Lefthook",
  };
}

function playwrightCheck() {
  const result = run("pnpm", [
    "--filter",
    "@rackio/desktop",
    "exec",
    "playwright",
    "install",
    "--list",
  ]);
  const normalizedRoot = repositoryRoot.replaceAll("\\", "/");
  const normalizedOutput = result.output.replaceAll("\\", "/");
  // Playwright lists the shared browser cache, so require this checkout as a reference too.
  const ok =
    result.ok &&
    normalizedOutput.includes("/chromium-") &&
    normalizedOutput.includes(normalizedRoot);
  return {
    name: "playwright_chromium",
    required: true,
    ok,
    detail: ok ? "project Chromium browser installed" : "run `mise run bootstrap`",
  };
}

function dependencyCheck() {
  const result = run("pnpm", ["list", "--depth", "-1", "--json"]);
  const workspaceLock = resolve(repositoryRoot, "pnpm-lock.yaml");
  const installedLock = resolve(repositoryRoot, "node_modules/.pnpm/lock.yaml");
  // pnpm copies the resolved lockfile here; equality proves this install is not stale.
  const lockMatches =
    existsSync(installedLock) && readFileSync(workspaceLock).equals(readFileSync(installedLock));
  return {
    name: "workspace_dependencies",
    required: true,
    ok: result.ok && lockMatches,
    detail:
      result.ok && lockMatches
        ? "pnpm workspace dependencies match the lockfile"
        : result.output || "run `mise run bootstrap`",
  };
}

function dockerCheck() {
  const result = run("docker", ["info", "--format", "{{.ServerVersion}}"]);
  return {
    name: "docker",
    // The agent and desktop work without Docker; only relay development requires it.
    required: requireRelay,
    ok: result.ok,
    detail: result.ok ? `relay runtime ${result.output}` : "relay runtime unavailable",
  };
}

const toolInventory = loadToolInventory();
const checks = [
  managedToolCheck(toolInventory, "node", "node", "node", ["--version"]),
  managedToolCheck(toolInventory, "pnpm", "github:pnpm/pnpm", "pnpm", ["--version"]),
  managedToolCheck(toolInventory, "rust", "rust", "rustc", ["--version"]),
  managedToolCheck(toolInventory, "cargo_nextest", "cargo:cargo-nextest", "cargo", [
    "nextest",
    "--version",
  ]),
  managedToolCheck(toolInventory, "just", "just", "just", ["--version"]),
  managedToolCheck(toolInventory, "lefthook", "aqua:evilmartians/lefthook", "lefthook", [
    "version",
  ]),
  platformDependencyCheck(),
  dependencyCheck(),
  playwrightCheck(),
  gitHookCheck(),
  dockerCheck(),
];

const result = evaluateEnvironment(checks);

if (jsonOutput) {
  process.stdout.write(`${JSON.stringify(result, null, 2)}\n`);
} else {
  for (const check of checks) {
    const marker = check.ok ? "ok" : check.required ? "failed" : "degraded";
    process.stdout.write(`${marker.padEnd(8)} ${check.name}: ${check.detail}\n`);
  }
  process.stdout.write(`environment: ${result.status}\n`);
}

process.exitCode = result.exitCode;
