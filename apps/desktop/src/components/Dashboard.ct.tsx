import { expect, test } from "@playwright/experimental-ct-react";

import type { FleetSnapshot, TrendPoint } from "../types";
import { traySurfaceStateRegistry } from "../state-registry";
import { Dashboard } from "./Dashboard";

/** Timestamped points at the agent's two-second cadence, newest last. */
function trendFixture(
  cpuValues: number[],
  memoryUsedBytes: number,
  memoryTotalBytes: number,
  // Zero is what a machine with swap disabled genuinely reports, so the
  // swapless fixtures carry it rather than omitting the field.
  swapTotalBytes = 8_589_934_592,
): TrendPoint[] {
  const base = 1_750_000_000_000;
  return cpuValues.map((cpuPercent, index) => ({
    timestampMs: base + index * 2_000,
    cpuPercent,
    memoryUsedBytes,
    memoryTotalBytes,
    swapUsedBytes: swapTotalBytes === 0 ? 0 : 1_073_741_824 * (1 + (index % 2)),
    swapTotalBytes,
    diskUsedBytes: 320_000_000_000,
    diskTotalBytes: 1_000_000_000_000,
    temperatureCelsius: 55 + cpuPercent / 10,
    networkReceivedBytesPerSecond: 1_024 * (index + 1),
    networkSentBytesPerSecond: 256 * (index + 1),
    rttMs: 4 + (index % 3),
  }));
}

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
      swapUsedBytes: 2_147_483_648,
      swapTotalBytes: 8_589_934_592,
      uptimeSeconds: 12 * 86_400 + 4 * 3_600,
      diskUsedBytes: 320_000_000_000,
      diskTotalBytes: 1_000_000_000_000,
      temperature: {
        label: "PMU tdie8",
        celsius: 60.7,
        criticalCelsius: 95,
        sensorCount: 41,
      },
      networkReceivedBytesPerSecond: 6_144,
      networkSentBytesPerSecond: 1_536,
      rttMs: 4,
      trend: trendFixture([18, 23, 21, 34, 29, 28], 12_800_000_000, 32_000_000_000),
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
      // A machine with no readable sensor — a container or cloud VM. The same
      // host has swap disabled and reports no uptime, so neither may be
      // rendered as a healthy-looking zero.
      temperature: null,
      swapUsedBytes: 0,
      swapTotalBytes: 0,
      rttMs: 43,
      trend: trendFixture([42, 54, 66, 71, 64, 61], 24_000_000_000, 64_000_000_000, 0),
      detail: "Storage degraded",
    },
  ],
};

test("labels the trend chart with its metric and time window", async ({ mount }) => {
  const component = await mount(<Dashboard snapshot={snapshot} />);
  const studio = component.locator("article").filter({ hasText: "Studio Mac" });
  // Six samples at the agent's two-second cadence span ten seconds.
  await expect(
    studio.getByRole("img", { name: "Studio Mac CPU load over the last 10 s" }),
  ).toBeVisible();
  await expect(studio.getByText("10 s ago")).toBeVisible();
  await expect(studio.getByText("now", { exact: true })).toBeVisible();
});

test("switches the trend chart to memory from its metric tile", async ({ mount }) => {
  const component = await mount(<Dashboard snapshot={snapshot} />);
  const studio = component.locator("article").filter({ hasText: "Studio Mac" });
  // The tile's accessible name is its content ("Memory 40%"); the title
  // attribute is only the pointer tooltip.
  const memoryTile = studio.getByRole("button", { name: /^Memory/ });
  await memoryTile.click();
  await expect(
    studio.getByRole("img", { name: "Studio Mac Memory load over the last 10 s" }),
  ).toBeVisible();
  await expect(memoryTile).toHaveAttribute("aria-pressed", "true");
  await studio.screenshot({ path: "../../output/playwright/node-card-memory.png" });
});

test("every metric tile switches the trend chart", async ({ mount }) => {
  const component = await mount(<Dashboard snapshot={snapshot} />);
  const studio = component.locator("article").filter({ hasText: "Studio Mac" });
  const expectations = [
    { tile: /^Swap/, chart: "Studio Mac Swap usage over the last 10 s" },
    { tile: /^Disk/, chart: "Studio Mac Disk usage over the last 10 s" },
    { tile: /^Temp/, chart: "Studio Mac Temperature over the last 10 s" },
    { tile: /^RTT/, chart: "Studio Mac RTT over the last 10 s" },
  ];
  for (const { tile, chart } of expectations) {
    await studio.getByRole("button", { name: tile }).click();
    await expect(studio.getByRole("img", { name: chart })).toBeVisible();
  }
  // The RTT axis derives its ceiling from the data instead of pretending the
  // unit is a percentage.
  await expect(studio.getByText("10 ms")).toBeVisible();
  await studio.screenshot({ path: "../../output/playwright/node-card-rtt.png" });
});

test("dates an offline machine's numbers instead of presenting them as live", async ({ mount }) => {
  const offline: FleetSnapshot = {
    daemon: "connected",
    nodes: [
      {
        id: "node-3",
        endpointId: "endpoint-node-3",
        name: "Steam Deck",
        os: "Linux · x86_64",
        state: "offline",
        path: "lan_direct",
        cpuPercent: 2,
        memoryUsedBytes: 4_000_000_000,
        memoryTotalBytes: 15_500_000_000,
        rttMs: 111,
        lastSeenMs: Date.now() - 5 * 60_000,
        trend: trendFixture([2, 3, 2, 4, 2, 3], 4_000_000_000, 15_500_000_000),
        detail: "remote operation timed out: connect",
      },
    ],
  };
  const component = await mount(<Dashboard snapshot={offline} />);
  const card = component.locator("article").filter({ hasText: "Steam Deck" });
  // The frozen trend must not claim to end "now", and the stale numbers must
  // be datable without hiding the failure cause.
  await expect(card.getByText("last contact", { exact: true })).toBeVisible();
  await expect(
    card.getByText(/remote operation timed out: connect · last contact 5 min ago/),
  ).toBeVisible();
  await card.screenshot({ path: "../../output/playwright/node-card-offline.png" });
});

test("puts the worst machine first so it needs no scrolling", async ({ mount }) => {
  const component = await mount(<Dashboard snapshot={snapshot} />);
  // Home Server is degraded and Studio Mac is healthy, so the degraded card
  // leads regardless of the order the daemon reported them in.
  await expect(component.locator("article h2").first()).toHaveText("Home Server");
});

test("plots network as two lines and draws the hardware temperature limit", async ({ mount }) => {
  const component = await mount(<Dashboard snapshot={snapshot} />);
  const studio = component.locator("article").filter({ hasText: "Studio Mac" });

  await studio.getByRole("button", { name: /^Net/ }).click();
  await expect(
    studio.getByRole("img", { name: "Studio Mac Network throughput over the last 10 s" }),
  ).toBeVisible();
  // Received and sent are separate quantities; summing them would report a
  // throughput no sample measured.
  await expect(studio.locator("polyline")).toHaveCount(2);
  await expect(studio.getByText("Received")).toBeVisible();

  await studio.getByRole("button", { name: /^Temp/ }).click();
  await expect(studio.getByText("critical 95 °C")).toBeVisible();
  await studio.screenshot({ path: "../../output/playwright/node-card-network.png" });
});

test("reads out the time and value under the pointer", async ({ mount }) => {
  const component = await mount(<Dashboard snapshot={snapshot} />);
  const plot = component
    .locator("article")
    .filter({ hasText: "Studio Mac" })
    .locator(".trend-plot");
  await plot.hover();
  const tooltip = component.locator(".trend-tooltip");
  await expect(tooltip).toBeVisible();
  // The readout carries a real sample's value, not an interpolation.
  await expect(tooltip).toContainText("%");
});

test("compares every machine on one chart when asked", async ({ mount }) => {
  const component = await mount(<Dashboard snapshot={snapshot} />);
  const compare = component.getByRole("button", { name: "Compare machines" });
  await compare.click();
  const chart = component.getByLabel("Compare machines");
  await expect(chart.getByRole("img", { name: /CPU load across every machine/ })).toBeVisible();
  await expect(chart.getByText("Studio Mac")).toBeVisible();
  await expect(chart.getByText("Home Server")).toBeVisible();
  await chart.screenshot({ path: "../../output/playwright/fleet-compare.png" });
  await component.getByRole("button", { name: "Hide comparison" }).click();
  await expect(component.getByLabel("Compare machines")).toHaveCount(0);
});

test("keeps each card's chosen metric across a remount", async ({ mount }) => {
  const first = await mount(<Dashboard snapshot={snapshot} />);
  await first
    .locator("article")
    .filter({ hasText: "Studio Mac" })
    .getByRole("button", { name: /^Disk/ })
    .click();
  await first.unmount();

  const second = await mount(<Dashboard snapshot={snapshot} />);
  const studio = second.locator("article").filter({ hasText: "Studio Mac" });
  await expect(studio.getByRole("button", { name: /^Disk/ })).toHaveAttribute(
    "aria-pressed",
    "true",
  );
  // Only the machine that was touched changes; the others keep the default.
  const server = second.locator("article").filter({ hasText: "Home Server" });
  await expect(server.getByRole("button", { name: /^CPU/ })).toHaveAttribute(
    "aria-pressed",
    "true",
  );
});

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

test("shows a machine temperature without inventing one for a sensorless host", async ({
  mount,
}) => {
  const component = await mount(<Dashboard snapshot={snapshot} />);
  const studio = component.locator("article").filter({ hasText: "Studio Mac" });
  await expect(studio.getByText("61 °C")).toBeVisible();
  // The named sensor and the count it was the hottest of keep the reading
  // attributable rather than presenting an anonymous number.
  await expect(studio.locator(".metric-value", { hasText: "61 °C" })).toHaveAttribute(
    "title",
    "PMU tdie8 · hottest of 41 sensors · hardware critical 95 °C",
  );

  // A machine with no readable sensor shows an em dash, never 0 °C.
  const server = component.locator("article").filter({ hasText: "Home Server" });
  await expect(server.getByText("0 °C")).toHaveCount(0);
  await expect(server.locator(".metric-value").filter({ hasText: /^—$/ }).first()).toBeVisible();
});

test("plots swap and dates uptime without inventing either", async ({ mount }) => {
  const component = await mount(<Dashboard snapshot={snapshot} />);
  const studio = component.locator("article").filter({ hasText: "Studio Mac" });

  // Swap is a sampled level, so it is a plottable tile like CPU and memory
  // rather than a static number.
  const swapTile = studio.getByRole("button", { name: /^Swap/ });
  await expect(swapTile.locator(".metric-value")).toHaveText("25%");
  await swapTile.click();
  await expect(swapTile).toHaveAttribute("aria-pressed", "true");
  await expect(
    studio.getByRole("img", { name: "Studio Mac Swap usage over the last 10 s" }),
  ).toBeVisible();
  // Uptime is a card field, not a series: it renders one fixed instant.
  await expect(studio.getByText("Uptime 12d 4h")).toBeVisible();
  await studio.screenshot({ path: "../../output/playwright/node-card-swap.png" });

  // The swapless machine has no percentage to show and no uptime to date, and
  // must say so on both instead of reading as idle swap since boot.
  const server = component.locator("article").filter({ hasText: "Home Server" });
  const serverSwap = server.getByRole("button", { name: /^Swap/ });
  await expect(serverSwap.locator(".metric-value")).toHaveText("—");
  await expect(serverSwap.locator(".metric-value")).toHaveAttribute(
    "title",
    "No swap device on this machine",
  );
  await serverSwap.click();
  await expect(server.getByText("No Swap readings on this machine")).toBeVisible();
  await expect(server.getByText("Uptime —")).toBeVisible();
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
  // The pairing panel is now a labelled `tabpanel`, so address the field by its
  // textbox role rather than by the shared accessible name.
  await dialog.getByRole("textbox", { name: "Pairing bundle" }).fill("  rackio-pair:test-bundle  ");
  await dialog.getByRole("button", { name: /pair machine/i }).click();
  await expect.poll(() => submitted).toBe("rackio-pair:test-bundle");
  await expect(dialog).not.toBeVisible();
  await component.getByRole("button", { name: /pair machine/i }).click();
  await expect(dialog.getByRole("textbox", { name: "Pairing bundle" })).toHaveValue("");
});

test("keeps pairing rejection visible instead of adding a fake healthy machine", async ({
  mount,
}) => {
  const component = await mount(
    <Dashboard
      snapshot={snapshot}
      pairing={{ state: "error", message: "pairing failed: pairing window has expired" }}
      onPair={async () => {
        throw new Error("pairing window has expired");
      }}
    />,
  );
  await component.getByRole("button", { name: /pair machine/i }).click();
  const dialog = component.getByRole("dialog", { name: /pair a machine/i });
  const draft = dialog.getByRole("textbox", { name: "Pairing bundle" });
  await draft.fill("rackio-pair:expired-bundle");
  await dialog.getByRole("button", { name: "Pair machine" }).click();
  await expect(dialog).toBeVisible();
  await expect(draft).toHaveValue("rackio-pair:expired-bundle");
  await expect(dialog.getByRole("alert")).toContainText("pairing window has expired");
  await expect(component.getByText("Studio Mac")).toBeVisible();
  await expect(component.getByText("Home Server")).toBeVisible();
});

test("imports a pairing bundle from a local file", async ({ mount }) => {
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
  await component.getByLabel("Or import a pairing file").setInputFiles({
    name: "rackio-pairing-bundle.txt",
    mimeType: "text/plain",
    buffer: Buffer.from("  rackio-pair:file-bundle  \n"),
  });
  await component.getByRole("dialog").getByRole("button", { name: "Pair machine" }).click();
  await expect.poll(() => submitted).toBe("rackio-pair:file-bundle");
});

test("shows daemon failure as an alert instead of an empty healthy fleet", async ({ mount }) => {
  const component = await mount(
    <Dashboard
      snapshot={{ daemon: "unavailable", nodes: [], message: "Agent socket not found." }}
    />,
  );
  await expect(component.getByRole("alert")).toContainText("Background monitoring is disconnected");
});

test("does not summarise an unreachable agent as a healthy rack", async ({ mount }) => {
  const component = await mount(
    <Dashboard
      snapshot={{ daemon: "unavailable", nodes: [], message: "Agent socket not found." }}
    />,
  );
  const summary = component.getByLabel("Rack summary");
  await expect(summary).not.toContainText("Healthy");
  await expect(summary).toContainText("Rack state");
  await expect(summary.locator("strong").first()).toHaveText("—");
});

test("does not summarise an empty connected rack as healthy", async ({ mount }) => {
  const component = await mount(
    <Dashboard snapshot={{ daemon: "connected", nodes: [], message: "No machines paired yet." }} />,
  );
  const summary = component.getByLabel("Rack summary");
  await expect(summary).not.toContainText("Healthy");
  await expect(summary.locator("strong").first()).toHaveText("0");
});

test("closes the pairing dialog with Escape and returns focus to its opener", async ({ mount }) => {
  const component = await mount(<Dashboard snapshot={snapshot} />);
  const opener = component.getByRole("button", { name: /pair machine/i });
  await opener.click();
  const dialog = component.getByRole("dialog", { name: /pair a machine/i });
  await expect(dialog).toBeVisible();
  await component.page().keyboard.press("Escape");
  await expect(dialog).toHaveCount(0);
  await expect(opener).toBeFocused();
});

test("does not abandon a pairing request that is already submitting", async ({ mount }) => {
  const component = await mount(
    <Dashboard snapshot={snapshot} pairing={{ state: "submitting" }} />,
  );
  await component.getByRole("button", { name: /pair machine/i }).click();
  const dialog = component.getByRole("dialog", { name: /pair a machine/i });
  await expect(dialog.getByRole("button", { name: "Cancel" })).toBeDisabled();
  await expect(dialog.getByRole("button", { name: "Pairing…" })).toBeDisabled();

  await component.page().keyboard.press("Escape");

  await expect(dialog).toBeVisible();
  await expect(dialog.locator(":focus")).toHaveCount(1);
});

test("keeps the selected pairing method and draft across a close and reopen", async ({ mount }) => {
  const component = await mount(<Dashboard snapshot={snapshot} />);
  const opener = component.getByRole("button", { name: /pair machine/i });
  await opener.click();
  const dialog = component.getByRole("dialog", { name: /pair a machine/i });
  await dialog.getByRole("textbox", { name: "Pairing bundle" }).fill("rackio-pair:draft");
  await dialog.getByRole("tab", { name: "Install over SSH" }).click();
  await dialog.getByLabel("Host").fill("server.test");

  await dialog.getByRole("button", { name: "Cancel" }).click();
  await expect(dialog).toHaveCount(0);
  await expect(opener).toBeFocused();
  await opener.click();

  await expect(dialog.getByRole("tab", { name: "Install over SSH" })).toHaveAttribute(
    "aria-selected",
    "true",
  );
  await expect(dialog.getByLabel("Host")).toHaveValue("");
  await dialog.getByRole("tab", { name: "Pairing bundle" }).click();
  await expect(dialog.getByRole("textbox", { name: "Pairing bundle" })).toHaveValue(
    "rackio-pair:draft",
  );
});

test("keeps Tab focus inside the modal pairing dialog", async ({ mount }) => {
  const component = await mount(<Dashboard snapshot={snapshot} />);
  await component.getByRole("button", { name: /pair machine/i }).click();
  const dialog = component.getByRole("dialog", { name: /pair a machine/i });
  for (let step = 0; step < 12; step += 1) {
    await component.page().keyboard.press("Tab");
    await expect(dialog.locator(":focus")).toHaveCount(1);
  }
});

test("exposes the pairing methods as a complete tab pattern", async ({ mount }) => {
  const component = await mount(<Dashboard snapshot={snapshot} />);
  await component.getByRole("button", { name: /pair machine/i }).click();
  const bundleTab = component.getByRole("tab", { name: "Pairing bundle" });
  await expect(bundleTab).toHaveAttribute("aria-controls", "pair-tabpanel");
  const panel = component.getByRole("tabpanel");
  await expect(panel).toHaveAttribute("aria-labelledby", "pair-tab-bundle");
  await bundleTab.focus();
  await component.page().keyboard.press("ArrowRight");
  await expect(component.getByRole("tab", { name: "Install over SSH" })).toHaveAttribute(
    "aria-selected",
    "true",
  );
  await expect(panel).toHaveAttribute("aria-labelledby", "pair-tab-ssh");
});

test("falls back to the normal window when tray integration is unavailable", async ({ mount }) => {
  const component = await mount(
    <Dashboard snapshot={snapshot} traySurface={traySurfaceStateRegistry.unavailable} />,
  );
  await expect(component.getByText("Tray unavailable")).toBeVisible();
  await expect(component.getByText("Studio Mac")).toBeVisible();
});
