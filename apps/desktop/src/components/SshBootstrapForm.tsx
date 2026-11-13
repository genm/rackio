import { useEffect, useRef, useState } from "react";

import type { SshBootstrapInput, SshBootstrapStatus, SshTarget } from "../types";

export function SshBootstrapForm({
  status,
  onInspect,
  onInstall,
  onCancel,
}: {
  status: SshBootstrapStatus;
  onInspect: (target: SshTarget) => Promise<void>;
  onInstall: (input: SshBootstrapInput) => Promise<void>;
  onCancel: () => void;
}) {
  const [target, setTarget] = useState<SshTarget>({ host: "", user: "", port: 22 });
  const [archivePath, setArchivePath] = useState("");
  const [checksumPath, setChecksumPath] = useState("");
  const [hostKeyConfirmed, setHostKeyConfirmed] = useState(false);
  const hostKeyPanelRef = useRef<HTMLElement>(null);
  const busy = status.state === "checking_host" || status.state === "running";

  useEffect(() => {
    if (status.state === "confirming_host_key") hostKeyPanelRef.current?.focus();
  }, [status.state]);

  const inspect = async (event: React.FormEvent) => {
    event.preventDefault();
    setHostKeyConfirmed(false);
    await onInspect(target);
  };
  const install = async () => {
    if (status.state !== "confirming_host_key" || !hostKeyConfirmed) return;
    await onInstall({
      target,
      acceptedHostKeys: status.hostKeys,
      archivePath: archivePath.trim(),
      checksumPath: checksumPath.trim(),
    });
  };

  return (
    <form onSubmit={inspect} className="ssh-form">
      <p>
        Rackio uploads a local release archive, verifies it on the server, installs the systemd
        service, and pairs it automatically. The server does not need internet access.
      </p>
      <div className="field-grid">
        <label>
          Host
          <input
            value={target.host}
            onChange={(event) => setTarget({ ...target, host: event.target.value })}
            placeholder="server.test"
            required
          />
        </label>
        <label>
          SSH user
          <input
            value={target.user}
            onChange={(event) => setTarget({ ...target, user: event.target.value })}
            placeholder="operator"
            required
          />
        </label>
        <label>
          Port
          <input
            type="number"
            min="1"
            max="65535"
            value={target.port}
            onChange={(event) => setTarget({ ...target, port: Number(event.target.value) })}
            required
          />
        </label>
        <label>
          Identity file
          <input
            value={target.identityFile ?? ""}
            onChange={(event) =>
              setTarget({ ...target, identityFile: event.target.value || undefined })
            }
            placeholder="/Users/me/.ssh/id_ed25519"
          />
        </label>
      </div>
      <label>
        Release archive
        <input
          value={archivePath}
          onChange={(event) => setArchivePath(event.target.value)}
          placeholder="/path/to/rackio-v0.1.0-x86_64-unknown-linux-gnu.tar.gz"
          required
        />
      </label>
      <label>
        SHA-256 checksum file
        <input
          value={checksumPath}
          onChange={(event) => setChecksumPath(event.target.value)}
          placeholder="/path/to/rackio-v0.1.0-x86_64-unknown-linux-gnu.tar.gz.sha256"
          required
        />
      </label>
      {status.state === "confirming_host_key" ? (
        // This panel replaces the submit button that had focus, which would
        // otherwise drop focus to <body> on the one screen whose whole purpose
        // is deliberate human verification. Take focus here and announce the
        // fingerprints so keyboard and screen-reader users land on the thing
        // they are being asked to check.
        <section
          className="host-key-panel"
          aria-label="SSH host key confirmation"
          aria-live="polite"
          ref={hostKeyPanelRef}
          tabIndex={-1}
        >
          <strong>Confirm the server host key</strong>
          {status.fingerprints.map((fingerprint) => (
            <code key={fingerprint}>{fingerprint}</code>
          ))}
          <label className="confirmation">
            <input
              type="checkbox"
              checked={hostKeyConfirmed}
              onChange={(event) => setHostKeyConfirmed(event.target.checked)}
            />
            I verified this fingerprint through a trusted channel.
          </label>
        </section>
      ) : null}
      {status.state === "running" ? (
        <p className="progress-message" role="status">
          <span className="spinner" aria-hidden="true" />
          {status.detail}
        </p>
      ) : null}
      {status.state === "completed" ? (
        <p className="form-message success-message" role="status">
          {status.machineName} was installed and paired over P2P ({status.remotePlatform}).
        </p>
      ) : null}
      {status.state === "failed" ? (
        <p className="form-message error-message" role="alert">
          {status.message}
        </p>
      ) : null}
      <div className="dialog-actions">
        <button type="button" className="secondary-button" onClick={onCancel} disabled={busy}>
          Cancel
        </button>
        {status.state === "confirming_host_key" ? (
          <button
            type="button"
            className="pair-button"
            onClick={install}
            disabled={
              !hostKeyConfirmed ||
              archivePath.trim().length === 0 ||
              checksumPath.trim().length === 0
            }
          >
            Confirm and install
          </button>
        ) : (
          <button type="submit" className="pair-button" disabled={busy}>
            {status.state === "checking_host" ? "Checking host…" : "Check SSH host"}
          </button>
        )}
      </div>
    </form>
  );
}
