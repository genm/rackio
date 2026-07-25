export type NodeState =
  | "healthy"
  | "warning"
  | "critical"
  | "stale"
  | "offline"
  | "auth_error"
  | "incompatible"
  | "degraded";

export type ConnectionPath = "lan_direct" | "wan_direct" | "relayed" | "unknown";

export interface FleetNode {
  id: string;
  endpointId?: string;
  name: string;
  os: string;
  state: NodeState;
  path: ConnectionPath;
  cpuPercent?: number;
  memoryUsedBytes?: number;
  memoryTotalBytes?: number;
  diskUsedBytes?: number;
  diskTotalBytes?: number;
  rttMs?: number;
  lastSeenMs?: number;
  history: number[];
  detail?: string;
}

export interface HistoryPoint {
  timestampMs: number;
  cpuPercent?: number;
  memoryUsedBytes?: number;
  memoryTotalBytes?: number;
  networkReceivedBytesPerSecond?: number;
  networkSentBytesPerSecond?: number;
}

export type MachineDetailState =
  | { state: "closed" }
  | { state: "loading"; node: FleetNode }
  | { state: "ready"; node: FleetNode; points: HistoryPoint[] }
  | { state: "error"; node: FleetNode; message: string };

export type TraySurfaceState = { state: "available" } | { state: "unavailable"; message: string };

export type NotificationThreshold = "warning" | "degraded" | "critical" | "offline";

export type NotificationState =
  | { state: "disabled"; threshold: NotificationThreshold }
  | { state: "requesting"; threshold: NotificationThreshold }
  | { state: "enabled"; threshold: NotificationThreshold }
  | { state: "denied"; threshold: NotificationThreshold; message: string }
  | { state: "error"; threshold: NotificationThreshold; message: string };

export interface FleetSnapshot {
  daemon: "connected" | "unavailable";
  nodes: FleetNode[];
  message?: string;
}

export type PairingStatus =
  | { state: "idle" }
  | { state: "submitting" }
  | { state: "error"; message: string }
  | { state: "success"; machineName: string };

export interface SshTarget {
  host: string;
  user: string;
  port: number;
  identityFile?: string;
}

export interface SshHostIdentity {
  hostKeys: string[];
  fingerprints: string[];
}

export interface SshBootstrapInput {
  target: SshTarget;
  acceptedHostKeys: string[];
  archivePath: string;
  checksumPath: string;
}

export interface SshProgress {
  stage: string;
  detail: string;
}

export type SshBootstrapStatus =
  | { state: "editing" }
  | { state: "checking_host" }
  | ({ state: "confirming_host_key" } & SshHostIdentity)
  | { state: "running"; stage: string; detail: string }
  | { state: "failed"; message: string }
  | { state: "completed"; machineName: string; remotePlatform: string };
