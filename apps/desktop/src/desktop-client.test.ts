import { beforeEach, describe, expect, it, vi } from "vitest";

const tauri = vi.hoisted(() => {
  class TestChannel<T> {
    onmessage: (message: T) => void = () => undefined;
  }

  return {
    invoke: vi.fn(),
    Channel: TestChannel,
  };
});

vi.mock("@tauri-apps/api/core", () => ({
  invoke: tauri.invoke,
  Channel: tauri.Channel,
}));

import {
  bootstrapSsh,
  createPairingShare,
  fetchFleetSnapshot,
  fetchMachineHistory,
  importPairingBundle,
  inspectSshHost,
  savePairingBundle,
} from "./desktop-client";
import type { FleetSnapshot, SshBootstrapInput, SshProgress, SshTarget } from "./types";

beforeEach(() => {
  tauri.invoke.mockReset();
});

describe("desktop IPC client", () => {
  it("uses the exact fleet and pairing commands without transforming results", async () => {
    const snapshot: FleetSnapshot = { daemon: "connected", nodes: [] };
    const paired = { node: { display_name: "Build Server" } };
    const shared = {
      bundle: "rackio-pair:test-contract",
      expiresAtMs: 1_750_000_300_000,
      qrDataUrl: "data:image/svg+xml;base64,PHN2Zy8+",
    };
    tauri.invoke
      .mockResolvedValueOnce(snapshot)
      .mockResolvedValueOnce(paired)
      .mockResolvedValueOnce(shared);

    await expect(fetchFleetSnapshot()).resolves.toBe(snapshot);
    await expect(importPairingBundle(" rackio-pair:test-contract ")).resolves.toBe(paired);
    await expect(createPairingShare()).resolves.toBe(shared);

    expect(tauri.invoke.mock.calls).toEqual([
      ["fleet_snapshot"],
      ["pair_machine", { bundle: " rackio-pair:test-contract " }],
      ["create_pairing_share"],
    ]);
  });

  it("preserves the SSH target and machine-history payload keys", async () => {
    const target: SshTarget = { host: "server.test", user: "operator", port: 2222 };
    const identity = {
      hostKeys: ["[server.test]:2222 ssh-ed25519 test-key"],
      fingerprints: ["256 SHA256:test server.test (ED25519)"],
    };
    const history = [{ timestampMs: 1_750_000_000_000, cpuPercent: 42 }];
    tauri.invoke.mockResolvedValueOnce(identity).mockResolvedValueOnce(history);

    await expect(inspectSshHost(target)).resolves.toBe(identity);
    await expect(fetchMachineHistory("endpoint-id", 168)).resolves.toBe(history);

    expect(tauri.invoke.mock.calls).toEqual([
      ["ssh_inspect_host", { target }],
      ["machine_history", { endpointId: "endpoint-id", hours: 168 }],
    ]);
  });

  it("registers SSH progress before invoking the bootstrap command", async () => {
    const request: SshBootstrapInput = {
      target: { host: "server.test", user: "operator", port: 22 },
      acceptedHostKeys: ["server.test ssh-ed25519 test-key"],
      archivePath: "/tmp/rackio.tar.gz",
      checksumPath: "/tmp/rackio.tar.gz.sha256",
    };
    const progress: SshProgress = { stage: "uploading", detail: "Uploading release archive" };
    const installed = {
      pairingBundle: "rackio-pair:test-contract",
      remotePlatform: "Linux x86_64",
    };
    const onProgress = vi.fn();
    tauri.invoke.mockImplementationOnce((_command, payload) => {
      const channel = (payload as { onProgress: InstanceType<typeof tauri.Channel> }).onProgress;
      channel.onmessage(progress);
      return Promise.resolve(installed);
    });

    await expect(bootstrapSsh(request, onProgress)).resolves.toBe(installed);

    expect(onProgress).toHaveBeenCalledWith(progress);
    expect(tauri.invoke).toHaveBeenCalledWith("ssh_bootstrap", {
      request,
      onProgress: expect.any(tauri.Channel),
    });
  });

  it("uses the exact local save command and payload", async () => {
    tauri.invoke.mockResolvedValueOnce(undefined);

    await expect(savePairingBundle("/tmp/pairing.txt", "rackio-pair:test-contract")).resolves.toBe(
      undefined,
    );

    expect(tauri.invoke).toHaveBeenCalledWith("save_pairing_bundle", {
      path: "/tmp/pairing.txt",
      bundle: "rackio-pair:test-contract",
    });
  });

  it("passes backend rejection through to the caller", async () => {
    const rejection = new Error("daemon unavailable");
    tauri.invoke.mockRejectedValueOnce(rejection);

    await expect(fetchFleetSnapshot()).rejects.toBe(rejection);
  });
});
