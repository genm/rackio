import { ago, bytes, shortDuration, timeOfDay, uptime } from "../format";
import {
  swapDetail,
  temperatureDetail,
  tileValues,
  unavailableTileValues,
} from "../machine-presentation";
import { connectionPathRegistry, isLiveNodeState, nodeStateRegistry } from "../state-model";
import { type TrendMetric, trendLines, trendMetricRegistry, trendScale } from "../trend-series";
import type { FleetNode } from "../types";
import { TrendChart } from "./TrendChart";

/**
 * States in which the metric stream is still delivering: values shown on the
 * card are current only in these states. Last-known samples remain available
 * to the muted trend for diagnosis, but are not rendered as current numbers.
 */
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
  const live = isLiveNodeState(node.state);
  const spec = trendMetricRegistry[metric];
  const series = trendLines(node.trend, metric);
  const spanSeconds =
    series.firstMs !== undefined && series.lastMs !== undefined
      ? (series.lastMs - series.firstMs) / 1000
      : 0;
  const values = live ? tileValues(node) : unavailableTileValues;
  const currentValue = values[metric] === "—" ? "" : values[metric];
  // A machine that streams samples but never reports this metric (a sensorless
  // host, the local machine's RTT) must say so instead of promising data.
  const emptyText =
    node.trend.length >= 2
      ? `No ${spec.label} readings on this machine`
      : `Collecting ${spec.label} samples…`;
  // Tiles whose "—" has more than one cause explain which one it is, so an
  // unavailable reading is never mistaken for an idle machine.
  const tileDetail = (tileMetric: TrendMetric) =>
    tileMetric === "temp"
      ? temperatureDetail(node.temperature)
      : tileMetric === "swap"
        ? swapDetail(node)
        : undefined;
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
        title={tileDetail(tileMetric)}
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
          {live
            ? `Memory ${bytes(node.memoryUsedBytes)} / ${bytes(node.memoryTotalBytes)}`
            : "Current memory unavailable"}
        </span>
        {/* Uptime is a card field rather than a trend tile, and that is not the
            trend rule being broken: the rule in `trend-series.ts` covers
            periodically sampled quantities, and uptime is not one. It is a
            rendering of a single fixed instant — the boot time — so a chart of
            it could only draw a straight ramp. What an operator reads from it
            (did this machine restart?) is in the one number. */}
        <span title="Time since this machine last booted">
          {live ? `Uptime ${uptime(node.uptimeSeconds)}` : "Current uptime unavailable"}
        </span>
        {/* The daemon derives stale/offline from age without attaching a
            detail string, so an unconditional "operational" fallback would
            claim a healthy collector for an unreachable machine. */}
        <span>
          {node.detail ??
            (node.state === "healthy"
              ? "Collectors operational"
              : `No detail reported · ${state.label}`)}
          {/* Keep the failure detail and contact age visible while current
              metric values remain deliberately unavailable. */}
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
