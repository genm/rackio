import { expect, test } from "@playwright/experimental-ct-react";

import { sshBootstrapStateRegistry } from "../state-registry";
import type { SshBootstrapInput, SshTarget } from "../types";
import { SshBootstrapForm } from "./SshBootstrapForm";

test("requires explicit host-key confirmation before SSH installation", async ({ mount }) => {
  let submitted: SshBootstrapInput | undefined;
  const component = await mount(
    <div className="pair-dialog">
      <SshBootstrapForm
        status={sshBootstrapStateRegistry.confirmingHostKey}
        onInspect={async () => undefined}
        onInstall={async (input) => {
          submitted = input;
        }}
        onCancel={() => undefined}
      />
    </div>,
  );

  await component.getByRole("textbox", { name: "Host", exact: true }).fill("server.test");
  await component.getByLabel("SSH user").fill("operator");
  await component.getByLabel("Release archive").fill("/tmp/rackio-release.tar.gz");
  await component.getByLabel("SHA-256 checksum file").fill("/tmp/rackio-release.tar.gz.sha256");
  const install = component.getByRole("button", { name: "Confirm and install" });
  await expect(install).toBeDisabled();
  await component.getByLabel("I verified this fingerprint through a trusted channel.").check();
  await install.click();
  await expect.poll(() => submitted?.target.host).toBe("server.test");
  await expect.poll(() => submitted?.acceptedHostKeys.length).toBe(1);
  await component.screenshot({ path: "../../output/playwright/ssh-host-key-confirmation.png" });
});

test("passes validated form fields into SSH host inspection", async ({ mount }) => {
  let inspected: SshTarget | undefined;
  const component = await mount(
    <SshBootstrapForm
      status={sshBootstrapStateRegistry.editing}
      onInspect={async (target) => {
        inspected = target;
      }}
      onInstall={async () => undefined}
      onCancel={() => undefined}
    />,
  );

  await component.getByLabel("Host").fill("server.test");
  await component.getByLabel("SSH user").fill("operator");
  await component.getByLabel("Port").fill("2222");
  await component.getByLabel("Release archive").fill("/tmp/rackio-release.tar.gz");
  await component.getByLabel("SHA-256 checksum file").fill("/tmp/rackio-release.tar.gz.sha256");
  await component.getByRole("button", { name: "Check SSH host" }).click();
  await expect.poll(() => inspected?.port).toBe(2222);
});

test("keeps SSH authentication failure visible", async ({ mount }) => {
  const component = await mount(
    <SshBootstrapForm
      status={sshBootstrapStateRegistry.failed}
      onInspect={async () => undefined}
      onInstall={async () => undefined}
      onCancel={() => undefined}
    />,
  );
  await expect(component.getByRole("alert")).toContainText("SSH authentication failed");
});
