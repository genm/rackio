import type { ConnectionPath, FleetSnapshot, NodeState } from "./types";

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

export const surfaceStateRegistry: Record<string, FleetSnapshot> = {
  empty: { daemon: "connected", nodes: [], message: "No peers paired yet." },
  daemonUnavailable: {
    daemon: "unavailable",
    nodes: [],
    message: "The background agent is not reachable.",
  },
};

export function worstState(states: NodeState[]): NodeState {
  return states.reduce<NodeState>(
    (worst, state) =>
      nodeStateRegistry[state].rank > nodeStateRegistry[worst].rank ? state : worst,
    "healthy",
  );
}
