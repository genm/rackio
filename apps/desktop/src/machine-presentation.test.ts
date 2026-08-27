import { describe, expect, it } from "vitest";

import { swapDetail, temperatureDetail, tileValues } from "./machine-presentation";
import type { FleetNode } from "./types";

const baseNode: FleetNode = {
  id: "presentation-node",
  name: "Presentation Server",
  os: "Test OS",
  state: "healthy",
  path: "unknown",
  trend: [],
};

describe("temperatureDetail", () => {
  it("keeps an unreadable sensor explicit instead of describing a zero", () => {
    expect(temperatureDetail()).toBe("No temperature sensor is readable on this machine");
  });

  it("names the hottest sensor, count, and hardware-owned critical threshold", () => {
    expect(
      temperatureDetail({
        label: "PMU tdie8",
        celsius: 61.4,
        criticalCelsius: 94.6,
        sensorCount: 41,
      }),
    ).toBe("PMU tdie8 · hottest of 41 sensors · hardware critical 95 °C");
  });
});

describe("swapDetail", () => {
  it("tells an absent swap reading apart from a machine that has no swap", () => {
    expect(swapDetail(baseNode)).toBe("No swap reading from this machine");
    expect(swapDetail({ ...baseNode, swapUsedBytes: 0, swapTotalBytes: 0 })).toBe(
      "No swap device on this machine",
    );
  });

  it("quantifies a real swap reading rather than leaving the percentage bare", () => {
    expect(
      swapDetail({ ...baseNode, swapUsedBytes: 2_147_483_648, swapTotalBytes: 8_589_934_592 }),
    ).toBe("2.0 GiB of 8.0 GiB swap in use");
  });
});

describe("tileValues", () => {
  it("renders absent metrics as unavailable instead of healthy zeroes", () => {
    expect(tileValues(baseNode)).toEqual({
      cpu: "—",
      memory: "—",
      swap: "—",
      disk: "—",
      temp: "—",
      network: "—",
      rtt: "—",
    });
  });

  it("reads a machine with no swap device as unavailable, never as 0%", () => {
    // Swap disabled is a real reading of zero capacity. The percentage it
    // implies does not exist, so the tile must not report an idle-looking 0%.
    expect(tileValues({ ...baseNode, swapUsedBytes: 0, swapTotalBytes: 0 }).swap).toBe("—");
  });

  it("preserves every metric unit and partial network direction", () => {
    expect(
      tileValues({
        ...baseNode,
        cpuPercent: 42.6,
        memoryUsedBytes: 512,
        memoryTotalBytes: 1_024,
        swapUsedBytes: 256,
        swapTotalBytes: 1_024,
        diskUsedBytes: 768,
        diskTotalBytes: 1_024,
        temperature: { label: "CPU", celsius: 61.6, sensorCount: 1 },
        networkReceivedBytesPerSecond: 1_024,
        rttMs: 18,
      }),
    ).toEqual({
      cpu: "43%",
      memory: "50%",
      swap: "25%",
      disk: "75%",
      temp: "62 °C",
      network: "↓1 KiB ↑—",
      rtt: "18 ms",
    });
  });
});
