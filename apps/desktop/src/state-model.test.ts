import { describe, expect, it } from "vitest";

import { connectionPathRegistry, initialDesktopState, worstState } from "./state-model";

describe("production state model", () => {
  it("keeps relayed paths visibly distinct from direct paths", () => {
    expect(connectionPathRegistry.relayed.label).toBe("Relayed");
    expect(connectionPathRegistry.relayed.description).toContain("relay");
  });

  it("ranks authentication errors above healthy nodes", () => {
    expect(worstState(["healthy", "auth_error", "warning"])).toBe("auth_error");
  });

  it("does not represent an unavailable daemon as an empty healthy fleet", () => {
    expect(initialDesktopState.snapshot.daemon).toBe("unavailable");
    expect(initialDesktopState.snapshot.message).toMatch(/not reachable/i);
  });

  it("starts workflows in non-active states", () => {
    expect(initialDesktopState).toEqual({
      snapshot: {
        daemon: "unavailable",
        nodes: [],
        message: "The background agent is not reachable.",
      },
      pairing: { state: "idle" },
      pairingShare: { state: "idle" },
      sshBootstrap: { state: "editing" },
      machineDetail: { state: "closed" },
      traySurface: { state: "available" },
      notificationState: { state: "disabled", threshold: "critical" },
    });
  });
});
