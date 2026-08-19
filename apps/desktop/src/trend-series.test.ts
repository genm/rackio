import { describe, expect, it } from "vitest";

import { trendScale, trendSeries } from "./trend-series";

const points = [
  { timestampMs: 1_000, cpuPercent: 20, memoryUsedBytes: 4, memoryTotalBytes: 16 },
  { timestampMs: 3_000, cpuPercent: null, memoryUsedBytes: 8, memoryTotalBytes: 16 },
  { timestampMs: 5_000, cpuPercent: 40, memoryUsedBytes: null, memoryTotalBytes: 16 },
];

describe("trendSeries", () => {
  it("projects CPU and skips unreadable points without plotting zeros", () => {
    expect(trendSeries(points, "cpu")).toEqual({
      values: [20, 40],
      firstMs: 1_000,
      lastMs: 5_000,
    });
  });

  it("derives memory percentages and bounds the range to readable points", () => {
    // The last point has no memory reading, so the axis must not claim the
    // series reaches 5s.
    expect(trendSeries(points, "memory")).toEqual({
      values: [25, 50],
      firstMs: 1_000,
      lastMs: 3_000,
    });
  });

  it("treats a zero memory total as unreadable rather than dividing by it", () => {
    const zeroTotal = [
      { timestampMs: 1_000, cpuPercent: 1, memoryUsedBytes: 4, memoryTotalBytes: 0 },
    ];
    expect(trendSeries(zeroTotal, "memory")).toEqual({ values: [] });
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
    expect(trendSeries(rich, "disk").values).toEqual([30]);
    expect(trendSeries(rich, "temp").values).toEqual([61.5]);
    expect(trendSeries(rich, "rtt").values).toEqual([8]);
  });
});

describe("trendScale", () => {
  it("keeps bounded units on a fixed ceiling regardless of the data", () => {
    expect(trendScale("percent", [3, 5]).max).toBe(100);
    expect(trendScale("celsius", [40]).max).toBe(100);
  });

  it("gives milliseconds the smallest round ceiling that fits", () => {
    expect(trendScale("milliseconds", [8, 43])).toEqual({
      max: 50,
      topLabel: "50 ms",
      midLabel: "25",
    });
    expect(trendScale("milliseconds", [111]).max).toBe(200);
    // An all-quiet series still gets a positive ceiling instead of NaN.
    expect(trendScale("milliseconds", []).max).toBe(10);
  });
});
