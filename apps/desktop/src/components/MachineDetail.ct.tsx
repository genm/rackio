import { expect, test } from "@playwright/experimental-ct-react";

import { machineDetailStateRegistry } from "../state-registry";
import { MachineDetail } from "./MachineDetail";

test("renders the shared ready history state", async ({ mount }) => {
  const component = await mount(
    <MachineDetail detail={machineDetailStateRegistry.ready} onClose={() => undefined} />,
  );
  await expect(component.getByRole("dialog", { name: "Home Server" })).toBeVisible();
  await expect(component.getByText("2 one-minute buckets")).toBeVisible();
  await component.screenshot({ path: "../../output/playwright/machine-detail-history.png" });
});

test("keeps remote history failure visible without hiding live state", async ({ mount }) => {
  const component = await mount(
    <MachineDetail detail={machineDetailStateRegistry.error} onClose={() => undefined} />,
  );
  await expect(component.getByRole("alert")).toContainText("History request timed out");
  await expect(component.getByText("Healthy")).toBeVisible();
});

test("distinguishes empty history from a zero metric", async ({ mount }) => {
  const component = await mount(
    <MachineDetail detail={machineDetailStateRegistry.empty} onClose={() => undefined} />,
  );
  await expect(component.getByText("No history in this range")).toBeVisible();
  await expect(component.getByText("0%")).toHaveCount(0);
});
