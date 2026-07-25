import { Channel, invoke } from "@tauri-apps/api/core";
import { useEffect, useState } from "react";

import { Dashboard } from "./components/Dashboard";
import { sshBootstrapStateRegistry, surfaceStateRegistry } from "./state-registry";
import type {
  FleetSnapshot,
  PairingStatus,
  SshBootstrapInput,
  SshBootstrapStatus,
  SshHostIdentity,
  SshProgress,
  SshTarget,
} from "./types";

export default function App() {
  const [snapshot, setSnapshot] = useState<FleetSnapshot>(surfaceStateRegistry.daemonUnavailable);
  const [pairing, setPairing] = useState<PairingStatus>({ state: "idle" });
  const [sshBootstrap, setSshBootstrap] = useState<SshBootstrapStatus>(
    sshBootstrapStateRegistry.editing,
  );

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

  return (
    <Dashboard
      snapshot={snapshot}
      pairing={pairing}
      sshBootstrap={sshBootstrap}
      onPair={pairMachine}
      onInspectSshHost={inspectSshHost}
      onInstallViaSsh={installViaSsh}
    />
  );
}
