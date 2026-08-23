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

/**
 * The hottest sensor on a machine, named so the reading is attributable: an
 * unlabelled number cannot be told apart from a battery or drive reading.
 * `criticalCelsius` is the hardware's own threshold and is absent whenever the
 * OS does not publish one — the viewer never substitutes a guess.
 */
export interface TemperatureReading {
  label: string;
  celsius: number;
  criticalCelsius?: number | null;
  sensorCount: number;
}

/**
 * One timestamped point of a machine's metric series — the shared domain shape
 * behind both the live trend (the agent's `TrendWindow`) and the 24-hour
 * history query. The timestamp is the sample's own: time axes are labelled
 * from the data, never from an assumed sampling cadence.
 */
export interface TrendPoint {
  timestampMs: number;
  cpuPercent?: number | null;
  memoryUsedBytes?: number | null;
  memoryTotalBytes?: number | null;
  /**
   * Swap pressure at sample time. A machine with swap disabled reports a real
   * zero total, which reads as "no swap device" rather than as 0 % — absent
   * means the reading could not be taken at all.
   */
  swapUsedBytes?: number | null;
  swapTotalBytes?: number | null;
  /** The fullest disk at sample time, chosen by the agent's domain rule. */
  diskUsedBytes?: number | null;
  diskTotalBytes?: number | null;
  temperatureCelsius?: number | null;
  networkReceivedBytesPerSecond?: number | null;
  networkSentBytesPerSecond?: number | null;
  /** The viewer's own connection measurement; absent for the local machine. */
  rttMs?: number | null;
}

export interface FleetNode {
  id: string;
  endpointId?: string;
  name: string;
  os: string;
  state: NodeState;
  path: ConnectionPath;
  cpuPercent?: number | null;
  memoryUsedBytes?: number | null;
  memoryTotalBytes?: number | null;
  swapUsedBytes?: number | null;
  swapTotalBytes?: number | null;
  diskUsedBytes?: number | null;
  diskTotalBytes?: number | null;
  /** Absent or null on a machine with no readable sensor. */
  temperature?: TemperatureReading | null;
  networkReceivedBytesPerSecond?: number | null;
  networkSentBytesPerSecond?: number | null;
  rttMs?: number | null;
  /**
   * Seconds since the machine booted, absent when unknown. Not a trend point:
   * see the carve-out in `trend-series.ts`. The agent withholds the wire's
   * non-optional zero, which cannot be told apart from an unreported uptime.
   */
  uptimeSeconds?: number | null;
  lastSeenMs?: number;
  trend: TrendPoint[];
  /** The reporting machine's own explanation; absent or null when it has none. */
  detail?: string | null;
}

export type HistoryPoint = TrendPoint;

/** Ranges the history query offers; the agent accepts 1–168 hours. */
export type HistoryRange = 1 | 6 | 24 | 168;

export type MachineDetailState =
  | { state: "closed" }
  | { state: "loading"; node: FleetNode; hours: HistoryRange }
  | { state: "ready"; node: FleetNode; hours: HistoryRange; points: HistoryPoint[] }
  | { state: "error"; node: FleetNode; hours: HistoryRange; message: string };

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

export type PairingShareState =
  | { state: "idle" }
  | { state: "loading" }
  | {
      state: "ready";
      bundle: string;
      /**
       * Wall-clock expiry of the one-time pairing window, carried from the
       * bundle the agent generated. Required: a share whose expiry is unknown
       * must not be rendered as an open window.
       */
      expiresAtMs: number;
      qrDataUrl?: string;
      qrError?: string;
      lanWarning?: string;
    }
  | { state: "error"; message: string };

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
