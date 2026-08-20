import { expect, test } from "@playwright/experimental-ct-react";

import { machineDetailStateRegistry } from "../state-registry";
import { MachineDetail } from "./MachineDetail";

test("renders the shared ready history state", async ({ mount }) => {
  const component = await mount(
    <MachineDetail detail={machineDetailStateRegistry.ready} onClose={() => undefined} />,
  );
  await expect(component.getByRole("dialog", { name: "Home Server" })).toBeVisible();
  await expect(component.getByText("4 one-minute buckets")).toBeVisible();
  await component.screenshot({ path: "../../output/playwright/machine-detail-history.png" });
});

test("switches the history chart between CPU and memory", async ({ mount }) => {
  const component = await mount(
    <MachineDetail detail={machineDetailStateRegistry.ready} onClose={() => undefined} />,
  );
  await expect(component.getByRole("img", { name: /24h CPU history/ })).toBeVisible();
  const toggle = component.getByLabel("History metric");
  await toggle.getByRole("button", { name: "Memory" }).click();
  await expect(component.getByRole("img", { name: /24h Memory history/ })).toBeVisible();
});

test("requests a different range without changing the machine", async ({ mount }) => {
  const requested: number[] = [];
  const component = await mount(
    <MachineDetail
      detail={machineDetailStateRegistry.ready}
      onClose={() => undefined}
      onRangeChange={(hours) => requested.push(hours)}
    />,
  );
  const ranges = component.getByLabel("History range");
  await expect(ranges.getByRole("button", { name: "24h" })).toHaveAttribute("aria-pressed", "true");
  await ranges.getByRole("button", { name: "7d" }).click();
  await expect.poll(() => requested).toEqual([168]);
});

test("breaks the history line across a reporting gap", async ({ mount }) => {
  const component = await mount(
    <MachineDetail detail={machineDetailStateRegistry.ready} onClose={() => undefined} />,
  );
  // The fixture holds two one-minute runs an hour apart, so the chart must
  // draw two polylines rather than one line across the outage.
  await expect(component.locator(".history-chart polyline")).toHaveCount(2);
  await component.screenshot({ path: "../../output/playwright/machine-detail-history.png" });
});

test("also offers disk and temperature on the history chart", async ({ mount }) => {
  const component = await mount(
    <MachineDetail detail={machineDetailStateRegistry.ready} onClose={() => undefined} />,
  );
  const toggle = component.getByLabel("History metric");

  // The chart is named for the range actually being shown, so the label moves
  // with the range toggle rather than always claiming 24 hours.
  await toggle.getByRole("button", { name: "Disk" }).click();
  await expect(component.getByRole("img", { name: /24h Disk history/ })).toBeVisible();

  await toggle.getByRole("button", { name: "Temp" }).click();
  await expect(component.getByRole("img", { name: /24h Temp history/ })).toBeVisible();

  // RTT never appears: the peer's storage never captures it, so widening the
  // schema for CPU/memory/disk/temp still cannot offer it here.
  await expect(toggle.getByRole("button", { name: "RTT" })).toHaveCount(0);
});

test("keeps remote history failure visible without hiding live state", async ({ mount }) => {
  const component = await mount(
    <MachineDetail detail={machineDetailStateRegistry.error} onClose={() => undefined} />,
  );
  await expect(component.getByRole("alert")).toContainText("History request timed out");
  await expect(component.getByText("Healthy")).toBeVisible();
});

test("closes the history dialog with Escape", async ({ mount }) => {
  let closed = 0;
  const component = await mount(
    <MachineDetail
      detail={machineDetailStateRegistry.ready}
      onClose={() => {
        closed += 1;
      }}
    />,
  );
  await expect(component.getByRole("dialog", { name: "Home Server" })).toBeVisible();
  await component.page().keyboard.press("Escape");
  await expect.poll(() => closed).toBe(1);
});

test("names the sensor behind the temperature it reports", async ({ mount }) => {
  const component = await mount(
    <MachineDetail detail={machineDetailStateRegistry.ready} onClose={() => undefined} />,
  );
  await expect(component.getByText("62 °C")).toBeVisible();
  await expect(component.getByText("Package id 0")).toBeVisible();
});

test("distinguishes empty history from a zero metric", async ({ mount }) => {
  const component = await mount(
    <MachineDetail detail={machineDetailStateRegistry.empty} onClose={() => undefined} />,
  );
  await expect(component.getByText("No history in this range")).toBeVisible();
  await expect(component.getByText("0%")).toHaveCount(0);
});
