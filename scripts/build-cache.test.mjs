import assert from "node:assert/strict";
import { linkSync, mkdirSync, mkdtempSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import test from "node:test";

import {
  cargoCacheTagSignature,
  cleanupRefusal,
  processesUnder,
  renderSummary,
  summarizeBuildCache,
} from "./build-cache-lib.mjs";

function scratch() {
  return mkdtempSync(join(tmpdir(), "rackio-build-cache-"));
}

function write(path, bytes) {
  mkdirSync(join(path, ".."), { recursive: true });
  writeFileSync(path, Buffer.alloc(bytes, 1));
}

function cargoDirectory(path) {
  mkdirSync(path, { recursive: true });
  writeFileSync(join(path, "CACHEDIR.TAG"), `${cargoCacheTagSignature}\n# cargo\n`);
}

function profile(root, name, lock = ".cargo-lock") {
  const directory = join(root, name);
  mkdirSync(directory, { recursive: true });
  writeFileSync(join(directory, lock), "");
  return directory;
}

test("a shared directory splits each profile into final and intermediate data", () => {
  const root = join(scratch(), "target");
  cargoDirectory(root);
  const debug = profile(root, "debug");
  write(join(debug, "deps", "rackio-0123"), 64 * 1024);
  // Cargo hard-links the final binary to its deps copy: counted once, as final.
  linkSync(join(debug, "deps", "rackio-0123"), join(debug, "rackio"));
  write(join(debug, "incremental", "rackio-x", "s-1", "query-cache.bin"), 32 * 1024);
  write(join(debug, "build", "ring-1", "out", "libring.a"), 16 * 1024);
  write(join(debug, ".fingerprint", "rackio-0123", "bin-rackio"), 4 * 1024);
  const release = profile(root, "release");
  write(join(release, "rackio"), 8 * 1024);
  write(join(root, "doc", "index.html"), 4 * 1024);

  const summary = summarizeBuildCache({ targetDirectory: root, buildDirectory: root });

  assert.equal(summary.separated, false);
  assert.deepEqual(
    summary.directories.map(({ role, exists }) => ({ role, exists })),
    [{ role: "target+build", exists: true }],
  );
  const [debugSummary, releaseSummary] = summary.profiles;
  assert.equal(debugSummary.name, "debug");
  assert.ok(
    debugSummary.finalBytes >= 64 * 1024,
    "hard-linked binary is attributed to final output",
  );
  assert.ok(debugSummary.depsBytes < 64 * 1024, "the shared inode is not counted twice");
  assert.ok(debugSummary.incrementalBytes >= 32 * 1024);
  assert.ok(debugSummary.buildScriptsBytes >= 16 * 1024);
  assert.ok(debugSummary.fingerprintBytes >= 4 * 1024);
  assert.equal(
    debugSummary.totalBytes,
    debugSummary.finalBytes +
      debugSummary.depsBytes +
      debugSummary.incrementalBytes +
      debugSummary.buildScriptsBytes +
      debugSummary.fingerprintBytes,
  );
  assert.equal(releaseSummary.name, "release");
  assert.deepEqual(
    summary.other.map(({ name }) => name),
    ["CACHEDIR.TAG", "doc"],
  );
  assert.equal(summary.totalBytes, summary.directories[0].bytes);
  assert.match(renderSummary(summary), /share one directory/);
});

test("a separate build directory keeps intermediates out of the target directory", () => {
  const base = scratch();
  const target = join(base, "target");
  const build = join(base, "build");
  cargoDirectory(target);
  cargoDirectory(build);
  write(join(profile(target, "debug"), "rackio"), 8 * 1024);
  // Cargo 1.97 names the build-directory lock differently.
  write(join(profile(build, "debug", ".cargo-build-lock"), "deps", "rackio-0123"), 64 * 1024);
  const cross = profile(join(build, "x86_64-pc-windows-gnu"), "debug", ".cargo-build-lock");
  write(join(cross, "deps", "rackio-0456.exe"), 16 * 1024);

  const summary = summarizeBuildCache({ targetDirectory: target, buildDirectory: build });

  assert.equal(summary.separated, true);
  assert.deepEqual(
    summary.directories.map(({ role }) => role),
    ["target", "build"],
  );
  assert.ok(summary.directories[0].bytes < summary.directories[1].bytes);
  const names = summary.profiles.map(({ name }) => name);
  assert.deepEqual(names, ["debug", "x86_64-pc-windows-gnu/debug"]);
  assert.ok(summary.profiles[0].finalBytes >= 8 * 1024);
  assert.ok(summary.profiles[0].depsBytes >= 64 * 1024);
  assert.deepEqual(
    summary.other.map(({ name }) => name),
    ["CACHEDIR.TAG"],
    "build-directory profiles are not misreported as unclassified data",
  );
  assert.equal(
    summary.totalBytes,
    summary.directories.reduce((total, { bytes }) => total + bytes, 0),
  );
  assert.doesNotMatch(renderSummary(summary), /share one directory/);
});

test("a missing cache is reported as absent, not as an empty cache", () => {
  const missing = join(scratch(), "never-built");

  const summary = summarizeBuildCache({ targetDirectory: missing });

  assert.deepEqual(summary.directories, [
    { role: "target+build", path: missing, exists: false, bytes: null },
  ]);
  assert.match(renderSummary(summary), /absent/);
});

test("cleanup accepts only a Cargo-tagged directory", () => {
  const base = scratch();
  const context = { repositoryRoot: join(base, "checkout"), homeDirectory: join(base, "home") };
  const cache = join(base, "cache", "rackio", "target");
  cargoDirectory(cache);
  assert.equal(cleanupRefusal(cache, context), null);

  // Nothing to clean is not a refusal.
  assert.equal(cleanupRefusal(join(base, "absent"), context), null);

  const untagged = join(base, "data");
  mkdirSync(untagged);
  assert.match(cleanupRefusal(untagged, context), /no Cargo CACHEDIR\.TAG/);

  const foreign = join(base, "foreign");
  mkdirSync(foreign);
  writeFileSync(join(foreign, "CACHEDIR.TAG"), "Signature: 0000\n");
  assert.match(cleanupRefusal(foreign, context), /Cargo did not write/);
});

test("cleanup refuses directories that contain the checkout, home or a root", () => {
  const base = scratch();
  const repositoryRoot = join(base, "checkout");
  const homeDirectory = join(base, "home");
  mkdirSync(join(repositoryRoot), { recursive: true });
  mkdirSync(join(homeDirectory), { recursive: true });
  // Even a tagged directory is refused when it would take the checkout with it.
  writeFileSync(join(base, "CACHEDIR.TAG"), `${cargoCacheTagSignature}\n`);
  const context = { repositoryRoot, homeDirectory };

  assert.match(cleanupRefusal(base, context), /contains the repository checkout/);
  assert.match(cleanupRefusal(repositoryRoot, context), /contains the repository checkout/);
  assert.match(cleanupRefusal(homeDirectory, context), /contains the home directory/);
  assert.match(
    cleanupRefusal("/", { repositoryRoot: null, homeDirectory: null }),
    /filesystem root/,
  );
});

test("running processes are matched by executable location only", () => {
  const cache = join(scratch(), "cache", "rackio");
  const processes = [
    { pid: 10, executable: join(cache, "target", "debug", "rackio") },
    { pid: 11, executable: join(cache, "target-other", "debug", "rackio") },
    { pid: 12, executable: join(scratch(), "bin", "rackio") },
    { pid: 13, executable: "rackio" },
  ];

  const running = processesUnder(processes, [join(cache, "target"), join(cache, "build")]);

  assert.deepEqual(
    running.map(({ pid }) => pid),
    [10],
  );
});
