import { invoke } from "@tauri-apps/api/core";
import { save as chooseSavePath } from "@tauri-apps/plugin-dialog";
import { useState } from "react";

import type { PairingShareState } from "../types";

export function PairingShare({
  status,
  onCreate,
}: {
  status: PairingShareState;
  onCreate: () => Promise<void>;
}) {
  const [message, setMessage] = useState("");
  const copy = async (bundle: string) => {
    try {
      await navigator.clipboard.writeText(bundle);
      setMessage("Pairing bundle copied.");
    } catch (error: unknown) {
      setMessage(error instanceof Error ? error.message : String(error));
    }
  };
  const save = async (bundle: string) => {
    try {
      const path = await chooseSavePath({
        defaultPath: "rackio-pairing-bundle.txt",
        filters: [{ name: "Rackio pairing bundle", extensions: ["txt"] }],
      });
      if (path === null) return;
      await invoke("save_pairing_bundle", { path, bundle });
      setMessage("Pairing bundle saved.");
    } catch (error: unknown) {
      setMessage(error instanceof Error ? error.message : String(error));
    }
  };

  if (status.state === "idle") {
    return (
      <section className="pairing-share">
        <p>
          Open a five-minute pairing window for another trusted Rackio machine. The bundle is
          generated locally and is never sent to a cloud service.
        </p>
        <button type="button" className="pair-button" onClick={() => void onCreate()}>
          Create pairing window
        </button>
      </section>
    );
  }
  if (status.state === "loading") {
    return (
      <p className="progress-message" role="status">
        <span className="spinner" aria-hidden="true" />
        Opening the local pairing window…
      </p>
    );
  }
  if (status.state === "error") {
    return (
      <section className="pairing-share">
        <p className="form-message error-message" role="alert">
          {status.message}
        </p>
        <button type="button" className="pair-button" onClick={() => void onCreate()}>
          Try again
        </button>
      </section>
    );
  }
  return (
    <section className="pairing-share">
      <p>
        Scan this QR code from the trusted viewer, or transfer the file directly. It expires after
        five minutes and works once.
      </p>
      {status.qrDataUrl ? (
        <img src={status.qrDataUrl} alt="One-time Rackio pairing QR code" />
      ) : (
        <p className="form-message warning-message" role="status">
          {status.qrError ?? "QR generation is unavailable. Use the file or copy the bundle."}
        </p>
      )}
      <code className="bundle-preview">{status.bundle}</code>
      <div className="dialog-actions">
        <button type="button" className="secondary-button" onClick={() => void save(status.bundle)}>
          Save file
        </button>
        <button type="button" className="pair-button" onClick={() => void copy(status.bundle)}>
          Copy bundle
        </button>
      </div>
      {message ? (
        <p className="form-message" role="status">
          {message}
        </p>
      ) : null}
    </section>
  );
}
