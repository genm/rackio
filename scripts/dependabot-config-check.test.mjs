import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";
import { load } from "js-yaml";

const config = load(readFileSync(".github/dependabot.yml", "utf8"));
const knownLabels = new Set(
  JSON.parse(readFileSync(".github/labels.json", "utf8")).map((label) => label.name),
);

test("every maintained dependency surface is monitored", () => {
  const surfaces = config.updates.flatMap((update) => {
    const directories = update.directories ?? [update.directory];
    return directories.map((directory) => `${update["package-ecosystem"]}:${directory}`);
  });

  assert.deepEqual(surfaces.sort(), [
    "cargo:/",
    "cargo:/fuzz",
    "docker:/relay-package",
    "github-actions:/",
    "npm:/",
  ]);
});

test("version checks are bounded, staggered, and do not group security updates", () => {
  const scheduleSlots = new Set();
  for (const update of config.updates) {
    assert.equal(update.schedule.interval, "weekly");
    assert.equal(update.schedule.timezone, "Asia/Tokyo");
    assert.ok(update["open-pull-requests-limit"] >= 1);
    assert.ok(update["open-pull-requests-limit"] <= 5);
    assert.deepEqual(update.cooldown, {
      "default-days": 3,
      "semver-minor-days": 7,
      "semver-major-days": 14,
    });
    assert.ok(update.labels.includes("dependencies"));
    for (const label of update.labels) {
      assert.ok(knownLabels.has(label), `unknown Dependabot label: ${label}`);
    }
    assert.deepEqual(update["commit-message"], { prefix: "chore", include: "scope" });
    for (const group of Object.values(update.groups ?? {})) {
      assert.notEqual(group["applies-to"], "security-updates");
    }
    scheduleSlots.add(`${update.schedule.day}:${update.schedule.time}`);
  }
  assert.equal(scheduleSlots.size, config.updates.length);
});
