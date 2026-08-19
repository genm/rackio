import type { TrendPoint } from "./types";

export type TrendMetric = "cpu" | "memory";

export const trendMetricRegistry: Record<TrendMetric, { label: string }> = {
  cpu: { label: "CPU" },
  memory: { label: "Memory" },
};

export interface TrendSeries {
  /** Percentages on a 0–100 scale, oldest first. */
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
    const value =
      metric === "cpu"
        ? point.cpuPercent
        : point.memoryUsedBytes != null &&
            point.memoryTotalBytes != null &&
            point.memoryTotalBytes > 0
          ? (point.memoryUsedBytes / point.memoryTotalBytes) * 100
          : null;
    if (value == null) continue;
    series.values.push(value);
    series.firstMs ??= point.timestampMs;
    series.lastMs = point.timestampMs;
  }
  return series;
}
