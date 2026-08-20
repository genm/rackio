import { Channel, invoke } from "@tauri-apps/api/core";

import type {
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

export function savePairingBundle(path: string, bundle: string): Promise<void> {
  return invoke<void>("save_pairing_bundle", { path, bundle });
}
