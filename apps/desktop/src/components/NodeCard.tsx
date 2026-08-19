import { ago, bytes, celsius, percent, shortDuration } from "../format";
import { connectionPathRegistry, nodeStateRegistry } from "../state-registry";
import type { FleetNode, TemperatureReading } from "../types";
import { TrendChart } from "./TrendChart";

/**
 * The agent streams one metric sample every two seconds (see
 * `apps/agent/src/runtime.rs`), so the card's trend window is derived from the
 * sample count rather than guessed. There are no timestamps in the live
 * snapshot to derive it from instead.
 */
const SAMPLE_INTERVAL_SECONDS = 2;

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
  const trendSpan = shortDuration((node.history.length - 1) * SAMPLE_INTERVAL_SECONDS);
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
        <span className="trend-title">CPU load</span>
        <span className="trend-now">
          {node.cpuPercent == null ? "" : `${Math.round(node.cpuPercent)}%`}
        </span>
      </div>
      <TrendChart
        values={node.history}
        label={`${node.name} CPU load over the last ${trendSpan}`}
        startLabel={node.history.length >= 2 ? `${trendSpan} ago` : undefined}
        endLabel={live ? "now" : "last contact"}
        emptyText="Collecting CPU samples…"
        muted={!live}
      />
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
          <dt>Temp</dt>
          {/* Titled with the sensor and how many it was the hottest of: the
              number alone cannot be told apart from a battery reading, and a
              machine without sensors must show "—" rather than 0 °C. */}
          <dd title={temperatureDetail(node.temperature)}>{celsius(node.temperature?.celsius)}</dd>
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
