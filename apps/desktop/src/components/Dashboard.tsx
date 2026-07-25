import { useState } from "react";

import { nodeStateRegistry, sshBootstrapStateRegistry, worstState } from "../state-registry";
import type {
  FleetSnapshot,
  PairingStatus,
  SshBootstrapInput,
  SshBootstrapStatus,
  SshTarget,
} from "../types";
import { NodeCard } from "./NodeCard";
import { SshBootstrapForm } from "./SshBootstrapForm";

export function Dashboard({
  snapshot,
  pairing = { state: "idle" },
  sshBootstrap = sshBootstrapStateRegistry.editing,
  onPair = async () => undefined,
  onInspectSshHost = async () => undefined,
  onInstallViaSsh = async () => undefined,
}: {
  snapshot: FleetSnapshot;
  pairing?: PairingStatus;
  sshBootstrap?: SshBootstrapStatus;
  onPair?: (bundle: string) => Promise<void>;
  onInspectSshHost?: (target: SshTarget) => Promise<void>;
  onInstallViaSsh?: (input: SshBootstrapInput) => Promise<void>;
}) {
  const [pairingOpen, setPairingOpen] = useState(false);
  const [pairingMethod, setPairingMethod] = useState<"bundle" | "ssh">("bundle");
  const [bundle, setBundle] = useState("");
  const fleetState =
    snapshot.nodes.length > 0 ? worstState(snapshot.nodes.map((node) => node.state)) : "healthy";
  const submitPairing = async (event: React.FormEvent) => {
    event.preventDefault();
    const normalized = bundle.trim();
    if (normalized.length === 0) return;
    try {
      await onPair(normalized);
      setBundle("");
      setPairingOpen(false);
    } catch {
      // The owning App surfaces the rejected operation through pairing state.
    }
  };
  return (
    <main>
      <header className="topbar">
        <div className="brand">
          <span className={`pulse tone-${nodeStateRegistry[fleetState].tone}`} aria-hidden="true" />
          <div>
            <p className="eyebrow">YOUR PRIVATE MACHINE RACK</p>
            <h1>Rackio</h1>
          </div>
        </div>
        <button type="button" className="pair-button" onClick={() => setPairingOpen(true)}>
          <span aria-hidden="true">＋</span> Pair machine
        </button>
      </header>
      {pairingOpen ? (
        <div className="dialog-backdrop">
          <section
            className="pair-dialog"
            role="dialog"
            aria-modal="true"
            aria-labelledby="pair-title"
          >
            <div>
              <p className="eyebrow">E2E ENCRYPTED PAIRING</p>
              <h2 id="pair-title">Pair a machine</h2>
            </div>
            <div className="method-tabs" role="tablist" aria-label="Pairing method">
              <button
                type="button"
                role="tab"
                aria-selected={pairingMethod === "bundle"}
                onClick={() => setPairingMethod("bundle")}
              >
                Pairing bundle
              </button>
              <button
                type="button"
                role="tab"
                aria-selected={pairingMethod === "ssh"}
                onClick={() => setPairingMethod("ssh")}
              >
                Install over SSH
              </button>
            </div>
            {pairingMethod === "bundle" ? (
              <>
                <p>
                  On the machine you want to monitor, run <code>rackio pairing create</code>, then
                  paste its one-time bundle here.
                </p>
                <form onSubmit={submitPairing}>
                  <label htmlFor="pairing-bundle">Pairing bundle</label>
                  <textarea
                    id="pairing-bundle"
                    name="pairing-bundle"
                    value={bundle}
                    onChange={(event) => setBundle(event.target.value)}
                    placeholder="rackio-pair:…"
                    autoComplete="off"
                    spellCheck={false}
                    autoFocus
                  />
                  {pairing.state === "error" ? (
                    <p className="form-message error-message" role="alert">
                      {pairing.message}
                    </p>
                  ) : null}
                  {pairing.state === "success" ? (
                    <p className="form-message success-message" role="status">
                      {pairing.machineName} is paired.
                    </p>
                  ) : null}
                  <div className="dialog-actions">
                    <button
                      type="button"
                      className="secondary-button"
                      onClick={() => setPairingOpen(false)}
                      disabled={pairing.state === "submitting"}
                    >
                      Cancel
                    </button>
                    <button
                      type="submit"
                      className="pair-button"
                      disabled={pairing.state === "submitting" || bundle.trim().length === 0}
                    >
                      {pairing.state === "submitting" ? "Pairing…" : "Pair machine"}
                    </button>
                  </div>
                </form>
              </>
            ) : (
              <SshBootstrapForm
                status={sshBootstrap}
                onInspect={onInspectSshHost}
                onInstall={onInstallViaSsh}
                onCancel={() => setPairingOpen(false)}
              />
            )}
          </section>
        </div>
      ) : null}
      <section className="summary" aria-label="Rack summary">
        <div>
          <strong>{snapshot.nodes.length}</strong>
          <span>Machines</span>
        </div>
        <div>
          <strong>{nodeStateRegistry[fleetState].label}</strong>
          <span>Rack state</span>
        </div>
        <div>
          <strong>{snapshot.nodes.filter((node) => node.path === "relayed").length}</strong>
          <span>Relayed</span>
        </div>
        <p>Metrics stay on your machines. No account. No central database.</p>
      </section>
      {snapshot.daemon === "unavailable" ? (
        <section className="empty-state alert-state" role="alert">
          <p className="eyebrow">AGENT UNAVAILABLE</p>
          <h2>Background monitoring is disconnected</h2>
          <p>{snapshot.message}</p>
          <code>rackio daemon</code>
        </section>
      ) : snapshot.nodes.length === 0 ? (
        <section className="empty-state">
          <p className="eyebrow">READY TO CONNECT</p>
          <h2>Your private rack starts here</h2>
          <p>{snapshot.message ?? "Open a pairing window on another machine to begin."}</p>
        </section>
      ) : (
        <section className="node-grid" aria-label="Monitored machines">
          {snapshot.nodes.map((node) => (
            <NodeCard key={node.id} node={node} />
          ))}
        </section>
      )}
    </main>
  );
}
