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

  // Swap is offered because the peer's minute buckets aggregate it, on the
  // same terms as memory and disk.
  await toggle.getByRole("button", { name: "Swap" }).click();
  await expect(component.getByRole("img", { name: /24h Swap history/ })).toBeVisible();

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

test("reports swap and uptime as machine fields", async ({ mount }) => {
  const component = await mount(
    <MachineDetail detail={machineDetailStateRegistry.ready} onClose={() => undefined} />,
  );
  const fields = component.locator(".detail-metrics");
  await expect(fields.getByText("25%")).toBeVisible();
  // Uptime is a field rather than a chart: it renders one fixed instant.
  await expect(fields.getByText("12d 4h")).toBeVisible();
});

test("says a machine has no swap device and no uptime instead of showing zeroes", async ({
  mount,
}) => {
  const component = await mount(
    <MachineDetail detail={machineDetailStateRegistry.swapless} onClose={() => undefined} />,
  );
  const fields = component.locator(".detail-metrics");
  await expect(fields.getByText("0%")).toHaveCount(0);
  await expect(fields.getByText("0s")).toHaveCount(0);
  await expect(fields.locator("dd").filter({ hasText: /^—$/ })).toHaveCount(2);
});

test("lists every filesystem, fullest first, with its mount named", async ({ mount }) => {
  // The metric tiles report one filesystem. An operator answering an alert
  // that names `/data` has to be able to see `/data` — and the machine's other
  // capacity — somewhere in the viewer.
  const component = await mount(
    <MachineDetail detail={machineDetailStateRegistry.ready} onClose={() => undefined} />,
  );
  const filesystems = component.locator(".detail-filesystems li");
  await expect(filesystems).toHaveCount(3);
  await component.screenshot({ path: "../../output/playwright/machine-detail-filesystems.png" });
  await expect(filesystems.first()).toContainText("/data");
  await expect(filesystems.first()).toContainText("92%");
  await expect(filesystems.nth(1)).toContainText("/");
  await expect(filesystems.nth(2)).toContainText("/boot");
});

test("says a machine reported no filesystem instead of showing an empty list", async ({
  mount,
}) => {
  const component = await mount(
    <MachineDetail detail={machineDetailStateRegistry.filesystemless} onClose={() => undefined} />,
  );
  await expect(component.locator(".detail-filesystems li")).toHaveCount(0);
  await expect(component.getByText("This machine reported no measurable filesystem")).toBeVisible();
});

test("distinguishes empty history from a zero metric", async ({ mount }) => {
  const component = await mount(
    <MachineDetail detail={machineDetailStateRegistry.empty} onClose={() => undefined} />,
  );
  await expect(component.getByText("No history in this range")).toBeVisible();
  await expect(component.getByText("0%")).toHaveCount(0);
});
