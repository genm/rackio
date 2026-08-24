import { bytes, celsius, percent } from "./format";
import type { TrendMetric } from "./trend-series";
import type { FleetNode, TemperatureReading } from "./types";

/**
 * Name the sensor the reading came from, and say how many sensors it was the
 * hottest of, so "the machine's temperature" stays checkable. The hardware's
 * own critical threshold is shown only when the OS reported one.
 */
export function temperatureDetail(temperature?: TemperatureReading | null): string {
  if (temperature == null) return "No temperature sensor is readable on this machine";
  const sensors =
    temperature.sensorCount > 1 ? ` · hottest of ${temperature.sensorCount} sensors` : "";
  const critical =
    temperature.criticalCelsius == null
      ? ""
      : ` · hardware critical ${Math.round(temperature.criticalCelsius)} °C`;
  return `${temperature.label}${sensors}${critical}`;
}

/**
 * Say which kind of "—" a swap tile is showing. A machine with swap disabled
 * and a machine whose swap could not be read both have no percentage, but they
 * are different facts and an operator acts differently on each.
 */
export function swapDetail(node: FleetNode): string {
  if (node.swapTotalBytes == null) return "No swap reading from this machine";
  if (node.swapTotalBytes === 0) return "No swap device on this machine";
  return `${bytes(node.swapUsedBytes)} of ${bytes(node.swapTotalBytes)} swap in use`;
}

/**
 * Say which filesystem the headline disk figure is, and how much of it is in
 * use. A percentage with no mount cannot be acted on when a machine has
 * several filesystems, and the alert an operator was notified about names one.
 */
export function diskDetail(node: FleetNode): string {
  const filesystems = node.filesystems ?? [];
  if (filesystems.length === 0) return "No filesystem reading from this machine";
  const [fullest] = filesystems;
  if (fullest === undefined) return "No filesystem reading from this machine";
  const others = filesystems.length > 1 ? ` · ${filesystems.length - 1} more on this machine` : "";
  return `${fullest.mount} · ${bytes(fullest.usedBytes)} of ${bytes(fullest.totalBytes)} used${others}`;
}

/** The latest value each tile reports, in the tile's own unit. */
export function tileValues(node: FleetNode): Record<TrendMetric, string> {
  return {
    cpu: node.cpuPercent == null ? "—" : `${Math.round(node.cpuPercent)}%`,
    memory: percent(node.memoryUsedBytes, node.memoryTotalBytes),
    // `percent` already reads a zero total as unavailable, which is exactly
    // right here: a machine with swap disabled has no percentage to report,
    // and "0%" would claim idle swap on a machine that has none.
    swap: percent(node.swapUsedBytes, node.swapTotalBytes),
    disk: percent(node.diskUsedBytes, node.diskTotalBytes),
    temp: celsius(node.temperature?.celsius),
    network:
      node.networkReceivedBytesPerSecond == null && node.networkSentBytesPerSecond == null
        ? "—"
        : `↓${bytes(node.networkReceivedBytesPerSecond)} ↑${bytes(node.networkSentBytesPerSecond)}`,
    rtt: node.rttMs == null ? "—" : `${node.rttMs} ms`,
  };
}

/** Non-live machines must not expose their last-known values as current data. */
export const unavailableTileValues: Record<TrendMetric, string> = {
  cpu: "—",
  memory: "—",
  swap: "—",
  disk: "—",
  temp: "—",
  network: "—",
  rtt: "—",
};
