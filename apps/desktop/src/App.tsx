import { invoke } from "@tauri-apps/api/core";
import { useEffect, useState } from "react";

import { Dashboard } from "./components/Dashboard";
import { surfaceStateRegistry } from "./state-registry";
import type { FleetSnapshot } from "./types";

export default function App() {
  const [snapshot, setSnapshot] = useState<FleetSnapshot>(surfaceStateRegistry.daemonUnavailable);

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

  return <Dashboard snapshot={snapshot} />;
}
