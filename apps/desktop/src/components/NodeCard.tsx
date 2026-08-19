import { useState } from "react";

import { ago, bytes, celsius, percent, shortDuration } from "../format";
import { connectionPathRegistry, nodeStateRegistry } from "../state-registry";
import { type TrendMetric, trendMetricRegistry, trendSeries } from "../trend-series";
import type { FleetNode, TemperatureReading } from "../types";
import { TrendChart } from "./TrendChart";

/**
 * States in which the metric stream is still delivering: everything shown is
 * current. Stale/offline/auth/incompatible machines keep their last numbers on
 * screen, but those must read as "as of last contact", not as live.
 */
const liveStates = new Set(["healthy", "warning", "degraded", "critical"]);

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

export function NodeCard({
  node,
  onViewHistory,
}: {
  node: FleetNode;
  onViewHistory?: (node: FleetNode) => void;
}) {
  const state = nodeStateRegistry[node.state];
  const path = connectionPathRegistry[node.path];
  const live = liveStates.has(node.state);
  const [metric, setMetric] = useState<TrendMetric>("cpu");
  const series = trendSeries(node.trend, metric);
  const spanSeconds =
    series.firstMs !== undefined && series.lastMs !== undefined
      ? (series.lastMs - series.firstMs) / 1000
      : 0;
  const metricLabel = trendMetricRegistry[metric].label;
  const tileValues: Record<TrendMetric, string> = {
    cpu: node.cpuPercent == null ? "—" : `${Math.round(node.cpuPercent)}%`,
    memory: percent(node.memoryUsedBytes, node.memoryTotalBytes),
  };
  const currentValue = tileValues[metric] === "—" ? "" : tileValues[metric];
  const metricTile = (tileMetric: TrendMetric) => (
    <button
      type="button"
      className="metric metric-selectable"
      aria-pressed={metric === tileMetric}
      title={`Show the ${trendMetricRegistry[tileMetric].label} trend`}
      onClick={() => setMetric(tileMetric)}
    >
      <span className="metric-label">{trendMetricRegistry[tileMetric].label}</span>
      <span className="metric-value">{tileValues[tileMetric]}</span>
    </button>
  );
  return (
    <article className={`node-card${live ? "" : " node-card-not-live"}`}>
      <header>
        <div>
          <p className="eyebrow">{node.os}</p>
          <h2>{node.name}</h2>
        </div>
        <div className="badges">
          <span className={`badge tone-${state.tone}`}>{state.label}</span>
          <span className={`badge path-${node.path}`} title={path.description}>
            {path.label}
          </span>
        </div>
      </header>
      <div className="trend-head">
        <span className="trend-title">{metricLabel} load</span>
        <span className="trend-now">{currentValue}</span>
      </div>
      <TrendChart
        values={series.values}
        label={`${node.name} ${metricLabel} load over the last ${shortDuration(spanSeconds)}`}
        startLabel={series.values.length >= 2 ? `${shortDuration(spanSeconds)} ago` : undefined}
        endLabel={live ? "now" : "last contact"}
        emptyText={`Collecting ${metricLabel} samples…`}
        muted={!live}
      />
      <div className="metrics">
        {metricTile("cpu")}
        {metricTile("memory")}
        <div className="metric">
          <span className="metric-label">Disk</span>
          <span className="metric-value">{percent(node.diskUsedBytes, node.diskTotalBytes)}</span>
        </div>
        <div className="metric">
          <span className="metric-label">Temp</span>
          {/* Titled with the sensor and how many it was the hottest of: the
              number alone cannot be told apart from a battery reading, and a
              machine without sensors must show "—" rather than 0 °C. */}
          <span className="metric-value" title={temperatureDetail(node.temperature)}>
            {celsius(node.temperature?.celsius)}
          </span>
        </div>
        <div className="metric">
          <span className="metric-label">RTT</span>
          <span className="metric-value">{node.rttMs == null ? "—" : `${node.rttMs} ms`}</span>
        </div>
      </div>
      <footer>
        <span>
          Memory {bytes(node.memoryUsedBytes)} / {bytes(node.memoryTotalBytes)}
        </span>
        {/* The daemon derives stale/offline from age without attaching a
            detail string, so an unconditional "operational" fallback would
            claim a healthy collector for an unreachable machine. */}
        <span>
          {node.detail ??
            (node.state === "healthy"
              ? "Collectors operational"
              : `No detail reported · ${state.label}`)}
          {/* The failure detail must stay visible; the age is appended so the
              frozen numbers above are datable without hiding the cause. */}
          {!live && node.lastSeenMs !== undefined
            ? ` · last contact ${ago(node.lastSeenMs, Date.now())}`
            : null}
        </span>
      </footer>
      <button
        type="button"
        className="history-button"
        disabled={node.endpointId === undefined}
        title={
          node.endpointId === undefined
            ? "History is unavailable until this machine has an endpoint identity"
            : "Query this machine for its 24-hour history"
        }
        onClick={() => onViewHistory?.(node)}
      >
        View 24-hour history
      </button>
    </article>
  );
}
