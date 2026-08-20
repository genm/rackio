import { describe, expect, it } from "vitest";

import { trendCoordinates, trendScale, trendSeries } from "./trend-series";

const points = [
  { timestampMs: 1_000, cpuPercent: 20, memoryUsedBytes: 4, memoryTotalBytes: 16 },
  { timestampMs: 3_000, cpuPercent: null, memoryUsedBytes: 8, memoryTotalBytes: 16 },
  { timestampMs: 5_000, cpuPercent: 40, memoryUsedBytes: null, memoryTotalBytes: 16 },
];

describe("trendSeries", () => {
  it("projects CPU and skips unreadable points without plotting zeros", () => {
    expect(trendSeries(points, "cpu")).toEqual({
      samples: [
        { value: 20, timestampMs: 1_000 },
        { value: 40, timestampMs: 5_000 },
      ],
      firstMs: 1_000,
      lastMs: 5_000,
    });
  });

  it("derives memory percentages and bounds the range to readable points", () => {
    // The last point has no memory reading, so the axis must not claim the
    // series reaches 5s.
    expect(trendSeries(points, "memory")).toEqual({
      samples: [
        { value: 25, timestampMs: 1_000 },
        { value: 50, timestampMs: 3_000 },
      ],
      firstMs: 1_000,
      lastMs: 3_000,
    });
  });

  it("treats a zero memory total as unreadable rather than dividing by it", () => {
    const zeroTotal = [
      { timestampMs: 1_000, cpuPercent: 1, memoryUsedBytes: 4, memoryTotalBytes: 0 },
    ];
    expect(trendSeries(zeroTotal, "memory")).toEqual({ samples: [] });
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
    expect(trendSeries(rich, "disk").samples).toEqual([{ value: 30, timestampMs: 1_000 }]);
    expect(trendSeries(rich, "temp").samples).toEqual([{ value: 61.5, timestampMs: 1_000 }]);
    expect(trendSeries(rich, "rtt").samples).toEqual([{ value: 8, timestampMs: 1_000 }]);
  });

  it("projects received and sent network throughput as independent metrics", () => {
    // A history payload as `machine_history` ships it: a minute where only
    // the outbound direction was readable must not fabricate an inbound 0.
    const historyPayload = [
      {
        timestampMs: 1_000,
        networkReceivedBytesPerSecond: 12_000,
        networkSentBytesPerSecond: 3_000,
      },
      { timestampMs: 2_000, networkReceivedBytesPerSecond: null, networkSentBytesPerSecond: 1_500 },
    ];
    expect(trendSeries(historyPayload, "netRx").samples).toEqual([
      { value: 12_000, timestampMs: 1_000 },
    ]);
    expect(trendSeries(historyPayload, "netTx").samples).toEqual([
      { value: 3_000, timestampMs: 1_000 },
      { value: 1_500, timestampMs: 2_000 },
    ]);
  });
});

describe("trendCoordinates", () => {
  it("spaces points by elapsed time, so a gap renders as a wider span", () => {
    // Ticks land every 2s except one metric misses the 5s tick, leaving a 4s
    // gap between the 3s and 7s samples inside an 8s-wide series.
    const samples = [
      { value: 10, timestampMs: 0 },
      { value: 20, timestampMs: 3_000 },
      { value: 30, timestampMs: 7_000 },
      { value: 40, timestampMs: 8_000 },
    ];
    const coordinates = trendCoordinates(samples, 100);
    expect(coordinates[0].x).toBe(0);
    expect(coordinates[1].x).toBeCloseTo(37.5); // 3s / 8s
    // The point right after the gap sits at 7s / 8s, not evenly spaced by index.
    expect(coordinates[2].x).toBeCloseTo(87.5);
    expect(coordinates[3].x).toBe(100);
  });

  it("falls back to even index spacing when every sample shares a timestamp", () => {
    const samples = [
      { value: 10, timestampMs: 5_000 },
      { value: 20, timestampMs: 5_000 },
      { value: 30, timestampMs: 5_000 },
    ];
    expect(trendCoordinates(samples, 100).map((c) => c.x)).toEqual([0, 50, 100]);
  });

  it("places a single sample without dividing by zero", () => {
    expect(trendCoordinates([{ value: 10, timestampMs: 5_000 }], 100)).toEqual([{ x: 0, y: 90 }]);
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

  it("gives a byte rate a data-derived ceiling labelled as a rate, not a fixed percentage", () => {
    expect(trendScale("byteRate", [1_100_000, 400_000])).toEqual({
      max: 2_000_000,
      topLabel: "1.9 MiB/s",
      midLabel: "977 KiB/s",
    });
  });
});
