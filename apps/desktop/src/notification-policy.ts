import { nodeStateRegistry } from "./state-model";
import type { FleetNode, NodeState, NotificationThreshold } from "./types";

export interface MachineNotification {
  title: string;
  body: string;
}

export function machineNotificationTransitions(
  previous: ReadonlyMap<string, NodeState>,
  nodes: readonly FleetNode[],
  threshold: NotificationThreshold,
): MachineNotification[] {
  const alerting = nodeStateRegistry[threshold].rank;
  const current = new Map(nodes.map((node) => [node.id, node] as const));
  const announcements: MachineNotification[] = [];
  for (const [id, node] of current) {
    const was = previous.get(id);
    if (was === undefined || was === node.state) continue;
    const wasAlerting = nodeStateRegistry[was].rank >= alerting;
    const isAlerting = nodeStateRegistry[node.state].rank >= alerting;
    // Naming the machine is the point: "the rack is critical" does not say
    // which box to look at. Recovery is announced too, so an operator who was
    // told about a failure learns it ended without opening the app.
    if (isAlerting && !wasAlerting) {
      announcements.push({
        title: `Rackio · ${node.name} is ${nodeStateRegistry[node.state].label}`,
        // The reporting machine's own detail — which filesystem, what value,
        // which threshold — is what makes the notification actionable without
        // opening the app. States the daemon derives from silence, such as
        // offline, carry no detail (the daemon sends `null`), so the transition
        // sentence still has to stand on its own.
        body: `${node.name} changed from ${nodeStateRegistry[was].label} to ${nodeStateRegistry[node.state].label}.${
          node.detail ? ` ${node.detail}` : ""
        }`,
      });
    } else if (wasAlerting && !isAlerting) {
      announcements.push({
        title: `Rackio · ${node.name} recovered`,
        body: `${node.name} is ${nodeStateRegistry[node.state].label} again.`,
      });
    }
  }
  return announcements;
}
