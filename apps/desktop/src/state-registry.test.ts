import { describe, expect, it } from "vitest";

import { connectionPathRegistry, surfaceStateRegistry, worstState } from "./state-registry";

describe("state registry", () => {
  it("keeps relayed paths visibly distinct from direct paths", () => {
    expect(connectionPathRegistry.relayed.label).toBe("Relayed");
    expect(connectionPathRegistry.relayed.description).toContain("relay");
  });

  it("ranks authentication errors above healthy nodes", () => {
    expect(worstState(["healthy", "auth_error", "warning"])).toBe("auth_error");
  });

  it("does not represent an unavailable daemon as an empty healthy fleet", () => {
    expect(surfaceStateRegistry.daemonUnavailable.daemon).toBe("unavailable");
    expect(surfaceStateRegistry.daemonUnavailable.message).toMatch(/not reachable/i);
  });
});
