import type {
  ConnectionPath,
  FleetSnapshot,
  MachineDetailState,
  NodeState,
  NotificationState,
  PairingShareState,
  PairingStatus,
  SshBootstrapStatus,
  TraySurfaceState,
} from "./types";

export const nodeStateRegistry: Record<NodeState, { label: string; rank: number; tone: string }> = {
  healthy: { label: "Healthy", rank: 0, tone: "good" },
  warning: { label: "Warning", rank: 1, tone: "warn" },
  degraded: { label: "Degraded", rank: 2, tone: "warn" },
  stale: { label: "Stale", rank: 3, tone: "muted" },
  critical: { label: "Critical", rank: 4, tone: "bad" },
  offline: { label: "Offline", rank: 5, tone: "bad" },
  auth_error: { label: "Auth error", rank: 6, tone: "bad" },
  incompatible: { label: "Incompatible", rank: 6, tone: "bad" },
};

export const connectionPathRegistry: Record<
  ConnectionPath,
  { label: string; description: string }
> = {
  lan_direct: { label: "LAN Direct", description: "Direct local connection" },
  wan_direct: { label: "WAN Direct", description: "Direct internet connection" },
  relayed: {
    label: "Relayed",
    description: "E2E encrypted through your configured relay",
  },
  unknown: { label: "Unknown path", description: "Path has not been verified" },
};

const liveNodeStates = new Set<NodeState>(["healthy", "warning", "degraded", "critical"]);

/** A metric is current only while the agent is still receiving the stream. */
export function isLiveNodeState(state: NodeState): boolean {
  return liveNodeStates.has(state);
}

// Keep production defaults in one fixture-free module. Components may use
// these values directly without pulling the component-test state space into
// the shipped application bundle.
export const initialDesktopState = {
  snapshot: {
    daemon: "unavailable",
    nodes: [],
    message: "The background agent is not reachable.",
  },
  pairing: { state: "idle" },
  pairingShare: { state: "idle" },
  sshBootstrap: { state: "editing" },
  machineDetail: { state: "closed" },
  traySurface: { state: "available" },
  notificationState: { state: "disabled", threshold: "critical" },
} satisfies {
  snapshot: FleetSnapshot;
  pairing: PairingStatus;
  pairingShare: PairingShareState;
  sshBootstrap: SshBootstrapStatus;
  machineDetail: MachineDetailState;
  traySurface: TraySurfaceState;
  notificationState: NotificationState;
};

export function worstState(states: NodeState[]): NodeState {
  return states.reduce<NodeState>(
    (worst, state) =>
      nodeStateRegistry[state].rank > nodeStateRegistry[worst].rank ? state : worst,
    "healthy",
  );
}
