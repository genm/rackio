import { useState } from "react";

import {
  machineDetailStateRegistry,
  nodeStateRegistry,
  notificationStateRegistry,
  pairingShareStateRegistry,
  sshBootstrapStateRegistry,
  traySurfaceStateRegistry,
  worstState,
} from "../state-registry";
import type {
  FleetSnapshot,
  FleetNode,
  MachineDetailState,
  NotificationState,
  NotificationThreshold,
  PairingStatus,
  PairingShareState,
  SshBootstrapInput,
  SshBootstrapStatus,
  SshTarget,
  TraySurfaceState,
} from "../types";
import { NodeCard } from "./NodeCard";
import { MachineDetail } from "./MachineDetail";
import { NotificationControls } from "./NotificationControls";
import { PairingShare } from "./PairingShare";
import { SshBootstrapForm } from "./SshBootstrapForm";

export function Dashboard({
  snapshot,
  pairing = { state: "idle" },
  sshBootstrap = sshBootstrapStateRegistry.editing,
  machineDetail = machineDetailStateRegistry.closed,
  traySurface = traySurfaceStateRegistry.available,
  notificationState = notificationStateRegistry.disabled,
  pairingShare = pairingShareStateRegistry.idle,
  onPair = async () => undefined,
  onInspectSshHost = async () => undefined,
  onInstallViaSsh = async () => undefined,
  onViewHistory = async () => undefined,
  onCloseHistory = () => undefined,
  onEnableNotifications = async () => undefined,
  onDisableNotifications = () => undefined,
  onNotificationThresholdChange = () => undefined,
  onCreatePairingShare = async () => undefined,
}: {
  snapshot: FleetSnapshot;
  pairing?: PairingStatus;
  sshBootstrap?: SshBootstrapStatus;
  machineDetail?: MachineDetailState;
  traySurface?: TraySurfaceState;
  notificationState?: NotificationState;
  pairingShare?: PairingShareState;
  onPair?: (bundle: string) => Promise<void>;
  onInspectSshHost?: (target: SshTarget) => Promise<void>;
  onInstallViaSsh?: (input: SshBootstrapInput) => Promise<void>;
  onViewHistory?: (node: FleetNode) => Promise<void>;
  onCloseHistory?: () => void;
  onEnableNotifications?: () => Promise<void>;
  onDisableNotifications?: () => void;
  onNotificationThresholdChange?: (threshold: NotificationThreshold) => void;
  onCreatePairingShare?: () => Promise<void>;
}) {
  const [pairingOpen, setPairingOpen] = useState(false);
  const [pairingMethod, setPairingMethod] = useState<"bundle" | "share" | "ssh">("bundle");
  const [bundle, setBundle] = useState("");
  // A disconnected agent or an empty rack is not a healthy rack. Keep the
  // summary and the header pulse unknown rather than green when there is
  // nothing to base a state on.
  const fleetState =
    snapshot.daemon === "connected" && snapshot.nodes.length > 0
      ? worstState(snapshot.nodes.map((node) => node.state))
      : null;
  const fleetTone =
    fleetState === null
      ? snapshot.daemon === "connected"
        ? "muted"
        : "bad"
      : nodeStateRegistry[fleetState].tone;
  const fleetLabel = fleetState === null ? "—" : nodeStateRegistry[fleetState].label;
  const machineCount = snapshot.daemon === "connected" ? String(snapshot.nodes.length) : "—";
  const relayedCount =
    snapshot.daemon === "connected"
      ? String(snapshot.nodes.filter((node) => node.path === "relayed").length)
      : "—";
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
  const importPairingFile = async (event: React.ChangeEvent<HTMLInputElement>) => {
    const file = event.target.files?.[0];
    if (file === undefined) return;
    setBundle((await file.text()).trim());
  };
  return (
    <main>
      <header className="topbar">
        <div className="brand">
          <span className={`pulse tone-${fleetTone}`} aria-hidden="true" />
          <div>
            <p className="eyebrow">YOUR PRIVATE MACHINE RACK</p>
            <h1>Rackio</h1>
          </div>
        </div>
        <div className="topbar-actions">
          <NotificationControls
            status={notificationState}
            onEnable={onEnableNotifications}
            onDisable={onDisableNotifications}
            onThresholdChange={onNotificationThresholdChange}
          />
          <button type="button" className="pair-button" onClick={() => setPairingOpen(true)}>
            <span aria-hidden="true">＋</span> Pair machine
          </button>
        </div>
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
              <button
                type="button"
                role="tab"
                aria-selected={pairingMethod === "share"}
                onClick={() => setPairingMethod("share")}
              >
                Share this machine
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
                  <label className="file-import">
                    Or import a pairing file
                    <input type="file" accept=".txt,text/plain" onChange={importPairingFile} />
                  </label>
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
            ) : pairingMethod === "ssh" ? (
              <SshBootstrapForm
                status={sshBootstrap}
                onInspect={onInspectSshHost}
                onInstall={onInstallViaSsh}
                onCancel={() => setPairingOpen(false)}
              />
            ) : (
              <PairingShare status={pairingShare} onCreate={onCreatePairingShare} />
            )}
          </section>
        </div>
      ) : null}
      <MachineDetail detail={machineDetail} onClose={onCloseHistory} />
      {traySurface.state === "unavailable" ? (
        <section className="capability-banner" role="status">
          <strong>Tray unavailable</strong>
          <span>{traySurface.message}</span>
        </section>
      ) : null}
      <section className="summary" aria-label="Rack summary">
        <div>
          <strong>{machineCount}</strong>
          <span>Machines</span>
        </div>
        <div>
          <strong>{fleetLabel}</strong>
          <span>Rack state</span>
        </div>
        <div>
          <strong>{relayedCount}</strong>
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
            <NodeCard key={node.id} node={node} onViewHistory={onViewHistory} />
          ))}
        </section>
      )}
    </main>
  );
}
