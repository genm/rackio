import { expect, test } from "@playwright/experimental-ct-react";

import type { FleetSnapshot } from "../types";
import { traySurfaceStateRegistry } from "../state-registry";
import { Dashboard } from "./Dashboard";

const snapshot: FleetSnapshot = {
  daemon: "connected",
  nodes: [
    {
      id: "node-1",
      name: "Studio Mac",
      os: "macOS · arm64",
      state: "healthy",
      path: "lan_direct",
      cpuPercent: 28,
      memoryUsedBytes: 12_800_000_000,
      memoryTotalBytes: 32_000_000_000,
      diskUsedBytes: 320_000_000_000,
      diskTotalBytes: 1_000_000_000_000,
      rttMs: 4,
      history: [18, 23, 21, 34, 29, 28],
    },
    {
      id: "node-2",
      endpointId: "endpoint-node-2",
      name: "Home Server",
      os: "Linux · x86_64",
      state: "degraded",
      path: "relayed",
      cpuPercent: 61,
      memoryUsedBytes: 24_000_000_000,
      memoryTotalBytes: 64_000_000_000,
      diskUsedBytes: 3_200_000_000_000,
      diskTotalBytes: 4_000_000_000_000,
      rttMs: 43,
      history: [42, 54, 66, 71, 64, 61],
      detail: "Storage degraded",
    },
  ],
};

test("shows relay and degraded state without disguising them as direct health", async ({
  mount,
}) => {
  const component = await mount(<Dashboard snapshot={snapshot} />);
  const server = component.locator("article").filter({ hasText: "Home Server" });
  await expect(server).toBeVisible();
  await expect(server.getByText("Relayed", { exact: true })).toBeVisible();
  await expect(server.getByText("Degraded", { exact: true })).toBeVisible();
  await expect(component.getByRole("button", { name: /pair machine/i })).toBeEnabled();
  await component.screenshot({ path: "../../output/playwright/dashboard.png" });
});

test("imports a one-time pairing bundle from the desktop", async ({ mount }) => {
  let submitted = "";
  const component = await mount(
    <Dashboard
      snapshot={snapshot}
      onPair={async (bundle) => {
        submitted = bundle;
      }}
    />,
  );
  await component.getByRole("button", { name: /pair machine/i }).click();
  const dialog = component.getByRole("dialog", { name: /pair a machine/i });
  await expect(dialog).toBeVisible();
  await dialog.screenshot({ path: "../../output/playwright/pair-machine.png" });
  await dialog.getByLabel("Pairing bundle").fill("  rackio-pair:test-bundle  ");
  await dialog.getByRole("button", { name: /pair machine/i }).click();
  await expect.poll(() => submitted).toBe("rackio-pair:test-bundle");
  await expect(dialog).not.toBeVisible();
});

test("keeps pairing rejection visible instead of adding a fake healthy machine", async ({
  mount,
}) => {
  const component = await mount(
    <Dashboard
      snapshot={snapshot}
      pairing={{ state: "error", message: "pairing failed: pairing window has expired" }}
    />,
  );
  await component.getByRole("button", { name: /pair machine/i }).click();
  await expect(component.getByRole("alert")).toContainText("pairing window has expired");
  await expect(component.getByText("Studio Mac")).toBeVisible();
  await expect(component.getByText("Home Server")).toBeVisible();
});

test("shows daemon failure as an alert instead of an empty healthy fleet", async ({ mount }) => {
  const component = await mount(
    <Dashboard
      snapshot={{ daemon: "unavailable", nodes: [], message: "Agent socket not found." }}
    />,
  );
  await expect(component.getByRole("alert")).toContainText("Background monitoring is disconnected");
});

test("falls back to the normal window when tray integration is unavailable", async ({ mount }) => {
  const component = await mount(
    <Dashboard snapshot={snapshot} traySurface={traySurfaceStateRegistry.unavailable} />,
  );
  await expect(component.getByText("Tray unavailable")).toBeVisible();
  await expect(component.getByText("Studio Mac")).toBeVisible();
});
