import { useState } from "react";

import { bytes, celsius, percent, timeOfDay } from "../format";
import { connectionPathRegistry, nodeStateRegistry } from "../state-model";
import { type TrendMetric, trendLines, trendMetricRegistry, trendScale } from "../trend-series";
import type { HistoryRange, MachineDetailState } from "../types";
import { useModalDialog } from "../useModalDialog";
import { temperatureDetail } from "./NodeCard";
import { TrendChart } from "./TrendChart";

/**
 * The history query reads the peer's one-minute buckets, which aggregate CPU,
 * memory, disk and temperature. Network stays out until the schema aggregates
 * it; RTT stays out for good, being the viewer's own connection measurement
 * and never written to the peer's storage. The card's live trend covers the
 * full metric registry either way.
 */
const historyMetrics: TrendMetric[] = ["cpu", "memory", "disk", "temp"];

const historyRanges: { hours: HistoryRange; label: string }[] = [
  { hours: 1, label: "1h" },
  { hours: 6, label: "6h" },
  { hours: 24, label: "24h" },
  // The agent retains one-minute buckets for seven days; a longer range would
  // return a window the peer cannot fill.
  { hours: 168, label: "7d" },
];

export function MachineDetail({
  detail,
  onClose,
  onRangeChange,
}: {
  detail: MachineDetailState;
  onClose: () => void;
  onRangeChange?: (hours: HistoryRange) => void;
}) {
  if (detail.state === "closed") return null;
  return <MachineDetailDialog detail={detail} onClose={onClose} onRangeChange={onRangeChange} />;
}

// A separate component so the modal hook mounts and unmounts with the dialog:
// that unmount is what restores focus to the card that opened it.
function MachineDetailDialog({
  detail,
  onClose,
  onRangeChange,
}: {
  detail: Exclude<MachineDetailState, { state: "closed" }>;
  onClose: () => void;
  onRangeChange?: (hours: HistoryRange) => void;
}) {
  const dialogRef = useModalDialog<HTMLElement>(onClose);
  const { node } = detail;
  const [metric, setMetric] = useState<TrendMetric>("cpu");
  const series =
    detail.state === "ready"
      ? trendLines(detail.points, metric)
      : { lines: [], values: [] as number[] };
  const spec = trendMetricRegistry[metric];
  const rangeLabel = historyRanges.find((range) => range.hours === detail.hours)?.label ?? "24h";
  return (
    <div className="dialog-backdrop">
      <section
        ref={dialogRef}
        className="machine-detail"
        role="dialog"
        aria-modal="true"
        aria-labelledby="machine-detail-title"
        tabIndex={-1}
      >
        <header>
          <div>
            <p className="eyebrow">{node.os}</p>
            <h2 id="machine-detail-title">{node.name}</h2>
          </div>
          <button type="button" className="secondary-button" onClick={onClose}>
            Close
          </button>
        </header>
        <div className="detail-badges">
          <span className={`badge tone-${nodeStateRegistry[node.state].tone}`}>
            {nodeStateRegistry[node.state].label}
          </span>
          <span className={`badge path-${node.path}`}>
            {connectionPathRegistry[node.path].label}
          </span>
          <span className="badge">{node.rttMs == null ? "RTT —" : `${node.rttMs} ms`}</span>
          <div className="chart-toggle detail-range" aria-label="History range">
            {historyRanges.map((range) => (
              <button
                key={range.hours}
                type="button"
                aria-pressed={detail.hours === range.hours}
                disabled={detail.state === "loading"}
                onClick={() => onRangeChange?.(range.hours)}
              >
                {range.label}
              </button>
            ))}
          </div>
        </div>
        {detail.state === "loading" ? (
          <p className="progress-message" role="status">
            <span className="spinner" aria-hidden="true" />
            Loading {rangeLabel} history from this machine…
          </p>
        ) : detail.state === "error" ? (
          <div className="history-message error-message" role="alert">
            <strong>History unavailable</strong>
            <p>{detail.message}</p>
          </div>
        ) : detail.points.length === 0 ? (
          <div className="history-message" role="status">
            <strong>No history in this range</strong>
            <p>Live metrics remain available. The machine returned no one-minute buckets.</p>
          </div>
        ) : (
          <>
            <section className="history-chart" aria-label="Machine history">
              <div className="history-heading">
                <div>
                  <p className="eyebrow">LAST {rangeLabel.toUpperCase()}</p>
                  <strong>{spec.chartTitle}</strong>
                </div>
                <div className="chart-toggle" aria-label="History metric">
                  {historyMetrics.map((option) => (
                    <button
                      key={option}
                      type="button"
                      aria-pressed={metric === option}
                      onClick={() => setMetric(option)}
                    >
                      {trendMetricRegistry[option].label}
                    </button>
                  ))}
                </div>
              </div>
              {/* The axis ends come from the readable buckets themselves, so a
                  partial range (a machine that was off overnight) is labelled
                  with the hours it actually covers rather than the range that
                  was requested. */}
              <TrendChart
                lines={series.lines}
                scale={trendScale(spec.scale, series.values)}
                label={`${node.name} ${rangeLabel} ${spec.label} history`}
                startLabel={series.firstMs === undefined ? undefined : timeOfDay(series.firstMs)}
                endLabel={series.lastMs === undefined ? undefined : timeOfDay(series.lastMs)}
                emptyText={`No ${spec.label} samples in this range`}
                formatTime={timeOfDay}
              />
              <p className="history-buckets">{detail.points.length} one-minute buckets</p>
            </section>
            <dl className="detail-metrics">
              <div>
                <dt>Latest CPU</dt>
                <dd>{node.cpuPercent == null ? "—" : `${Math.round(node.cpuPercent)}%`}</dd>
              </div>
              <div>
                <dt>Memory</dt>
                <dd>{percent(node.memoryUsedBytes, node.memoryTotalBytes)}</dd>
              </div>
              <div>
                <dt>Memory used</dt>
                <dd>{bytes(node.memoryUsedBytes)}</dd>
              </div>
              <div>
                <dt>Temperature</dt>
                <dd title={temperatureDetail(node.temperature)}>
                  {celsius(node.temperature?.celsius)}
                </dd>
              </div>
              <div>
                <dt>Hottest sensor</dt>
                {/* Naming the sensor keeps the number attributable; a machine
                    with none says so rather than showing a blank. */}
                <dd>{node.temperature?.label ?? "No sensor readable"}</dd>
              </div>
              <div>
                <dt>Connection</dt>
                <dd>{connectionPathRegistry[node.path].label}</dd>
              </div>
            </dl>
          </>
        )}
      </section>
    </div>
  );
}
