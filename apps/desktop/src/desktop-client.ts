import { Channel, invoke } from "@tauri-apps/api/core";

import type {
  AlertListing,
  AlertRuleEdit,
  DesktopPreferences,
  DesktopPreferencesView,
  NotificationThreshold,
  FleetSnapshot,
  HistoryPoint,
  HistoryRange,
  SshBootstrapInput,
  SshHostIdentity,
  SshProgress,
  SshTarget,
} from "./types";

export interface PairedMachineResult {
  node?: { display_name?: string };
}

export interface PairingShareResult {
  bundle: string;
  expiresAtMs: number;
  qrDataUrl?: string;
  qrError?: string;
  lanWarning?: string;
}

export interface SshBootstrapResult {
  pairingBundle: string;
  remotePlatform: string;
}

export function fetchFleetSnapshot(): Promise<FleetSnapshot> {
  return invoke<FleetSnapshot>("fleet_snapshot");
}

export function importPairingBundle(bundle: string): Promise<PairedMachineResult> {
  return invoke<PairedMachineResult>("pair_machine", { bundle });
}

export function inspectSshHost(target: SshTarget): Promise<SshHostIdentity> {
  return invoke<SshHostIdentity>("ssh_inspect_host", { target });
}

export function createPairingShare(): Promise<PairingShareResult> {
  return invoke<PairingShareResult>("create_pairing_share");
}

export function bootstrapSsh(
  request: SshBootstrapInput,
  onProgress: (progress: SshProgress) => void,
): Promise<SshBootstrapResult> {
  const progressChannel = new Channel<SshProgress>();
  progressChannel.onmessage = onProgress;
  return invoke<SshBootstrapResult>("ssh_bootstrap", {
    request,
    onProgress: progressChannel,
  });
}

export function fetchMachineHistory(
  endpointId: string,
  hours: HistoryRange,
): Promise<HistoryPoint[]> {
  return invoke<HistoryPoint[]>("machine_history", { endpointId, hours });
}

/** Set a machine's tray icon (one character or emoji), or clear it with null. */
export function setTrayIcon(machineId: string, icon: string | null): Promise<void> {
  return invoke<void>("set_tray_icon", { machineId, icon });
}

export function savePairingBundle(path: string, bundle: string): Promise<void> {
  return invoke<void>("save_pairing_bundle", { path, bundle });
}

/** This machine's health rules, read back from the daemon. */
export function fetchAlerts(): Promise<AlertListing> {
  return invoke<AlertListing>("alerts_list");
}

export function setAlertRule(edit: AlertRuleEdit): Promise<AlertListing> {
  return invoke<AlertListing>("alerts_set", { edit });
}

export function setAlertRuleEnabled(id: string, enabled: boolean): Promise<AlertListing> {
  return invoke<AlertListing>("alerts_rule_enabled", { id, enabled });
}

/** Restore one rule's shipped levels, or every rule's when `id` is omitted. */
export function resetAlerts(id?: string): Promise<AlertListing> {
  return invoke<AlertListing>("alerts_reset", { id: id ?? null });
}

export function setAlertsEnabled(enabled: boolean): Promise<AlertListing> {
  return invoke<AlertListing>("alerts_set_enabled", { enabled });
}

export function fetchDesktopPreferences(): Promise<DesktopPreferencesView> {
  return invoke<DesktopPreferencesView>("get_desktop_preferences");
}

export function saveDesktopPreferences(
  preferences: DesktopPreferences,
): Promise<DesktopPreferencesView> {
  return invoke<DesktopPreferencesView>("set_desktop_preferences", { preferences });
}

/** Carry the legacy localStorage choices over once; a no-op once preferences exist. */
export function migrateLegacyPreferences(
  notificationsEnabled: boolean,
  threshold: NotificationThreshold,
): Promise<DesktopPreferencesView> {
  return invoke<DesktopPreferencesView>("migrate_legacy_preferences", {
    notificationsEnabled,
    threshold,
  });
}
