import { defineConfig, devices } from "@playwright/experimental-ct-react";

export default defineConfig({
  testDir: "./src",
  testMatch: "**/*.ct.tsx",
  outputDir: "../../test-results/playwright-ct",
  reporter: [["json", { outputFile: "../../test-results/playwright-ct.json" }]],
  use: {
    ...devices["Desktop Chrome"],
    ctPort: 3100,
    screenshot: "only-on-failure",
    trace: "retain-on-failure",
    video: "retain-on-failure",
  },
});
