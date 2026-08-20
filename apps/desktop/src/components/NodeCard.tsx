import { ago, bytes, shortDuration, timeOfDay } from "../format";
import { temperatureDetail, tileValues } from "../machine-presentation";
import { connectionPathRegistry, nodeStateRegistry } from "../state-model";
import { type TrendMetric, trendLines, trendMetricRegistry, trendScale } from "../trend-series";
import type { FleetNode } from "../types";
import { TrendChart } from "./TrendChart";

/**
 * States in which the metric stream is still delivering: everything shown is
 * current. Stale/offline/auth/incompatible machines keep their last numbers on
 * screen, but those must read as "as of last contact", not as live.
 */
const liveStates = new Set(["healthy", "warning", "degraded", "critical"]);

export function NodeCard({
  node,
  metric,
  onMetricChange,
  onViewHistory,
}: {
  node: FleetNode;
  metric: TrendMetric;
  onMetricChange: (metric: TrendMetric) => void;
  onViewHistory?: (node: FleetNode) => void;
}) {
  const state = nodeStateRegistry[node.state];
  const path = connectionPathRegistry[node.path];
  const live = liveStates.has(node.state);
  const spec = trendMetricRegistry[metric];
  const series = trendLines(node.trend, metric);
  const spanSeconds =
    series.firstMs !== undefined && series.lastMs !== undefined
      ? (series.lastMs - series.firstMs) / 1000
      : 0;
  const values = tileValues(node);
  const currentValue = values[metric] === "—" ? "" : values[metric];
  // A machine that streams samples but never reports this metric (a sensorless
  // host, the local machine's RTT) must say so instead of promising data.
  const emptyText =
    node.trend.length >= 2
      ? `No ${spec.label} readings on this machine`
      : `Collecting ${spec.label} samples…`;
  const metricTile = (tileMetric: TrendMetric) => (
    <button
      key={tileMetric}
      type="button"
      className="metric metric-selectable"
      aria-pressed={metric === tileMetric}
      title={`Show the ${trendMetricRegistry[tileMetric].label} trend`}
      onClick={() => onMetricChange(tileMetric)}
    >
      <span className="metric-label">{trendMetricRegistry[tileMetric].label}</span>
      <span
        className={`metric-value${tileMetric === "network" ? " metric-value-compact" : ""}`}
        title={tileMetric === "temp" ? temperatureDetail(node.temperature) : undefined}
      >
        {values[tileMetric]}
      </span>
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
        <span className="trend-title">{spec.chartTitle}</span>
        <span className="trend-now">{currentValue}</span>
      </div>
      <TrendChart
        lines={series.lines}
        scale={trendScale(spec.scale, series.values)}
        label={`${node.name} ${spec.chartTitle} over the last ${shortDuration(spanSeconds)}`}
        startLabel={series.values.length >= 2 ? `${shortDuration(spanSeconds)} ago` : undefined}
        endLabel={live ? "now" : "last contact"}
        emptyText={emptyText}
        muted={!live}
        // Only the threshold the hardware itself declares; Rackio never
        // invents one for a machine whose sensor layout it does not know.
        threshold={
          metric === "temp" && node.temperature?.criticalCelsius != null
            ? {
                value: node.temperature.criticalCelsius,
                label: `critical ${Math.round(node.temperature.criticalCelsius)} °C`,
              }
            : undefined
        }
        formatTime={timeOfDay}
      />
      <div className="metrics">
        {(Object.keys(trendMetricRegistry) as TrendMetric[]).map(metricTile)}
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
            : "Query this machine for its history"
        }
        onClick={() => onViewHistory?.(node)}
      >
        View history
      </button>
    </article>
  );
}
