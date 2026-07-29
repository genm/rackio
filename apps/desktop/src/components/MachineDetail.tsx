import { bytes, percent } from "../format";
import { connectionPathRegistry, nodeStateRegistry } from "../state-registry";
import type { MachineDetailState } from "../types";
import { useModalDialog } from "../useModalDialog";
import { Sparkline } from "./Sparkline";

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
  const cpuHistory =
    detail.state === "ready"
      ? detail.points.flatMap((point) => (point.cpuPercent == null ? [] : [point.cpuPercent]))
      : [];
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
            <section className="history-chart" aria-label="24-hour CPU history">
              <div className="history-heading">
                <div>
                  <p className="eyebrow">LAST 24 HOURS</p>
                  <strong>CPU</strong>
                </div>
                <span>{detail.points.length} one-minute buckets</span>
              </div>
              <Sparkline values={cpuHistory} label={`${node.name} 24-hour CPU history`} />
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
