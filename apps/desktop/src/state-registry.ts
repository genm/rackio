import type {
  ConnectionPath,
  FleetSnapshot,
  MachineDetailState,
  NodeState,
  NotificationState,
  PairingShareState,
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
  temperature: {
    label: "Package id 0",
    celsius: 61.5,
    criticalCelsius: 100,
    sensorCount: 7,
  },
  rttMs: 8,
  lastSeenMs: 1_750_000_000_000,
  trend: [
    {
      timestampMs: 1_749_999_996_000,
      cpuPercent: 32,
      memoryUsedBytes: 11_000_000_000,
      memoryTotalBytes: 32_000_000_000,
    },
    {
      timestampMs: 1_749_999_998_000,
      cpuPercent: 39,
      memoryUsedBytes: 11_600_000_000,
      memoryTotalBytes: 32_000_000_000,
    },
    {
      timestampMs: 1_750_000_000_000,
      cpuPercent: 42,
      memoryUsedBytes: 12_000_000_000,
      memoryTotalBytes: 32_000_000_000,
    },
  ],
};

export const machineDetailStateRegistry: Record<string, MachineDetailState> = {
  closed: { state: "closed" },
  loading: { state: "loading", node: detailFixtureNode, hours: 24 },
  ready: {
    state: "ready",
    node: detailFixtureNode,
    hours: 24,
    points: [
      {
        timestampMs: 1_750_000_000_000,
        cpuPercent: 32,
        memoryUsedBytes: 11_500_000_000,
        memoryTotalBytes: 32_000_000_000,
        diskUsedBytes: 180_000_000_000,
        diskTotalBytes: 500_000_000_000,
        temperatureCelsius: 58,
      },
      {
        timestampMs: 1_750_000_060_000,
        cpuPercent: 42,
        memoryUsedBytes: 12_000_000_000,
        memoryTotalBytes: 32_000_000_000,
        diskUsedBytes: 185_000_000_000,
        diskTotalBytes: 500_000_000_000,
        temperatureCelsius: 61,
      },
      // A gap far wider than the one-minute spacing: the chart must break the
      // line here rather than draw across an outage the machine never reported.
      {
        timestampMs: 1_750_003_600_000,
        cpuPercent: 51,
        memoryUsedBytes: 13_000_000_000,
        memoryTotalBytes: 32_000_000_000,
      },
      {
        timestampMs: 1_750_003_660_000,
        cpuPercent: 47,
        memoryUsedBytes: 12_800_000_000,
        memoryTotalBytes: 32_000_000_000,
      },
    ],
  },
  empty: { state: "ready", node: detailFixtureNode, hours: 24, points: [] },
  error: {
    state: "error",
    node: detailFixtureNode,
    hours: 24,
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

// The agent opens a five-minute pairing window, so fixtures are anchored to the
// moment they are loaded rather than to a baked-in timestamp that would render
// every "ready" fixture as already expired.
const PAIRING_WINDOW_MS = 5 * 60 * 1_000;
const pairingWindowOpenedAtMs = Date.now();

export const pairingShareStateRegistry: Record<string, PairingShareState> = {
  idle: { state: "idle" },
  loading: { state: "loading" },
  ready: {
    state: "ready",
    bundle: "rackio-pair:test-bundle",
    expiresAtMs: pairingWindowOpenedAtMs + PAIRING_WINDOW_MS,
    qrDataUrl:
      "data:image/svg+xml;base64,PHN2ZyB4bWxucz0iaHR0cDovL3d3dy53My5vcmcvMjAwMC9zdmciIHZpZXdCb3g9IjAgMCAyOSAyOSIgc2hhcGUtcmVuZGVyaW5nPSJjcmlzcEVkZ2VzIj48cmVjdCB3aWR0aD0iMjkiIGhlaWdodD0iMjkiIGZpbGw9IiNmM2Y2ZjEiLz48ZyBmaWxsPSIjMGIwZjBkIj48cGF0aCBkPSJNMiAyaDd2N0gyek0yMCAyaDd2N2gtN3pNMiAyMGg3djdIMnoiLz48cGF0aCBmaWxsPSIjZjNmNmYxIiBkPSJNNCA0aDN2M0g0ek0yMiA0aDN2M2gtM3pNNCAyMmgzdjNINHoiLz48cGF0aCBkPSJNMTEgMmgydjJoLTJ6TTE1IDJoM3YyaC0zek0xMSA2aDR2MmgtNHpNMTcgNWgydjVoLTJ6TTEwIDEwaDN2M2gtM3pNMTQgOWgydjJoLTJ6TTE4IDExaDN2MmgtM3pNMjIgMTBoNXYyaC01ek0yIDExaDJ2NUgyek01IDExaDR2Mkg1ek01IDE1aDJ2M0g1ek05IDE0aDN2Mkg5ek0xMyAxM2gydjVoLTJ6TTE2IDE1aDR2MmgtNHpNMjEgMTRoMnY1aC0yek0yNCAxM2gzdjNoLTN6TTEwIDE5aDJ2NGgtMnpNMTMgMjBoNHYyaC00ek0xOCAxOGgydjNoLTJ6TTIyIDIwaDV2MmgtNXpNMTIgMjRoM3YzaC0zek0xNiAyM2gydjRoLTJ6TTE5IDIyaDN2M2gtM3pNMjMgMjRoNHYzaC00eiIvPjwvZz48L3N2Zz4=",
  },
  qrUnavailable: {
    state: "ready",
    bundle: "rackio-pair:test-bundle-too-large-for-qr",
    expiresAtMs: pairingWindowOpenedAtMs + PAIRING_WINDOW_MS,
    qrError: "Pairing bundle is too large for a QR code.",
  },
  expired: {
    state: "ready",
    bundle: "rackio-pair:test-bundle",
    expiresAtMs: pairingWindowOpenedAtMs - 1_000,
    qrDataUrl:
      "data:image/svg+xml;base64,PHN2ZyB4bWxucz0iaHR0cDovL3d3dy53My5vcmcvMjAwMC9zdmciIHZpZXdCb3g9IjAgMCAyOSAyOSI+PC9zdmc+",
  },
  lanUnavailable: {
    state: "ready",
    bundle: "rackio-pair:test-bundle",
    expiresAtMs: pairingWindowOpenedAtMs + PAIRING_WINDOW_MS,
    qrDataUrl:
      "data:image/svg+xml;base64,PHN2ZyB4bWxucz0iaHR0cDovL3d3dy53My5vcmcvMjAwMC9zdmciIHZpZXdCb3g9IjAgMCAyOSAyOSIgc2hhcGUtcmVuZGVyaW5nPSJjcmlzcEVkZ2VzIj48cmVjdCB3aWR0aD0iMjkiIGhlaWdodD0iMjkiIGZpbGw9IiNmM2Y2ZjEiLz48ZyBmaWxsPSIjMGIwZjBkIj48cGF0aCBkPSJNMiAyaDd2N0gyek0yMCAyaDd2N2gtN3pNMiAyMGg3djdIMnoiLz48cGF0aCBmaWxsPSIjZjNmNmYxIiBkPSJNNCA0aDN2M0g0ek0yMiA0aDN2M2gtM3pNNCAyMmgzdjNINHoiLz48cGF0aCBkPSJNMTEgMmgydjJoLTJ6TTE1IDJoM3YyaC0zek0xMSA2aDR2MmgtNHpNMTcgNWgydjVoLTJ6TTEwIDEwaDN2M2gtM3pNMTQgOWgydjJoLTJ6TTE4IDExaDN2MmgtM3pNMjIgMTBoNXYyaC01ek0yIDExaDJ2NUgyek01IDExaDR2Mkg1ek01IDE1aDJ2M0g1ek05IDE0aDN2Mkg5ek0xMyAxM2gydjVoLTJ6TTE2IDE1aDR2MmgtNHpNMjEgMTRoMnY1aC0yek0yNCAxM2gzdjNoLTN6TTEwIDE5aDJ2NGgtMnpNMTMgMjBoNHYyaC00ek0xOCAxOGgydjNoLTJ6TTIyIDIwaDV2MmgtNXpNMTIgMjRoM3YzaC0zek0xNiAyM2gydjRoLTJ6TTE5IDIyaDN2M2gtM3pNMjMgMjRoNHYzaC00eiIvPjwvZz48L3N2Zz4=",
    lanWarning: "mDNS advertisement could not start: multicast is unavailable.",
  },
  error: {
    state: "error",
    message: "The local agent could not open a pairing window.",
  },
};

export function worstState(states: NodeState[]): NodeState {
  return states.reduce<NodeState>(
    (worst, state) =>
      nodeStateRegistry[state].rank > nodeStateRegistry[worst].rank ? state : worst,
    "healthy",
  );
}
