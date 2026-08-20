import { describe, expect, it } from "vitest";

import { temperatureDetail, tileValues } from "./machine-presentation";
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

describe("tileValues", () => {
  it("renders absent metrics as unavailable instead of healthy zeroes", () => {
    expect(tileValues(baseNode)).toEqual({
      cpu: "—",
      memory: "—",
      disk: "—",
      temp: "—",
      network: "—",
      rtt: "—",
    });
  });

  it("preserves every metric unit and partial network direction", () => {
    expect(
      tileValues({
        ...baseNode,
        cpuPercent: 42.6,
        memoryUsedBytes: 512,
        memoryTotalBytes: 1_024,
        diskUsedBytes: 768,
        diskTotalBytes: 1_024,
        temperature: { label: "CPU", celsius: 61.6, sensorCount: 1 },
        networkReceivedBytesPerSecond: 1_024,
        rttMs: 18,
      }),
    ).toEqual({
      cpu: "43%",
      memory: "50%",
      disk: "75%",
      temp: "62 °C",
      network: "↓1 KiB ↑—",
      rtt: "18 ms",
    });
  });
});
