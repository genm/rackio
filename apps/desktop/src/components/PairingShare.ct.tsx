import { expect, test } from "@playwright/experimental-ct-react";

import { pairingShareStateRegistry } from "../state-registry";
import { PairingShare } from "./PairingShare";

test("renders the same ready state for QR and file transfer", async ({ mount }) => {
  const component = await mount(
    <div className="pair-dialog">
      <PairingShare status={pairingShareStateRegistry.ready} onCreate={async () => undefined} />
    </div>,
  );
  await expect(component.getByAltText("One-time Rackio pairing QR code")).toBeVisible();
  await expect(component.getByText("rackio-pair:test-bundle")).toBeVisible();
  await expect(component.getByRole("button", { name: "Save file" })).toBeVisible();
  await component.screenshot({ path: "../../output/playwright/pairing-share.png" });
});

test("keeps pairing-window creation failure visible and retryable", async ({ mount }) => {
  const component = await mount(
    <PairingShare status={pairingShareStateRegistry.error} onCreate={async () => undefined} />,
  );
  await expect(component.getByRole("alert")).toContainText("could not open");
  await expect(component.getByRole("button", { name: "Try again" })).toBeVisible();
});

test("keeps file and copy transfer available when a bundle cannot fit in a QR code", async ({
  mount,
}) => {
  const component = await mount(
    <PairingShare
      status={pairingShareStateRegistry.qrUnavailable}
      onCreate={async () => undefined}
    />,
  );
  await expect(component.getByRole("status")).toContainText("too large");
  await expect(component.getByRole("button", { name: "Save file" })).toBeVisible();
  await expect(component.getByRole("button", { name: "Copy bundle" })).toBeVisible();
});

test("surfaces LAN discovery degradation without disabling secure transfer", async ({ mount }) => {
  const component = await mount(
    <PairingShare
      status={pairingShareStateRegistry.lanUnavailable}
      onCreate={async () => undefined}
    />,
  );
  await expect(component.getByRole("status")).toContainText("LAN discovery is unavailable");
  await expect(component.getByAltText("One-time Rackio pairing QR code")).toBeVisible();
  await expect(component.getByRole("button", { name: "Save file" })).toBeVisible();
});
