import { Channel, invoke } from "@tauri-apps/api/core";
import {
  isPermissionGranted,
  requestPermission,
  sendNotification,
} from "@tauri-apps/plugin-notification";
import { useEffect, useRef, useState } from "react";

import { Dashboard } from "./components/Dashboard";
import { initialDesktopState, nodeStateRegistry } from "./state-model";
import type {
  FleetNode,
  FleetSnapshot,
  HistoryPoint,
  HistoryRange,
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
  const [snapshot, setSnapshot] = useState<FleetSnapshot>(initialDesktopState.snapshot);
  const [pairing, setPairing] = useState<PairingStatus>(initialDesktopState.pairing);
  const [pairingShare, setPairingShare] = useState<PairingShareState>(
    initialDesktopState.pairingShare,
  );
  const [sshBootstrap, setSshBootstrap] = useState<SshBootstrapStatus>(
    initialDesktopState.sshBootstrap,
  );
  const [machineDetail, setMachineDetail] = useState<MachineDetailState>(
    initialDesktopState.machineDetail,
  );
  const [notificationState, setNotificationState] =
    useState<NotificationState>(initialNotificationState);
  // Per machine, not per rack: a fleet-level state hides which box changed and
  // stays silent when a second machine fails while the first is still down.
  const previousMachineStates = useRef<Map<string, NodeState>>(new Map());

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
          expiresAtMs: number;
          qrDataUrl?: string;
          qrError?: string;
          lanWarning?: string;
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

  const viewHistory = async (node: FleetNode, hours: HistoryRange = 24) => {
    if (node.endpointId === undefined) return;
    setMachineDetail({ state: "loading", node, hours });
    try {
      const points = await invoke<HistoryPoint[]>("machine_history", {
        endpointId: node.endpointId,
        hours,
      });
      setMachineDetail({ state: "ready", node, hours, points });
    } catch (error: unknown) {
      setMachineDetail({
        state: "error",
        node,
        hours,
        message: error instanceof Error ? error.message : String(error),
      });
    }
  };

  const changeHistoryRange = async (hours: HistoryRange) => {
    // The open dialog owns which machine is being queried; a range change must
    // not be able to load one machine's history under another's name.
    if (machineDetail.state === "closed") return;
    await viewHistory(machineDetail.node, hours);
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
    if (snapshot.daemon !== "connected" || snapshot.nodes.length === 0) return;
    const previous = previousMachineStates.current;
    const current = new Map(snapshot.nodes.map((node) => [node.id, node]));
    previousMachineStates.current = new Map(
      snapshot.nodes.map((node) => [node.id, node.state] as const),
    );
    if (notificationState.state !== "enabled" || previous.size === 0) return;
    const alerting = nodeStateRegistry[notificationState.threshold].rank;
    const announcements: { title: string; body: string }[] = [];
    for (const [id, node] of current) {
      const was = previous.get(id);
      if (was === undefined || was === node.state) continue;
      const wasAlerting = nodeStateRegistry[was].rank >= alerting;
      const isAlerting = nodeStateRegistry[node.state].rank >= alerting;
      // Naming the machine is the point: "the rack is critical" does not say
      // which box to look at. Recovery is announced too, so an operator who
      // was told about a failure learns it ended without opening the app.
      if (isAlerting && !wasAlerting) {
        announcements.push({
          title: `Rackio · ${node.name} is ${nodeStateRegistry[node.state].label}`,
          body: `${node.name} changed from ${nodeStateRegistry[was].label} to ${nodeStateRegistry[node.state].label}.`,
        });
      } else if (wasAlerting && !isAlerting) {
        announcements.push({
          title: `Rackio · ${node.name} recovered`,
          body: `${node.name} is ${nodeStateRegistry[node.state].label} again.`,
        });
      }
    }
    if (announcements.length === 0) return;
    try {
      for (const announcement of announcements) {
        sendNotification(announcement);
      }
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
      notificationState={notificationState}
      onPair={pairMachine}
      onCreatePairingShare={createPairingShare}
      onInspectSshHost={inspectSshHost}
      onInstallViaSsh={installViaSsh}
      onViewHistory={viewHistory}
      onCloseHistory={() => setMachineDetail(initialDesktopState.machineDetail)}
      onHistoryRangeChange={changeHistoryRange}
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
