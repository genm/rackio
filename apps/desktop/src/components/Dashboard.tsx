import { useState } from "react";

import { initialDesktopState, nodeStateRegistry, worstState } from "../state-model";
import type {
  FleetSnapshot,
  FleetNode,
  HistoryRange,
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
import type { TrendMetric } from "../trend-series";
import { useStoredMetricMap } from "../useStoredMetricMap";
import { FleetCompare } from "./FleetCompare";
import { NodeCard } from "./NodeCard";
import { MachineDetail } from "./MachineDetail";
import { NotificationControls } from "./NotificationControls";
import { PairMachineControl } from "./PairMachineControl";

export function Dashboard({
  snapshot,
  pairing = initialDesktopState.pairing,
  sshBootstrap = initialDesktopState.sshBootstrap,
  machineDetail = initialDesktopState.machineDetail,
  traySurface = initialDesktopState.traySurface,
  notificationState = initialDesktopState.notificationState,
  pairingShare = initialDesktopState.pairingShare,
  onPair = async () => undefined,
  onInspectSshHost = async () => undefined,
  onInstallViaSsh = async () => undefined,
  onViewHistory = async () => undefined,
  onCloseHistory = () => undefined,
  onHistoryRangeChange = () => undefined,
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
  onHistoryRangeChange?: (hours: HistoryRange) => void;
  onEnableNotifications?: () => Promise<void>;
  onDisableNotifications?: () => void;
  onNotificationThresholdChange?: (threshold: NotificationThreshold) => void;
  onCreatePairingShare?: () => Promise<void>;
}) {
  const [cardMetrics, setCardMetrics] = useStoredMetricMap();
  const [compareOpen, setCompareOpen] = useState(false);
  const [compareMetric, setCompareMetric] = useState<TrendMetric>("cpu");
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
  // Worst first: the machine that needs attention must be on screen without
  // scrolling. Ties keep a stable alphabetical order so cards do not swap
  // places on every two-second poll.
  const orderedNodes = [...snapshot.nodes].sort(
    (left, right) =>
      nodeStateRegistry[right.state].rank - nodeStateRegistry[left.state].rank ||
      left.name.localeCompare(right.name),
  );
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
          <PairMachineControl
            pairing={pairing}
            sshBootstrap={sshBootstrap}
            pairingShare={pairingShare}
            onPair={onPair}
            onInspectSshHost={onInspectSshHost}
            onInstallViaSsh={onInstallViaSsh}
            onCreatePairingShare={onCreatePairingShare}
          />
        </div>
      </header>
      <MachineDetail
        detail={machineDetail}
        onClose={onCloseHistory}
        onRangeChange={onHistoryRangeChange}
      />
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
        <>
          {/* Comparison is opt-in: a single machine has nothing to compare
              against, and the grid stays the default view. */}
          {orderedNodes.length > 1 ? (
            <div className="compare-bar">
              <button
                type="button"
                className="secondary-button compare-toggle"
                aria-expanded={compareOpen}
                onClick={() => setCompareOpen((open) => !open)}
              >
                {compareOpen ? "Hide comparison" : "Compare machines"}
              </button>
            </div>
          ) : null}
          {compareOpen && orderedNodes.length > 1 ? (
            <FleetCompare
              nodes={orderedNodes}
              metric={compareMetric}
              onMetricChange={setCompareMetric}
            />
          ) : null}
          <section className="node-grid" aria-label="Monitored machines">
            {orderedNodes.map((node) => (
              <NodeCard
                key={node.id}
                node={node}
                metric={cardMetrics[node.id] ?? "cpu"}
                onMetricChange={(metric) => setCardMetrics(node.id, metric)}
                onViewHistory={onViewHistory}
              />
            ))}
          </section>
        </>
      )}
    </main>
  );
}
