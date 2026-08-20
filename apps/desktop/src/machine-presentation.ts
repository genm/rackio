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

/** The latest value each tile reports, in the tile's own unit. */
export function tileValues(node: FleetNode): Record<TrendMetric, string> {
  return {
    cpu: node.cpuPercent == null ? "—" : `${Math.round(node.cpuPercent)}%`,
    memory: percent(node.memoryUsedBytes, node.memoryTotalBytes),
    disk: percent(node.diskUsedBytes, node.diskTotalBytes),
    temp: celsius(node.temperature?.celsius),
    network:
      node.networkReceivedBytesPerSecond == null && node.networkSentBytesPerSecond == null
        ? "—"
        : `↓${bytes(node.networkReceivedBytesPerSecond)} ↑${bytes(node.networkSentBytesPerSecond)}`,
    rtt: node.rttMs == null ? "—" : `${node.rttMs} ms`,
  };
}
