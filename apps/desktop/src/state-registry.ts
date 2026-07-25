import type {
  ConnectionPath,
  FleetSnapshot,
  MachineDetailState,
  NodeState,
  NotificationState,
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

export const surfaceStateRegistry: Record<string, FleetSnapshot> = {
  empty: { daemon: "connected", nodes: [], message: "No machines paired yet." },
  daemonUnavailable: {
    daemon: "unavailable",
    nodes: [],
    message: "The background agent is not reachable.",
  },
};

export const sshBootstrapStateRegistry: Record<string, SshBootstrapStatus> = {
  editing: { state: "editing" },
  checkingHost: { state: "checking_host" },
  confirmingHostKey: {
    state: "confirming_host_key",
    hostKeys: ["[server.test]:22 ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAITestFixtureOnly"],
    fingerprints: ["256 SHA256:fixtureFingerprint server.test (ED25519)"],
  },
  installing: {
    state: "running",
    stage: "installing",
    detail: "Verifying the archive and installing the systemd service",
  },
  failed: {
    state: "failed",
    message: "SSH authentication failed. Check the user, key, and server policy.",
  },
  completed: {
    state: "completed",
    machineName: "Home Server",
    remotePlatform: "Linux x86_64",
  },
};

const detailFixtureNode = {
  id: "detail-node",
  endpointId: "endpoint-detail",
  name: "Home Server",
  os: "Linux · x86_64",
  state: "healthy" as const,
  path: "lan_direct" as const,
  cpuPercent: 42,
  memoryUsedBytes: 12_000_000_000,
  memoryTotalBytes: 32_000_000_000,
  rttMs: 8,
  lastSeenMs: 1_750_000_000_000,
  history: [32, 39, 42],
};

export const machineDetailStateRegistry: Record<string, MachineDetailState> = {
  closed: { state: "closed" },
  loading: { state: "loading", node: detailFixtureNode },
  ready: {
    state: "ready",
    node: detailFixtureNode,
    points: [
      {
        timestampMs: 1_750_000_000_000,
        cpuPercent: 32,
        memoryUsedBytes: 11_500_000_000,
        memoryTotalBytes: 32_000_000_000,
      },
      {
        timestampMs: 1_750_000_060_000,
        cpuPercent: 42,
        memoryUsedBytes: 12_000_000_000,
        memoryTotalBytes: 32_000_000_000,
      },
    ],
  },
  empty: { state: "ready", node: detailFixtureNode, points: [] },
  error: {
    state: "error",
    node: detailFixtureNode,
    message: "History request timed out. Live monitoring continues.",
  },
};

export const traySurfaceStateRegistry: Record<string, TraySurfaceState> = {
  available: { state: "available" },
  unavailable: {
    state: "unavailable",
    message: "System tray is unavailable in this desktop environment. Rackio remains open here.",
  },
};

export const notificationStateRegistry: Record<string, NotificationState> = {
  disabled: { state: "disabled", threshold: "critical" },
  requesting: { state: "requesting", threshold: "critical" },
  enabled: { state: "enabled", threshold: "critical" },
  denied: {
    state: "denied",
    threshold: "critical",
    message: "Notification permission was denied by the operating system.",
  },
  error: {
    state: "error",
    threshold: "critical",
    message: "Rackio could not deliver an operating-system notification.",
  },
};

export function worstState(states: NodeState[]): NodeState {
  return states.reduce<NodeState>(
    (worst, state) =>
      nodeStateRegistry[state].rank > nodeStateRegistry[worst].rank ? state : worst,
    "healthy",
  );
}
