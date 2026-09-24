import { existsSync, lstatSync, readdirSync, readFileSync } from "node:fs";
import { isAbsolute, join, parse, relative, resolve, sep } from "node:path";

// The first line Cargo writes into every target and build directory it owns.
// See https://bford.info/cachedir/ for the convention.
export const cargoCacheTagSignature = "Signature: 8a477f597d28d172789f06886806bc55";

// Cargo lays these out inside every profile directory. The names are only used
// to label the size report; cleanup never deletes by name and always goes
// through `cargo clean`, so a future layout change degrades the report into an
// `other` bucket instead of deleting the wrong thing.
const intermediateCategories = new Map([
  ["deps", "deps"],
  ["incremental", "incremental"],
  ["build", "buildScripts"],
  [".fingerprint", "fingerprint"],
]);

// Allocated bytes rather than apparent size, so the report matches what `du`
// and the file manager show. Hard links are counted once: Cargo hard-links
// final binaries to their `deps/` copies, and the caller walks final artifacts
// first so a shared inode is attributed to the artifact a developer runs.
export function diskUsage(path, seen) {
  let stat;
  try {
    stat = lstatSync(path, { bigint: true });
  } catch (error) {
    // A file Cargo removed while the walk was running holds no bytes.
    if (error.code === "ENOENT") {
      return 0;
    }
    throw error;
  }
  const identity = `${stat.dev}:${stat.ino}`;
  if (seen.has(identity)) {
    return 0;
  }
  seen.add(identity);
  let bytes = stat.blocks > 0n ? Number(stat.blocks * 512n) : Number(stat.size);
  if (stat.isDirectory()) {
    for (const entry of readdirSync(path)) {
      bytes += diskUsage(join(path, entry), seen);
    }
  }
  return bytes;
}

function isDirectory(path) {
  try {
    return lstatSync(path).isDirectory();
  } catch {
    return false;
  }
}

// Cargo serialises builds with a lock file in every profile directory:
// `.cargo-lock` in the target directory and, when `build.build-dir` is
// separate, `.cargo-build-lock` in the build directory (Cargo 1.97).
// `.fingerprint` also marks a build-directory profile in case the lock name
// changes again.
const profileMarkers = [".cargo-lock", ".cargo-build-lock", ".fingerprint"];

function isProfileDirectory(path) {
  return profileMarkers.some((marker) => existsSync(join(path, marker)));
}

function emptyProfile(name) {
  return {
    name,
    totalBytes: 0,
    finalBytes: 0,
    depsBytes: 0,
    incrementalBytes: 0,
    buildScriptsBytes: 0,
    fingerprintBytes: 0,
  };
}

function addProfile(profiles, name, directory, seen) {
  const profile = profiles.get(name) ?? emptyProfile(name);
  profiles.set(name, profile);
  const entries = readdirSync(directory);
  // Final artifacts first: see `diskUsage` on hard-link attribution.
  const ordered = [
    ...entries.filter((entry) => !intermediateCategories.has(entry)),
    ...entries.filter((entry) => intermediateCategories.has(entry)),
  ];
  for (const entry of ordered) {
    const bytes = diskUsage(join(directory, entry), seen);
    const category = intermediateCategories.get(entry) ?? "final";
    profile[`${category}Bytes`] += bytes;
    profile.totalBytes += bytes;
  }
}

function addOther(others, name, bytes) {
  others.set(name, (others.get(name) ?? 0) + bytes);
}

function walkCacheRoot(root, profiles, others, seen) {
  // The root directory inode itself is small but counted, so totals match `du`.
  let bytes = 0;
  const rootStat = lstatSync(root, { bigint: true });
  const rootIdentity = `${rootStat.dev}:${rootStat.ino}`;
  if (!seen.has(rootIdentity)) {
    seen.add(rootIdentity);
    bytes += rootStat.blocks > 0n ? Number(rootStat.blocks * 512n) : Number(rootStat.size);
  }
  for (const entry of readdirSync(root)) {
    const path = join(root, entry);
    if (isDirectory(path) && isProfileDirectory(path)) {
      const before = profiles.get(entry)?.totalBytes ?? 0;
      addProfile(profiles, entry, path, seen);
      bytes += profiles.get(entry).totalBytes - before;
      continue;
    }
    // A cross-compilation target such as `x86_64-pc-windows-gnu/` nests its
    // own profile directories.
    if (
      isDirectory(path) &&
      readdirSync(path).some((child) => isProfileDirectory(join(path, child)))
    ) {
      for (const child of readdirSync(path)) {
        const childPath = join(path, child);
        const name = `${entry}/${child}`;
        if (isDirectory(childPath) && isProfileDirectory(childPath)) {
          const before = profiles.get(name)?.totalBytes ?? 0;
          addProfile(profiles, name, childPath, seen);
          bytes += profiles.get(name).totalBytes - before;
        } else {
          const childBytes = diskUsage(childPath, seen);
          addOther(others, name, childBytes);
          bytes += childBytes;
        }
      }
      continue;
    }
    const entryBytes = diskUsage(path, seen);
    addOther(others, entry, entryBytes);
    bytes += entryBytes;
  }
  return bytes;
}

// Summarises a Cargo target directory and, when configured, its separate
// build directory. Missing directories are reported as absent rather than as
// an empty cache, so a mistyped override is visible instead of reading as 0.
export function summarizeBuildCache({ targetDirectory, buildDirectory }) {
  const target = resolve(targetDirectory);
  const build = resolve(buildDirectory ?? targetDirectory);
  const separated = target !== build;
  const profiles = new Map();
  const others = new Map();
  const seen = new Set();

  const directories = [];
  // Target first: see `diskUsage` on hard-link attribution.
  for (const [role, path] of separated
    ? [
        ["target", target],
        ["build", build],
      ]
    : [["target+build", target]]) {
    const exists = isDirectory(path);
    const bytes = exists ? walkCacheRoot(path, profiles, others, seen) : null;
    directories.push({ role, path, exists, bytes });
  }

  const byName = (left, right) => left.name.localeCompare(right.name);
  return {
    schemaVersion: 1,
    separated,
    totalBytes: directories.reduce((total, directory) => total + (directory.bytes ?? 0), 0),
    directories,
    profiles: [...profiles.values()].sort(byName),
    other: [...others.entries()].map(([name, bytes]) => ({ name, bytes })).sort(byName),
  };
}

export function formatBytes(bytes) {
  if (bytes === null || bytes === undefined) {
    return "absent";
  }
  const units = ["B", "KiB", "MiB", "GiB", "TiB"];
  let value = bytes;
  let unit = 0;
  while (value >= 1024 && unit < units.length - 1) {
    value /= 1024;
    unit += 1;
  }
  return unit === 0 ? `${value} B` : `${value.toFixed(1)} ${units[unit]}`;
}

export function renderSummary(summary) {
  const lines = [];
  for (const directory of summary.directories) {
    lines.push(
      `${directory.role.padEnd(13)} ${formatBytes(directory.bytes).padStart(10)}  ${directory.path}`,
    );
  }
  lines.push(`${"total".padEnd(13)} ${formatBytes(summary.totalBytes).padStart(10)}`);
  if (!summary.separated) {
    lines.push(
      "note: final artifacts and intermediates share one directory; set CARGO_BUILD_BUILD_DIR (mise does) to separate them",
    );
  }
  if (summary.profiles.length > 0) {
    lines.push("");
    const header = ["profile", "total", "final", "deps", "incremental", "build", "fingerprint"];
    const rows = summary.profiles.map((profile) => [
      profile.name,
      formatBytes(profile.totalBytes),
      formatBytes(profile.finalBytes),
      formatBytes(profile.depsBytes),
      formatBytes(profile.incrementalBytes),
      formatBytes(profile.buildScriptsBytes),
      formatBytes(profile.fingerprintBytes),
    ]);
    const widths = header.map((cell, column) =>
      Math.max(cell.length, ...rows.map((row) => row[column].length)),
    );
    for (const row of [header, ...rows]) {
      lines.push(
        row
          .map((cell, column) =>
            column === 0 ? cell.padEnd(widths[column]) : cell.padStart(widths[column]),
          )
          .join("  "),
      );
    }
  }
  if (summary.other.length > 0) {
    lines.push("");
    for (const entry of summary.other) {
      lines.push(`${entry.name.padEnd(24)} ${formatBytes(entry.bytes).padStart(10)}`);
    }
  }
  return `${lines.join("\n")}\n`;
}

function contains(parent, child) {
  const path = relative(parent, child);
  return path === "" || (!path.startsWith("..") && !isAbsolute(path));
}

// Fails closed unless `directory` is a Cargo-owned cache that does not contain
// the checkout, the home directory or a filesystem root. `cargo clean`
// deletes whole directories, so a misdirected `CARGO_TARGET_DIR` must never
// reach it.
export function cleanupRefusal(directory, { repositoryRoot, homeDirectory }) {
  const path = resolve(directory);
  if (!existsSync(path)) {
    return null;
  }
  if (path === parse(path).root) {
    return `${path} is a filesystem root`;
  }
  for (const [label, protectedPath] of [
    ["the repository checkout", repositoryRoot],
    ["the home directory", homeDirectory],
  ]) {
    if (protectedPath && contains(path, resolve(protectedPath))) {
      return `${path} contains ${label}`;
    }
  }
  let tag;
  try {
    tag = readFileSync(join(path, "CACHEDIR.TAG"), "utf8");
  } catch {
    return `${path} has no Cargo CACHEDIR.TAG, so it is not known to be a Cargo cache`;
  }
  if (!tag.startsWith(cargoCacheTagSignature)) {
    return `${path} has a CACHEDIR.TAG that Cargo did not write`;
  }
  return null;
}

// Returns the processes whose executable lives inside one of `directories`.
export function processesUnder(processes, directories) {
  const roots = directories.map((directory) => resolve(directory));
  return processes.filter(
    (process) =>
      isAbsolute(process.executable) &&
      roots.some(
        (root) => process.executable === root || process.executable.startsWith(`${root}${sep}`),
      ),
  );
}
