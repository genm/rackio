import { expect, test } from "@playwright/experimental-ct-react";

import { notificationStateRegistry } from "../state-registry";
import { NotificationControls } from "./NotificationControls";

test("keeps notification permission denial visible", async ({ mount }) => {
  const component = await mount(
    <NotificationControls
      status={notificationStateRegistry.denied}
      onEnable={async () => undefined}
      onDisable={() => undefined}
      onThresholdChange={() => undefined}
    />,
  );
  await expect(component.getByText("Notification permission was denied")).toBeVisible();
  await expect(component.getByRole("button", { name: "Notifications off" })).toBeVisible();
});

test("allows the user to choose the alert threshold", async ({ mount }) => {
  let threshold = "";
  const component = await mount(
    <NotificationControls
      status={notificationStateRegistry.enabled}
      onEnable={async () => undefined}
      onDisable={() => undefined}
      onThresholdChange={(value) => {
        threshold = value;
      }}
    />,
  );
  await component.getByLabel("Notify at").selectOption("offline");
  await expect.poll(() => threshold).toBe("offline");
});
