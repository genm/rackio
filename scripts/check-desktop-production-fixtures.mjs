import { readdir, readFile } from "node:fs/promises";
import { fileURLToPath } from "node:url";

const assetsDirectory = fileURLToPath(new URL("../apps/desktop/dist/assets/", import.meta.url));
const fixtureMarkers = [
  "AAAAC3NzaC1lZDI1NTE5AAAAITestFixtureOnly",
  "fixtureFingerprint",
  "rackio-pair:test-bundle",
  "History request timed out. Live monitoring continues.",
  "mDNS advertisement could not start: multicast is unavailable.",
  "Package id 0",
];

let entries;
try {
  entries = await readdir(assetsDirectory, { withFileTypes: true });
} catch (error) {
  console.error(
    JSON.stringify({
      check: "desktop-production-fixtures",
      status: "error",
      message: error instanceof Error ? error.message : String(error),
    }),
  );
  process.exit(1);
}

const assetNames = entries
  .filter((entry) => entry.isFile() && entry.name.endsWith(".js"))
  .map((entry) => entry.name)
  .sort();

if (assetNames.length === 0) {
  console.error(
    JSON.stringify({
      check: "desktop-production-fixtures",
      status: "error",
      message: "production build contains no JavaScript assets",
    }),
  );
  process.exit(1);
}

const leaks = [];
for (const assetName of assetNames) {
  const contents = await readFile(
    new URL(`../apps/desktop/dist/assets/${assetName}`, import.meta.url),
    "utf8",
  );
  for (const marker of fixtureMarkers) {
    if (contents.includes(marker)) leaks.push({ asset: assetName, marker });
  }
}

if (leaks.length > 0) {
  console.error(
    JSON.stringify({
      check: "desktop-production-fixtures",
      status: "error",
      message: "component-test fixtures leaked into the production bundle",
      leaks,
    }),
  );
  process.exit(1);
}

console.log(
  JSON.stringify({
    check: "desktop-production-fixtures",
    status: "ok",
    assetsChecked: assetNames.length,
    markersChecked: fixtureMarkers.length,
  }),
);
