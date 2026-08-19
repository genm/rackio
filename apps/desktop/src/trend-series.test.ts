import { describe, expect, it } from "vitest";

import { trendSeries } from "./trend-series";

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
});
