import { invoke } from "@tauri-apps/api/core";
import { useEffect, useState } from "react";

import { Dashboard } from "./components/Dashboard";
import { surfaceStateRegistry } from "./state-registry";
import type { FleetSnapshot, PairingStatus } from "./types";

export default function App() {
  const [snapshot, setSnapshot] = useState<FleetSnapshot>(surfaceStateRegistry.daemonUnavailable);
  const [pairing, setPairing] = useState<PairingStatus>({ state: "idle" });

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

  return <Dashboard snapshot={snapshot} pairing={pairing} onPair={pairMachine} />;
}
