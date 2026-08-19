import { useState } from "react";

import { bytes, celsius, percent, timeOfDay } from "../format";
import { connectionPathRegistry, nodeStateRegistry } from "../state-registry";
import { type TrendMetric, trendMetricRegistry, trendScale, trendSeries } from "../trend-series";

/**
 * The 24-hour query reads the peer's one-minute buckets, which aggregate
 * CPU, memory, disk and temperature. RTT stays out: it is the viewer's own
 * connection measurement, never written to the peer's storage, so there is
 * nothing on the peer side for a wider schema to aggregate.
 */
const historyMetrics: TrendMetric[] = ["cpu", "memory", "disk", "temp"];
import type { MachineDetailState } from "../types";
import { useModalDialog } from "../useModalDialog";
import { temperatureDetail } from "./NodeCard";
import { TrendChart } from "./TrendChart";

export function MachineDetail({
  detail,
  onClose,
}: {
  detail: MachineDetailState;
  onClose: () => void;
}) {
  if (detail.state === "closed") return null;
  return <MachineDetailDialog detail={detail} onClose={onClose} />;
}

// A separate component so the modal hook mounts and unmounts with the dialog:
// that unmount is what restores focus to the card that opened it.
function MachineDetailDialog({
  detail,
  onClose,
}: {
  detail: Exclude<MachineDetailState, { state: "closed" }>;
  onClose: () => void;
}) {
  const dialogRef = useModalDialog<HTMLElement>(onClose);
  const { node } = detail;
  const [metric, setMetric] = useState<TrendMetric>("cpu");
  const series = detail.state === "ready" ? trendSeries(detail.points, metric) : { values: [] };
  const spec = trendMetricRegistry[metric];
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
        </div>
        {detail.state === "loading" ? (
          <p className="progress-message" role="status">
            <span className="spinner" aria-hidden="true" />
            Loading 24-hour history from this machine…
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
            <section className="history-chart" aria-label="24-hour history">
              <div className="history-heading">
                <div>
                  <p className="eyebrow">LAST 24 HOURS</p>
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
                  with the hours it actually covers rather than a hardcoded
                  "24h ago". */}
              <TrendChart
                values={series.values}
                scale={trendScale(spec.scale, series.values)}
                label={`${node.name} 24-hour ${spec.label} history`}
                startLabel={series.firstMs === undefined ? undefined : timeOfDay(series.firstMs)}
                endLabel={series.lastMs === undefined ? undefined : timeOfDay(series.lastMs)}
                emptyText={`No ${spec.label} samples in this range`}
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
