import { describe, expect, it } from "vitest";

import { machineNotificationTransitions } from "./notification-policy";
import type { FleetNode, NodeState, NotificationThreshold } from "./types";

function machine(id: string, name: string, state: NodeState): FleetNode {
  return {
    id,
    name,
    os: "Test OS",
    state,
    path: "lan_direct",
    trend: [],
  };
}

function transitions(
  from: NodeState,
  to: NodeState,
  threshold: NotificationThreshold = "critical",
) {
  return machineNotificationTransitions(
    new Map([["machine-1", from]]),
    [machine("machine-1", "Build Server", to)],
    threshold,
  );
}

describe("machine notification transitions", () => {
  it("keeps the first snapshot and newly discovered machines silent", () => {
    expect(
      machineNotificationTransitions(
        new Map(),
        [machine("machine-1", "Build Server", "critical")],
        "warning",
      ),
    ).toEqual([]);
    expect(
      machineNotificationTransitions(
        new Map([["machine-1", "healthy"]]),
        [
          machine("machine-1", "Build Server", "healthy"),
          machine("machine-2", "New Server", "critical"),
        ],
        "warning",
      ),
    ).toEqual([]);
  });

  it("announces an inclusive threshold crossing with the machine identity", () => {
    expect(transitions("healthy", "warning", "warning")).toEqual([
      {
        title: "Rackio · Build Server is Warning",
        body: "Build Server changed from Healthy to Warning.",
      },
    ]);
  });

  it("announces recovery only when a machine leaves the alerting side", () => {
    expect(transitions("critical", "degraded")).toEqual([
      {
        title: "Rackio · Build Server recovered",
        body: "Build Server is Degraded again.",
      },
    ]);
  });

  it.each([
    ["healthy", "warning", "critical"],
    ["critical", "offline", "warning"],
    ["auth_error", "incompatible", "critical"],
    ["warning", "warning", "warning"],
  ] as const)("keeps a %s to %s change silent at the %s threshold", (from, to, threshold) => {
    expect(transitions(from, to, threshold)).toEqual([]);
  });

  it("keeps independent machines in snapshot order", () => {
    expect(
      machineNotificationTransitions(
        new Map([
          ["machine-a", "critical"],
          ["machine-b", "healthy"],
        ]),
        [machine("machine-b", "Database", "auth_error"), machine("machine-a", "Web", "healthy")],
        "critical",
      ),
    ).toEqual([
      {
        title: "Rackio · Database is Auth error",
        body: "Database changed from Healthy to Auth error.",
      },
      {
        title: "Rackio · Web recovered",
        body: "Web is Healthy again.",
      },
    ]);
  });

  it("preserves the existing last-value rule for a duplicate machine id", () => {
    expect(
      machineNotificationTransitions(
        new Map([["machine-1", "healthy"]]),
        [
          machine("machine-1", "Old Name", "warning"),
          machine("machine-1", "Current Name", "critical"),
        ],
        "critical",
      ),
    ).toEqual([
      {
        title: "Rackio · Current Name is Critical",
        body: "Current Name changed from Healthy to Critical.",
      },
    ]);
  });
});
