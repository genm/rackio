import {
  isPermissionGranted,
  requestPermission,
  sendNotification,
} from "@tauri-apps/plugin-notification";
import { useEffect, useRef, useState } from "react";

import { Dashboard } from "./components/Dashboard";
import {
  bootstrapSsh,
  createPairingShare as requestPairingShare,
  fetchDesktopPreferences,
  fetchFleetSnapshot,
  fetchMachineHistory,
  importPairingBundle,
  inspectSshHost as fetchSshHostIdentity,
  migrateLegacyPreferences,
  saveDesktopPreferences,
  setTrayIcon,
} from "./desktop-client";
import { machineNotificationTransitions } from "./notification-policy";
import {
  loadDesktopPreferences,
  notificationStateFromPreferences,
} from "./notification-preferences";
import { initialDesktopState } from "./state-model";
import type {
  DesktopPreferences,
  DesktopPreferencesView,
  FleetNode,
  FleetSnapshot,
  HistoryRange,
  MachineDetailState,
  NodeState,
  NotificationState,
  NotificationThreshold,
  PairingStatus,
  PairingShareState,
  SshBootstrapInput,
  SshBootstrapStatus,
  SshTarget,
} from "./types";

const DEFAULT_PREFERENCES: DesktopPreferences = {
  version: 1,
  notificationsEnabled: false,
  notificationThreshold: "critical",
  appearance: "system",
};

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
  const [notificationState, setNotificationState] = useState<NotificationState>(
    initialDesktopState.notificationState,
  );
  // The durable preferences as last read or saved by the native shell. Only the
  // notification fields are edited here; appearance is carried through so a
  // save never overwrites a choice made elsewhere.
  const preferences = useRef<DesktopPreferencesView | null>(null);
  // Per machine, not per rack: a fleet-level state hides which box changed and
  // stays silent when a second machine fails while the first is still down.
  const previousMachineStates = useRef<Map<string, NodeState>>(new Map());

  const pairMachine = async (bundle: string) => {
    setPairing({ state: "submitting" });
    try {
      const machine = await importPairingBundle(bundle);
      setPairing({
        state: "success",
        machineName: machine.node?.display_name ?? "Machine",
      });
      const value = await fetchFleetSnapshot();
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
      const identity = await fetchSshHostIdentity(target);
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
        ...(await requestPairingShare()),
      });
    } catch (error: unknown) {
      setPairingShare({
        state: "error",
        message: error instanceof Error ? error.message : String(error),
      });
    }
  };

  const installViaSsh = async (input: SshBootstrapInput) => {
    const installation = bootstrapSsh(input, ({ stage, detail }) =>
      setSshBootstrap({ state: "running", stage, detail }),
    );
    try {
      const installed = await installation;
      setSshBootstrap({
        state: "running",
        stage: "connecting_p2p",
        detail: "Authorizing the new machine over the encrypted P2P connection",
      });
      const machine = await importPairingBundle(installed.pairingBundle);
      const machineName = machine.node?.display_name ?? input.target.host;
      setSshBootstrap({
        state: "completed",
        machineName,
        remotePlatform: installed.remotePlatform,
      });
      setPairing({ state: "success", machineName });
      setSnapshot(await fetchFleetSnapshot());
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
      const points = await fetchMachineHistory(node.endpointId, hours);
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

  // A rejection propagates to the card, which shows the shell's reason. The
  // refresh only makes the new glyph appear before the next poll; if it fails,
  // that poll reports the agent state as usual.
  const changeTrayIcon = async (node: FleetNode, icon: string | null) => {
    await setTrayIcon(node.id, icon);
    fetchFleetSnapshot().then(setSnapshot, () => undefined);
  };

  const changeHistoryRange = async (hours: HistoryRange) => {
    // The open dialog owns which machine is being queried; a range change must
    // not be able to load one machine's history under another's name.
    if (machineDetail.state === "closed") return;
    await viewHistory(machineDetail.node, hours);
  };

  /**
   * Persist the notification intent. Resolves to an error message when the
   * native store could not save it; the in-memory preference is then left
   * as it was, so the window never shows a choice that will not survive a
   * restart.
   */
  const saveNotificationPreferences = async (
    enabled: boolean,
    threshold: NotificationThreshold,
  ): Promise<string | undefined> => {
    const current = preferences.current ?? { ...DEFAULT_PREFERENCES };
    try {
      preferences.current = await saveDesktopPreferences({
        version: 1,
        notificationsEnabled: enabled,
        notificationThreshold: threshold,
        appearance: current.appearance,
      });
      return undefined;
    } catch (error: unknown) {
      return error instanceof Error ? error.message : String(error);
    }
  };

  const failNotificationSave = (threshold: NotificationThreshold, message: string) =>
    setNotificationState({
      state: "error",
      threshold,
      message: `Could not save notification preferences: ${message}`,
    });

  /**
   * Check, and if needed ask for, the operating system's permission. Only a
   * user-initiated enable persists its outcome: a launch that merely restores a
   * stored preference must not rewrite it because permission was revoked.
   */
  const enableNotifications = async (
    threshold: NotificationThreshold = notificationState.threshold,
    userInitiated = true,
  ) => {
    setNotificationState({ state: "requesting", threshold });
    try {
      const granted = (await isPermissionGranted()) || (await requestPermission()) === "granted";
      if (!granted) {
        if (userInitiated) await saveNotificationPreferences(false, threshold);
        setNotificationState({
          state: "denied",
          threshold,
          message: "Notification permission was denied by the operating system.",
        });
        return;
      }
      if (userInitiated) {
        const failure = await saveNotificationPreferences(true, threshold);
        if (failure !== undefined) {
          failNotificationSave(threshold, failure);
          return;
        }
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

  const disableNotifications = async () => {
    const threshold = notificationState.threshold;
    const failure = await saveNotificationPreferences(false, threshold);
    if (failure !== undefined) {
      failNotificationSave(threshold, failure);
      return;
    }
    setNotificationState({ state: "disabled", threshold });
  };

  const setNotificationThreshold = async (threshold: NotificationThreshold) => {
    const enabled = notificationState.state === "enabled";
    const failure = await saveNotificationPreferences(enabled, threshold);
    if (failure !== undefined) {
      failNotificationSave(notificationState.threshold, failure);
      return;
    }
    setNotificationState((current) => ({ ...current, threshold }));
  };

  useEffect(() => {
    let active = true;
    void loadDesktopPreferences(
      { fetchDesktopPreferences, migrateLegacyPreferences },
      window.localStorage,
    )
      .then((view) => {
        if (!active) return;
        preferences.current = view;
        const restored = notificationStateFromPreferences(view);
        setNotificationState(restored);
        if (restored.state === "requesting") {
          void enableNotifications(restored.threshold, false);
        }
      })
      .catch((error: unknown) => {
        if (!active) return;
        setNotificationState({
          state: "error",
          threshold: initialDesktopState.notificationState.threshold,
          message: `Saved preferences could not be loaded: ${
            error instanceof Error ? error.message : String(error)
          }`,
        });
      });
    return () => {
      active = false;
    };
  }, []);

  useEffect(() => {
    let active = true;
    let timer: ReturnType<typeof setTimeout> | undefined;
    const refresh = async () => {
      try {
        const value = await fetchFleetSnapshot();
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
    previousMachineStates.current = new Map(
      snapshot.nodes.map((node) => [node.id, node.state] as const),
    );
    if (notificationState.state !== "enabled" || previous.size === 0) return;
    const announcements = machineNotificationTransitions(
      previous,
      snapshot.nodes,
      notificationState.threshold,
    );
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
      onEnableNotifications={() => enableNotifications()}
      onDisableNotifications={() => void disableNotifications()}
      onNotificationThresholdChange={(threshold) => void setNotificationThreshold(threshold)}
      onTrayIconChange={changeTrayIcon}
    />
  );
}
