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

test("switches the 24-hour chart between CPU and memory", async ({ mount }) => {
  const component = await mount(
    <MachineDetail detail={machineDetailStateRegistry.ready} onClose={() => undefined} />,
  );
  await expect(component.getByRole("img", { name: /24-hour CPU history/ })).toBeVisible();
  const toggle = component.getByLabel("History metric");
  await toggle.getByRole("button", { name: "Memory" }).click();
  await expect(component.getByRole("img", { name: /24-hour Memory history/ })).toBeVisible();
});

test("also offers disk and temperature on the 24-hour chart", async ({ mount }) => {
  const component = await mount(
    <MachineDetail detail={machineDetailStateRegistry.ready} onClose={() => undefined} />,
  );
  const toggle = component.getByLabel("History metric");

  await toggle.getByRole("button", { name: "Disk" }).click();
  await expect(component.getByRole("img", { name: /24-hour Disk history/ })).toBeVisible();

  await toggle.getByRole("button", { name: "Temp" }).click();
  await expect(component.getByRole("img", { name: /24-hour Temp history/ })).toBeVisible();

  // RTT never appears: the peer's storage never captures it, so widening the
  // schema for CPU/memory/disk/temp still cannot offer it here.
  await expect(toggle.getByRole("button", { name: "RTT" })).toHaveCount(0);
});

test("renders network throughput from the 24-hour history payload", async ({ mount }) => {
  const component = await mount(
    <MachineDetail detail={machineDetailStateRegistry.ready} onClose={() => undefined} />,
  );
  const toggle = component.getByLabel("History metric");

  await toggle.getByRole("button", { name: "Net In" }).click();
  await expect(component.getByRole("img", { name: /24-hour Net In history/ })).toBeVisible();
  // The data-derived ceiling covers the fixture's 1.8 MB/s peak.
  await expect(component.getByText("1.9 MiB/s")).toBeVisible();

  await toggle.getByRole("button", { name: "Net Out" }).click();
  await expect(component.getByRole("img", { name: /24-hour Net Out history/ })).toBeVisible();
  await expect(component.getByText("488 KiB/s")).toBeVisible();
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
