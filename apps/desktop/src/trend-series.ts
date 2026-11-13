import { bytesPerSecond } from "./format";
import type { TrendPoint } from "./types";

/**
 * Every periodically *sampled* quantity is a trend metric — a level the UI
 * shows on a cadence must also be plottable, without exception. Adding a
 * metric tile to the card means adding an entry here, never a static tile.
 *
 * The rule is about sampled quantities, and that carves out one case rather
 * than being broken by it: uptime is a card field, not a series. It is not
 * sampled — it is a rendering of one fixed instant, the boot time — so a chart
 * of it would draw the clock as a straight ramp and say nothing the single
 * number does not. Anything the machine actually re-measures each cycle
 * belongs here.
 */
export type TrendMetric = "cpu" | "memory" | "swap" | "disk" | "temp" | "network" | "rtt";

export type TrendScaleKind = "percent" | "celsius" | "milliseconds" | "bytesPerSecond";

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
    /**
     * One entry per plotted line. Most metrics have a single line; network is
     * two, because received and sent are separate quantities that must not be
     * summed into one misleading number.
     */
    series: { name: string; read: (point: TrendPoint) => number | null | undefined }[];
  }
> = {
  cpu: {
    label: "CPU",
    chartTitle: "CPU load",
    scale: "percent",
    series: [{ name: "CPU", read: (point) => point.cpuPercent }],
  },
  memory: {
    label: "Memory",
    chartTitle: "Memory load",
    scale: "percent",
    series: [
      { name: "Memory", read: (point) => ratio(point.memoryUsedBytes, point.memoryTotalBytes) },
    ],
  },
  swap: {
    label: "Swap",
    chartTitle: "Swap usage",
    scale: "percent",
    // A machine with swap disabled reports a zero total, and `ratio` reads
    // that as unreadable rather than as 0 %: the chart says the machine has no
    // swap to plot instead of drawing a flat, healthy-looking floor.
    series: [{ name: "Swap", read: (point) => ratio(point.swapUsedBytes, point.swapTotalBytes) }],
  },
  disk: {
    label: "Disk",
    chartTitle: "Disk usage",
    scale: "percent",
    series: [{ name: "Disk", read: (point) => ratio(point.diskUsedBytes, point.diskTotalBytes) }],
  },
  temp: {
    label: "Temp",
    chartTitle: "Temperature",
    scale: "celsius",
    series: [{ name: "Temp", read: (point) => point.temperatureCelsius }],
  },
  network: {
    label: "Net",
    chartTitle: "Network throughput",
    scale: "bytesPerSecond",
    series: [
      { name: "Received", read: (point) => point.networkReceivedBytesPerSecond },
      { name: "Sent", read: (point) => point.networkSentBytesPerSecond },
    ],
  },
  rtt: {
    label: "RTT",
    chartTitle: "RTT",
    scale: "milliseconds",
    series: [{ name: "RTT", read: (point) => point.rttMs }],
  },
};

export interface TrendDatum {
  timestampMs: number;
  value: number;
}

export interface TrendLine {
  name: string;
  points: TrendDatum[];
}

export interface TrendLines {
  lines: TrendLine[];
  /** Timestamps of the first and last readable point across every line. */
  firstMs?: number;
  lastMs?: number;
  /** Every readable value, for scales that must fit the data. */
  values: number[];
}

/**
 * Project one metric's lines out of a machine's timestamped points. Points
 * where a line was unreadable are skipped rather than plotted as zero, and the
 * reported range covers only readable points so the axis never claims time the
 * lines do not show.
 */
export function trendLines(points: TrendPoint[], metric: TrendMetric): TrendLines {
  const result: TrendLines = { lines: [], values: [] };
  for (const series of trendMetricRegistry[metric].series) {
    const line: TrendLine = { name: series.name, points: [] };
    for (const point of points) {
      const value = series.read(point);
      if (value == null) continue;
      line.points.push({ timestampMs: point.timestampMs, value });
      result.values.push(value);
      result.firstMs =
        result.firstMs === undefined
          ? point.timestampMs
          : Math.min(result.firstMs, point.timestampMs);
      result.lastMs =
        result.lastMs === undefined
          ? point.timestampMs
          : Math.max(result.lastMs, point.timestampMs);
    }
    result.lines.push(line);
  }
  return result;
}

export interface TrendScale {
  max: number;
  topLabel: string;
  midLabel: string;
  format: (value: number) => string;
}

/** Smallest 1/2/5×10ⁿ at or above the value, so axis tops stay readable. */
function niceCeiling(value: number, base: number): number {
  let magnitude = base;
  for (;;) {
    for (const step of [1, 2, 5]) {
      const candidate = step * magnitude;
      if (candidate >= value) return candidate;
    }
    magnitude *= 10;
  }
}

/**
 * Bounded units get a fixed scale so a calm chart and a loaded chart can never
 * look alike; unbounded units are the exception and take the smallest round
 * ceiling that fits the data.
 */
export function trendScale(kind: TrendScaleKind, values: number[]): TrendScale {
  switch (kind) {
    case "percent":
      return {
        max: 100,
        topLabel: "100%",
        midLabel: "50",
        format: (value) => `${Math.round(value)}%`,
      };
    case "celsius":
      // Hardware critical thresholds sit at or below roughly 100 °C, so the
      // fixed ceiling keeps "near the top" meaning "near critical".
      return {
        max: 100,
        topLabel: "100 °C",
        midLabel: "50",
        format: (value) => `${Math.round(value)} °C`,
      };
    case "milliseconds": {
      const max = niceCeiling(Math.max(...values, 1), 10);
      return {
        max,
        topLabel: `${max} ms`,
        midLabel: `${max / 2}`,
        format: (value) => `${Math.round(value)} ms`,
      };
    }
    case "bytesPerSecond": {
      const max = niceCeiling(Math.max(...values, 1), 1_024);
      return {
        max,
        topLabel: bytesPerSecond(max),
        midLabel: bytesPerSecond(max / 2),
        format: bytesPerSecond,
      };
    }
  }
}

/**
 * Milliseconds beyond which two consecutive samples are treated as separate
 * runs rather than one continuous line. Derived from the series' own typical
 * spacing, so it holds for both two-second live samples and one-minute
 * buckets; without it a machine that was offline for an hour would be drawn
 * as a straight line across the outage it did not report.
 */
export function gapThresholdMs(points: TrendDatum[]): number {
  if (points.length < 3) return Number.POSITIVE_INFINITY;
  const deltas: number[] = [];
  for (let index = 1; index < points.length; index += 1) {
    deltas.push(points[index].timestampMs - points[index - 1].timestampMs);
  }
  deltas.sort((left, right) => left - right);
  const median = deltas[Math.floor(deltas.length / 2)];
  return median > 0 ? median * 3 : Number.POSITIVE_INFINITY;
}
