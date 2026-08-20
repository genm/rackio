import { describe, expect, it } from "vitest";

import { gapThresholdMs, trendLines, trendScale } from "./trend-series";

const points = [
  { timestampMs: 1_000, cpuPercent: 20, memoryUsedBytes: 4, memoryTotalBytes: 16 },
  { timestampMs: 3_000, cpuPercent: null, memoryUsedBytes: 8, memoryTotalBytes: 16 },
  { timestampMs: 5_000, cpuPercent: 40, memoryUsedBytes: null, memoryTotalBytes: 16 },
];

describe("trendLines", () => {
  it("projects CPU with its timestamps and skips unreadable points", () => {
    expect(trendLines(points, "cpu")).toEqual({
      lines: [
        {
          name: "CPU",
          points: [
            { timestampMs: 1_000, value: 20 },
            { timestampMs: 5_000, value: 40 },
          ],
        },
      ],
      values: [20, 40],
      firstMs: 1_000,
      lastMs: 5_000,
    });
  });

  it("derives memory percentages and bounds the range to readable points", () => {
    // The last point has no memory reading, so the axis must not claim the
    // series reaches 5s.
    const memory = trendLines(points, "memory");
    expect(memory.values).toEqual([25, 50]);
    expect(memory.lastMs).toBe(3_000);
  });

  it("treats a zero memory total as unreadable rather than dividing by it", () => {
    const zeroTotal = [
      { timestampMs: 1_000, cpuPercent: 1, memoryUsedBytes: 4, memoryTotalBytes: 0 },
    ];
    expect(trendLines(zeroTotal, "memory").values).toEqual([]);
  });

  it("keeps network received and sent as two lines instead of one sum", () => {
    const network = trendLines(
      [
        {
          timestampMs: 1_000,
          networkReceivedBytesPerSecond: 2_048,
          networkSentBytesPerSecond: 512,
        },
      ],
      "network",
    );
    expect(network.lines.map((line) => line.name)).toEqual(["Received", "Sent"]);
    expect(network.lines[0].points[0].value).toBe(2_048);
    expect(network.lines[1].points[0].value).toBe(512);
  });

  it("projects disk, temperature, and RTT like any other metric", () => {
    const rich = [
      {
        timestampMs: 1_000,
        diskUsedBytes: 30,
        diskTotalBytes: 100,
        temperatureCelsius: 61.5,
        rttMs: 8,
      },
    ];
    expect(trendLines(rich, "disk").values).toEqual([30]);
    expect(trendLines(rich, "temp").values).toEqual([61.5]);
    expect(trendLines(rich, "rtt").values).toEqual([8]);
  });
});

describe("gapThresholdMs", () => {
  it("derives the break point from the series' own spacing", () => {
    const even = [0, 2_000, 4_000, 6_000].map((timestampMs) => ({ timestampMs, value: 1 }));
    expect(gapThresholdMs(even)).toBe(6_000);
  });

  it("never breaks a series too short to have a typical spacing", () => {
    expect(gapThresholdMs([{ timestampMs: 0, value: 1 }])).toBe(Number.POSITIVE_INFINITY);
  });

  it("is not dragged out by a single long outage", () => {
    // One hour-long hole among two-second samples must still leave the
    // threshold near the two-second spacing, or the hole would be drawn as a
    // continuous line.
    const withOutage = [0, 2_000, 4_000, 3_604_000, 3_606_000].map((timestampMs) => ({
      timestampMs,
      value: 1,
    }));
    expect(gapThresholdMs(withOutage)).toBe(6_000);
  });
});

describe("trendScale", () => {
  it("keeps bounded units on a fixed ceiling regardless of the data", () => {
    expect(trendScale("percent", [3, 5]).max).toBe(100);
    expect(trendScale("celsius", [40]).max).toBe(100);
  });

  it("gives milliseconds the smallest round ceiling that fits", () => {
    const scale = trendScale("milliseconds", [8, 43]);
    expect(scale.max).toBe(50);
    expect(scale.topLabel).toBe("50 ms");
    expect(trendScale("milliseconds", [111]).max).toBe(200);
    // An all-quiet series still gets a positive ceiling instead of NaN.
    expect(trendScale("milliseconds", []).max).toBe(10);
  });

  it("scales throughput in binary steps and labels it per second", () => {
    const scale = trendScale("bytesPerSecond", [3_000]);
    expect(scale.max).toBe(5_120);
    expect(scale.topLabel).toBe("5 KiB/s");
    expect(scale.format(2_048)).toBe("2 KiB/s");
  });
});
