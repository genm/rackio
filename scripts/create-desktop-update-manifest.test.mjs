import assert from "node:assert/strict";
import { createHash } from "node:crypto";
import { mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import test from "node:test";

import { createDesktopUpdateManifest } from "./create-desktop-update-manifest.mjs";

test("assembles stable macOS updater assets and manifest", () => {
  const root = mkdtempSync(join(tmpdir(), "rackio-updater-manifest-"));
  try {
    const artifactsDirectory = join(root, "input");
    const outputDirectory = join(root, "output");
    for (const platform of ["darwin-aarch64", "darwin-x86_64"]) {
      const platformDirectory = join(artifactsDirectory, platform);
      mkdirSync(platformDirectory, { recursive: true });
      writeFileSync(join(platformDirectory, "Rackio.app.tar.gz"), `${platform} bundle`);
      writeFileSync(join(platformDirectory, "Rackio.app.tar.gz.sig"), `${platform} signature\n`);
    }

    const manifest = createDesktopUpdateManifest({
      version: "0.1.0",
      releaseBaseUrl: "https://updates.example.test/rackio/releases/v0.1.0",
      artifactsDirectory,
      outputDirectory,
    });

    assert.deepEqual(Object.keys(manifest.platforms).sort(), ["darwin-aarch64", "darwin-x86_64"]);
    assert.equal(
      manifest.platforms["darwin-aarch64"].url,
      "https://updates.example.test/rackio/releases/v0.1.0/Rackio-darwin-aarch64.app.tar.gz",
    );
    assert.equal(manifest.platforms["darwin-x86_64"].signature, "darwin-x86_64 signature");
    assert.equal(
      readFileSync(join(outputDirectory, "Rackio-darwin-aarch64.app.tar.gz"), "utf8"),
      "darwin-aarch64 bundle",
    );
    assert.deepEqual(
      JSON.parse(readFileSync(join(outputDirectory, "latest.json"), "utf8")),
      manifest,
    );
    const checksumLines = readFileSync(join(outputDirectory, "SHA256SUMS"), "utf8")
      .trimEnd()
      .split("\n");
    assert.equal(checksumLines.length, 5);
    for (const line of checksumLines) {
      const [digest, name] = line.split("  ");
      assert.equal(
        digest,
        createHash("sha256")
          .update(readFileSync(join(outputDirectory, name)))
          .digest("hex"),
      );
    }
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
});

test("rejects pre-release versions and missing signatures", () => {
  const root = mkdtempSync(join(tmpdir(), "rackio-updater-manifest-"));
  try {
    const artifactsDirectory = join(root, "input");
    const outputDirectory = join(root, "output");
    for (const platform of ["darwin-aarch64", "darwin-x86_64"]) {
      const platformDirectory = join(artifactsDirectory, platform);
      mkdirSync(platformDirectory, { recursive: true });
      writeFileSync(join(platformDirectory, "Rackio.app.tar.gz"), "bundle");
    }

    const options = {
      releaseBaseUrl: "https://updates.example.test/rackio/releases/v0.1.0",
      artifactsDirectory,
      outputDirectory,
    };
    assert.throws(
      () => createDesktopUpdateManifest({ ...options, version: "0.1.0-rc.1" }),
      /stable semantic version/,
    );
    assert.throws(
      () => createDesktopUpdateManifest({ ...options, version: "0.1.0" }),
      /signature is missing or empty/,
    );
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
});
