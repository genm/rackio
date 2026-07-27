import { bytes, percent } from "../format";
import { connectionPathRegistry, nodeStateRegistry } from "../state-registry";
import type { FleetNode } from "../types";
import { Sparkline } from "./Sparkline";

export function NodeCard({
  node,
  onViewHistory,
}: {
  node: FleetNode;
  onViewHistory?: (node: FleetNode) => void;
}) {
  const state = nodeStateRegistry[node.state];
  const path = connectionPathRegistry[node.path];
  return (
    <article className="node-card">
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
      <Sparkline values={node.history} label={`${node.name} CPU history`} />
      <dl className="metrics">
        <div>
          <dt>CPU</dt>
          <dd>{node.cpuPercent == null ? "—" : `${Math.round(node.cpuPercent)}%`}</dd>
        </div>
        <div>
          <dt>Memory</dt>
          <dd>{percent(node.memoryUsedBytes, node.memoryTotalBytes)}</dd>
        </div>
        <div>
          <dt>Disk</dt>
          <dd>{percent(node.diskUsedBytes, node.diskTotalBytes)}</dd>
        </div>
        <div>
          <dt>RTT</dt>
          <dd>{node.rttMs == null ? "—" : `${node.rttMs} ms`}</dd>
        </div>
      </dl>
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
