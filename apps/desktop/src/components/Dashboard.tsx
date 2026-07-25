import { nodeStateRegistry, worstState } from "../state-registry";
import type { FleetSnapshot } from "../types";
import { NodeCard } from "./NodeCard";

export function Dashboard({ snapshot }: { snapshot: FleetSnapshot }) {
  const fleetState =
    snapshot.nodes.length > 0 ? worstState(snapshot.nodes.map((node) => node.state)) : "healthy";
  return (
    <main>
      <header className="topbar">
        <div className="brand">
          <span className={`pulse tone-${nodeStateRegistry[fleetState].tone}`} aria-hidden="true" />
          <div>
            <p className="eyebrow">PRIVATE P2P FLEET</p>
            <h1>Tray Monitor</h1>
          </div>
        </div>
        <button
          type="button"
          className="pair-button"
          disabled
          title="Pairing is available through the CLI in this preview"
        >
          <span aria-hidden="true">＋</span> Pair node
        </button>
      </header>
      <section className="summary" aria-label="Fleet summary">
        <div>
          <strong>{snapshot.nodes.length}</strong>
          <span>Nodes</span>
        </div>
        <div>
          <strong>{nodeStateRegistry[fleetState].label}</strong>
          <span>Fleet state</span>
        </div>
        <div>
          <strong>{snapshot.nodes.filter((node) => node.path === "relayed").length}</strong>
          <span>Relayed</span>
        </div>
        <p>Metrics stay on your nodes. No account. No central database.</p>
      </section>
      {snapshot.daemon === "unavailable" ? (
        <section className="empty-state alert-state" role="alert">
          <p className="eyebrow">AGENT UNAVAILABLE</p>
          <h2>Background monitoring is disconnected</h2>
          <p>{snapshot.message}</p>
          <code>tray-monitor daemon</code>
        </section>
      ) : snapshot.nodes.length === 0 ? (
        <section className="empty-state">
          <p className="eyebrow">READY TO CONNECT</p>
          <h2>Your private fleet starts here</h2>
          <p>{snapshot.message ?? "Open a pairing window on another node to begin."}</p>
        </section>
      ) : (
        <section className="node-grid" aria-label="Monitored nodes">
          {snapshot.nodes.map((node) => (
            <NodeCard key={node.id} node={node} />
          ))}
        </section>
      )}
    </main>
  );
}
