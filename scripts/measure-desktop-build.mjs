import { execFileSync } from "node:child_process";
import {
  existsSync,
  mkdirSync,
  mkdtempSync,
  readdirSync,
  rmSync,
  statSync,
  writeFileSync,
} from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const repositoryRoot = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const suppliedTarget = process.argv[2];
const ownsTarget = suppliedTarget === undefined;
const targetDirectory = ownsTarget
  ? mkdtempSync(join(tmpdir(), "rackio-desktop-build-"))
  : resolve(suppliedTarget);
const outputPath = resolve(repositoryRoot, "test-results/desktop-build-footprint.json");
const maxTargetBytes = Number(process.env.RACKIO_MAX_DESKTOP_BUILD_BYTES ?? "1610612736");

if (!Number.isSafeInteger(maxTargetBytes) || maxTargetBytes <= 0) {
  throw new Error("RACKIO_MAX_DESKTOP_BUILD_BYTES must be a positive integer");
}

function logicalBytes(path, seen = new Set()) {
  const stat = statSync(path, { bigint: true });
  const identity = `${stat.dev}:${stat.ino}`;
  if (seen.has(identity)) {
    return 0;
  }
  seen.add(identity);
  if (stat.isDirectory()) {
    return readdirSync(path).reduce(
      (total, entry) => total + logicalBytes(join(path, entry), seen),
      0,
    );
  }
  return Number(stat.size);
}

try {
  if (ownsTarget) {
    execFileSync("cargo", ["build", "--locked", "-p", "rackio-desktop"], {
      cwd: repositoryRoot,
      env: { ...process.env, CARGO_TARGET_DIR: targetDirectory },
      stdio: "inherit",
    });
  }

  const binaryName = process.platform === "win32" ? "rackio-desktop.exe" : "rackio-desktop";
  const binaryPath = join(targetDirectory, "debug", binaryName);
  const result = {
    schemaVersion: 1,
    profile: "dev",
    targetBytes: logicalBytes(targetDirectory),
    binaryBytes: existsSync(binaryPath) ? Number(statSync(binaryPath).size) : null,
    maxTargetBytes,
  };
  result.ok = result.targetBytes <= result.maxTargetBytes;

  mkdirSync(dirname(outputPath), { recursive: true });
  writeFileSync(outputPath, `${JSON.stringify(result, null, 2)}\n`);
  process.stdout.write(`${JSON.stringify(result)}\n`);

  if (!result.ok) {
    process.exitCode = 1;
  }
} finally {
  if (ownsTarget) {
    // This process created the exact temporary directory, so scoped cleanup is safe.
    rmSync(targetDirectory, { recursive: true, force: true });
  }
}
