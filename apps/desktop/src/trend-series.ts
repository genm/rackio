import type { TrendPoint } from "./types";

/**
 * Every periodically displayed metric is a trend metric — a number the UI
 * shows on a cadence must also be plottable, without exception. Adding a tile
 * to the card means adding an entry here, never a static tile.
 */
export type TrendMetric = "cpu" | "memory" | "disk" | "temp" | "rtt";

export type TrendScaleKind = "percent" | "celsius" | "milliseconds";

function ratio(used?: number | null, total?: number | null): number | null {
  if (used == null || total == null || total <= 0) return null;
  return (used / total) * 100;
}

export const trendMetricRegistry: Record<
  TrendMetric,
  {
    label: string;
    chartTitle: string;
    scale: TrendScaleKind;
    read: (point: TrendPoint) => number | null | undefined;
  }
> = {
  cpu: {
    label: "CPU",
    chartTitle: "CPU load",
    scale: "percent",
    read: (point) => point.cpuPercent,
  },
  memory: {
    label: "Memory",
    chartTitle: "Memory load",
    scale: "percent",
    read: (point) => ratio(point.memoryUsedBytes, point.memoryTotalBytes),
  },
  disk: {
    label: "Disk",
    chartTitle: "Disk usage",
    scale: "percent",
    read: (point) => ratio(point.diskUsedBytes, point.diskTotalBytes),
  },
  temp: {
    label: "Temp",
    chartTitle: "Temperature",
    scale: "celsius",
    read: (point) => point.temperatureCelsius,
  },
  rtt: {
    label: "RTT",
    chartTitle: "RTT",
    scale: "milliseconds",
    read: (point) => point.rttMs,
  },
};

export interface TrendSeries {
  /** Values in the metric's own unit, oldest first. */
  values: number[];
  /** Timestamp of the first/last readable point; absent when none exist. */
  firstMs?: number;
  lastMs?: number;
}

/**
 * Project one metric out of a machine's timestamped points. Points where the
 * metric was unreadable are skipped rather than plotted as zero, and the
 * returned range covers only readable points so the axis never claims time the
 * line does not show.
 */
export function trendSeries(points: TrendPoint[], metric: TrendMetric): TrendSeries {
  const series: TrendSeries = { values: [] };
  for (const point of points) {
    const value = trendMetricRegistry[metric].read(point);
    if (value == null) continue;
    series.values.push(value);
    series.firstMs ??= point.timestampMs;
    series.lastMs = point.timestampMs;
  }
  return series;
}

export interface TrendScale {
  max: number;
  topLabel: string;
  midLabel: string;
}

/** Smallest 1/2/5×10ⁿ at or above the value, so axis tops stay readable. */
function niceCeiling(value: number): number {
  let magnitude = 10;
  for (;;) {
    for (const step of [1, 2, 5]) {
      const candidate = step * magnitude;
      if (candidate >= value) return candidate;
    }
    magnitude *= 10;
  }
}

/**
 * Bounded units get a fixed scale so a calm chart and a loaded chart can
 * never look alike; unbounded milliseconds are the exception and take the
 * smallest round ceiling that fits the data.
 */
export function trendScale(kind: TrendScaleKind, values: number[]): TrendScale {
  switch (kind) {
    case "percent":
      return { max: 100, topLabel: "100%", midLabel: "50" };
    case "celsius":
      // Hardware critical thresholds sit at or below roughly 100 °C, so the
      // fixed ceiling keeps "near the top" meaning "near critical".
      return { max: 100, topLabel: "100 °C", midLabel: "50" };
    case "milliseconds": {
      const max = niceCeiling(Math.max(...values, 1));
      return { max, topLabel: `${max} ms`, midLabel: `${max / 2}` };
    }
  }
}
