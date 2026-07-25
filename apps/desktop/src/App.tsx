import { Channel, invoke } from "@tauri-apps/api/core";
import {
  isPermissionGranted,
  requestPermission,
  sendNotification,
} from "@tauri-apps/plugin-notification";
import { useEffect, useRef, useState } from "react";

import { Dashboard } from "./components/Dashboard";
import {
  machineDetailStateRegistry,
  nodeStateRegistry,
  pairingShareStateRegistry,
  sshBootstrapStateRegistry,
  surfaceStateRegistry,
  traySurfaceStateRegistry,
  worstState,
} from "./state-registry";
import type {
  FleetNode,
  FleetSnapshot,
  HistoryPoint,
  MachineDetailState,
  NodeState,
  NotificationState,
  NotificationThreshold,
  PairingStatus,
  PairingShareState,
  SshBootstrapInput,
  SshBootstrapStatus,
  SshHostIdentity,
  SshProgress,
  SshTarget,
  TraySurfaceState,
} from "./types";

const NOTIFICATION_THRESHOLD_KEY = "rackio.notification-threshold";
const NOTIFICATIONS_ENABLED_KEY = "rackio.notifications-enabled";

function initialNotificationState(): NotificationState {
  const stored = window.localStorage.getItem(NOTIFICATION_THRESHOLD_KEY);
  const threshold: NotificationThreshold =
    stored === "warning" || stored === "degraded" || stored === "critical" || stored === "offline"
      ? stored
      : "critical";
  return window.localStorage.getItem(NOTIFICATIONS_ENABLED_KEY) === "true"
    ? { state: "requesting", threshold }
    : { state: "disabled", threshold };
}

export default function App() {
  const [snapshot, setSnapshot] = useState<FleetSnapshot>(surfaceStateRegistry.daemonUnavailable);
  const [pairing, setPairing] = useState<PairingStatus>({ state: "idle" });
  const [pairingShare, setPairingShare] = useState<PairingShareState>(
    pairingShareStateRegistry.idle,
  );
  const [sshBootstrap, setSshBootstrap] = useState<SshBootstrapStatus>(
    sshBootstrapStateRegistry.editing,
  );
  const [machineDetail, setMachineDetail] = useState<MachineDetailState>(
    machineDetailStateRegistry.closed,
  );
  const [traySurface, setTraySurface] = useState<TraySurfaceState>(
    traySurfaceStateRegistry.available,
  );
  const [notificationState, setNotificationState] =
    useState<NotificationState>(initialNotificationState);
  const previousFleetState = useRef<NodeState | undefined>(undefined);

  const pairMachine = async (bundle: string) => {
    setPairing({ state: "submitting" });
    try {
      const machine = await invoke<{ node?: { display_name?: string } }>("pair_machine", {
        bundle,
      });
      setPairing({
        state: "success",
        machineName: machine.node?.display_name ?? "Machine",
      });
      const value = await invoke<FleetSnapshot>("fleet_snapshot");
      setSnapshot(value);
    } catch (error: unknown) {
      setPairing({
        state: "error",
        message: error instanceof Error ? error.message : String(error),
      });
      throw error;
    }
  };

  const inspectSshHost = async (target: SshTarget) => {
    setSshBootstrap({ state: "checking_host" });
    try {
      const identity = await invoke<SshHostIdentity>("ssh_inspect_host", { target });
      setSshBootstrap({ state: "confirming_host_key", ...identity });
    } catch (error: unknown) {
      setSshBootstrap({
        state: "failed",
        message: error instanceof Error ? error.message : String(error),
      });
    }
  };

  const createPairingShare = async () => {
    setPairingShare({ state: "loading" });
    try {
      setPairingShare({
        state: "ready",
        ...(await invoke<{
          bundle: string;
          qrDataUrl?: string;
          qrError?: string;
        }>("create_pairing_share")),
      });
    } catch (error: unknown) {
      setPairingShare({
        state: "error",
        message: error instanceof Error ? error.message : String(error),
      });
    }
  };

  const installViaSsh = async (input: SshBootstrapInput) => {
    const onProgress = new Channel<SshProgress>();
    onProgress.onmessage = ({ stage, detail }) => {
      setSshBootstrap({ state: "running", stage, detail });
    };
    try {
      const installed = await invoke<{ pairingBundle: string; remotePlatform: string }>(
        "ssh_bootstrap",
        { request: input, onProgress },
      );
      setSshBootstrap({
        state: "running",
        stage: "connecting_p2p",
        detail: "Authorizing the new machine over the encrypted P2P connection",
      });
      const machine = await invoke<{ node?: { display_name?: string } }>("pair_machine", {
        bundle: installed.pairingBundle,
      });
      const machineName = machine.node?.display_name ?? input.target.host;
      setSshBootstrap({
        state: "completed",
        machineName,
        remotePlatform: installed.remotePlatform,
      });
      setPairing({ state: "success", machineName });
      setSnapshot(await invoke<FleetSnapshot>("fleet_snapshot"));
    } catch (error: unknown) {
      setSshBootstrap({
        state: "failed",
        message: error instanceof Error ? error.message : String(error),
      });
    }
  };

  const viewHistory = async (node: FleetNode) => {
    if (node.endpointId === undefined) return;
    setMachineDetail({ state: "loading", node });
    try {
      const points = await invoke<HistoryPoint[]>("machine_history", {
        endpointId: node.endpointId,
        hours: 24,
      });
      setMachineDetail({ state: "ready", node, points });
    } catch (error: unknown) {
      setMachineDetail({
        state: "error",
        node,
        message: error instanceof Error ? error.message : String(error),
      });
    }
  };

  const enableNotifications = async () => {
    const threshold = notificationState.threshold;
    setNotificationState({ state: "requesting", threshold });
    try {
      const granted = (await isPermissionGranted()) || (await requestPermission()) === "granted";
      if (!granted) {
        setNotificationState({
          state: "denied",
          threshold,
          message: "Notification permission was denied by the operating system.",
        });
        return;
      }
      setNotificationState({ state: "enabled", threshold });
    } catch (error: unknown) {
      setNotificationState({
        state: "error",
        threshold,
        message: error instanceof Error ? error.message : String(error),
      });
    }
  };

  const setNotificationThreshold = (threshold: NotificationThreshold) => {
    setNotificationState((current) => ({ ...current, threshold }));
  };

  useEffect(() => {
    if (notificationState.state === "requesting") {
      void enableNotifications();
    }
  }, []);

  useEffect(() => {
    window.localStorage.setItem(NOTIFICATION_THRESHOLD_KEY, notificationState.threshold);
    if (notificationState.state === "enabled") {
      window.localStorage.setItem(NOTIFICATIONS_ENABLED_KEY, "true");
    } else if (
      notificationState.state === "disabled" ||
      notificationState.state === "denied" ||
      notificationState.state === "error"
    ) {
      window.localStorage.setItem(NOTIFICATIONS_ENABLED_KEY, "false");
    }
  }, [notificationState]);

  useEffect(() => {
    let active = true;
    let timer: ReturnType<typeof setTimeout> | undefined;
    const refresh = async () => {
      try {
        const value = await invoke<FleetSnapshot>("fleet_snapshot");
        if (active) setSnapshot(value);
      } catch (error: unknown) {
        if (active) {
          setSnapshot({
            daemon: "unavailable",
            nodes: [],
            message: error instanceof Error ? error.message : String(error),
          });
        }
      } finally {
        if (active) timer = setTimeout(refresh, 2_000);
      }
    };
    void refresh();
    return () => {
      active = false;
      if (timer !== undefined) clearTimeout(timer);
    };
  }, []);

  useEffect(() => {
    if (snapshot.daemon !== "connected") return;
    const state =
      snapshot.nodes.length === 0
        ? "healthy"
        : worstState(snapshot.nodes.map((node) => node.state));
    void invoke("set_tray_state", { state })
      .then(() => setTraySurface(traySurfaceStateRegistry.available))
      .catch((error: unknown) =>
        setTraySurface({
          state: "unavailable",
          message: error instanceof Error ? error.message : String(error),
        }),
      );
  }, [snapshot]);

  useEffect(() => {
    if (snapshot.daemon !== "connected" || snapshot.nodes.length === 0) return;
    const state = worstState(snapshot.nodes.map((node) => node.state));
    const previous = previousFleetState.current;
    previousFleetState.current = state;
    if (
      notificationState.state !== "enabled" ||
      previous === undefined ||
      previous === state ||
      nodeStateRegistry[state].rank < nodeStateRegistry[notificationState.threshold].rank
    ) {
      return;
    }
    try {
      sendNotification({
        title: `Rackio · ${nodeStateRegistry[state].label}`,
        body: `Your rack changed from ${nodeStateRegistry[previous].label} to ${nodeStateRegistry[state].label}.`,
      });
    } catch (error: unknown) {
      setNotificationState({
        state: "error",
        threshold: notificationState.threshold,
        message: error instanceof Error ? error.message : String(error),
      });
    }
  }, [notificationState, snapshot]);

  return (
    <Dashboard
      snapshot={snapshot}
      pairing={pairing}
      pairingShare={pairingShare}
      sshBootstrap={sshBootstrap}
      machineDetail={machineDetail}
      traySurface={traySurface}
      notificationState={notificationState}
      onPair={pairMachine}
      onCreatePairingShare={createPairingShare}
      onInspectSshHost={inspectSshHost}
      onInstallViaSsh={installViaSsh}
      onViewHistory={viewHistory}
      onCloseHistory={() => setMachineDetail(machineDetailStateRegistry.closed)}
      onEnableNotifications={enableNotifications}
      onDisableNotifications={() =>
        setNotificationState({
          state: "disabled",
          threshold: notificationState.threshold,
        })
      }
      onNotificationThresholdChange={setNotificationThreshold}
    />
  );
}
