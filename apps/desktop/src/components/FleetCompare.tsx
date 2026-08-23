import { shortDuration, timeOfDay } from "../format";
import { isLiveNodeState } from "../state-model";
import {
  type TrendLine,
  type TrendMetric,
  trendLines,
  trendMetricRegistry,
  trendScale,
} from "../trend-series";
import type { FleetNode } from "../types";
import { TrendChart } from "./TrendChart";

/**
 * One chart carrying every machine's line for a single metric.
 *
 * The per-machine cards answer "how is this machine"; the question a rack
 * owner actually opens the app with is "which machine is the odd one out",
 * and that comparison cannot be made across separately scaled cards.
 */
export function FleetCompare({
  nodes,
  metric,
  onMetricChange,
}: {
  nodes: FleetNode[];
  metric: TrendMetric;
  onMetricChange: (metric: TrendMetric) => void;
}) {
  const spec = trendMetricRegistry[metric];
  // Comparing a frozen sample with live samples makes an outage look like a
  // current outlier. Keep the comparison truthful by plotting live streams
  // only; the per-machine card retains the muted last-contact trend.
  const liveNodes = nodes.filter((node) => isLiveNodeState(node.state));
  const lines: TrendLine[] = [];
  const values: number[] = [];
  let firstMs: number | undefined;
  let lastMs: number | undefined;
  for (const node of liveNodes) {
    const series = trendLines(node.trend, metric);
    // Each machine contributes one line, so a two-line metric is flattened
    // with the series name kept alongside the machine it belongs to.
    for (const line of series.lines) {
      if (line.points.length === 0) continue;
      lines.push({
        name: spec.series.length > 1 ? `${node.name} ${line.name}` : node.name,
        points: line.points,
      });
    }
    values.push(...series.values);
    if (series.firstMs !== undefined) {
      firstMs = firstMs === undefined ? series.firstMs : Math.min(firstMs, series.firstMs);
    }
    if (series.lastMs !== undefined) {
      lastMs = lastMs === undefined ? series.lastMs : Math.max(lastMs, series.lastMs);
    }
  }
  const spanSeconds = firstMs !== undefined && lastMs !== undefined ? (lastMs - firstMs) / 1000 : 0;
  return (
    <section className="fleet-compare" aria-label="Compare machines">
      <div className="history-heading">
        <div>
          <p className="eyebrow">ALL MACHINES</p>
          <strong>{spec.chartTitle}</strong>
        </div>
        <div className="chart-toggle" aria-label="Comparison metric">
          {(Object.keys(trendMetricRegistry) as TrendMetric[]).map((option) => (
            <button
              key={option}
              type="button"
              aria-pressed={metric === option}
              onClick={() => onMetricChange(option)}
            >
              {trendMetricRegistry[option].label}
            </button>
          ))}
        </div>
      </div>
      <TrendChart
        lines={lines}
        scale={trendScale(spec.scale, values)}
        label={`${spec.chartTitle} across every machine over the last ${shortDuration(spanSeconds)}`}
        startLabel={values.length >= 2 ? `${shortDuration(spanSeconds)} ago` : undefined}
        endLabel="now"
        emptyText={`No live machine reports ${spec.label} yet`}
        formatTime={timeOfDay}
      />
    </section>
  );
}
