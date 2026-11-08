import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { mkdtempSync, readFileSync, rmSync } from "node:fs";
import test from "node:test";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const repositoryRoot = resolve(fileURLToPath(new URL("..", import.meta.url)));

test("JavaScript license notices include workspace production dependencies", () => {
  const appPackage = JSON.parse(
    readFileSync(new URL("../apps/desktop/package.json", import.meta.url), "utf8"),
  );
  const expectedPackage = Object.keys(appPackage.dependencies ?? {})[0];
  assert.ok(expectedPackage, "desktop manifest should declare production dependencies");

  const temporaryDirectory = mkdtempSync(join(tmpdir(), "rackio-js-licenses-"));
  const output = join(temporaryDirectory, "notices.html");

  try {
    const result = spawnSync(
      process.execPath,
      [resolve(repositoryRoot, "scripts/generate-javascript-licenses.mjs"), output],
      { cwd: repositoryRoot, encoding: "utf8", timeout: 30_000 },
    );
    assert.equal(result.status, 0, result.stderr || result.stdout);

    const notices = readFileSync(output, "utf8");
    assert.ok(
      notices.includes(`<h2>${expectedPackage} `),
      `generated notices should include ${expectedPackage} from @rackio/desktop`,
    );
  } finally {
    rmSync(temporaryDirectory, { recursive: true, force: true });
  }
});
