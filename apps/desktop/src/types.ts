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

export interface FleetSnapshot {
  daemon: "connected" | "unavailable";
  nodes: FleetNode[];
  message?: string;
}
