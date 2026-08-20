import { useState } from "react";

import type {
  PairingShareState,
  PairingStatus,
  SshBootstrapInput,
  SshBootstrapStatus,
  SshTarget,
} from "../types";
import { useModalDialog } from "../useModalDialog";
import { PairingShare } from "./PairingShare";
import { SshBootstrapForm } from "./SshBootstrapForm";

type PairingMethod = "bundle" | "ssh" | "share";

const pairingMethods: { id: PairingMethod; label: string }[] = [
  { id: "bundle", label: "Pairing bundle" },
  { id: "ssh", label: "Install over SSH" },
  { id: "share", label: "Share this machine" },
];

function PairDialogSurface({
  onClose,
  children,
}: {
  onClose: () => void;
  children: React.ReactNode;
}) {
  const dialogRef = useModalDialog<HTMLElement>(onClose);
  return (
    <section
      ref={dialogRef}
      className="pair-dialog"
      role="dialog"
      aria-modal="true"
      aria-labelledby="pair-title"
      tabIndex={-1}
    >
      {children}
    </section>
  );
}

export function PairMachineControl({
  pairing,
  sshBootstrap,
  pairingShare,
  onPair,
  onInspectSshHost,
  onInstallViaSsh,
  onCreatePairingShare,
}: {
  pairing: PairingStatus;
  sshBootstrap: SshBootstrapStatus;
  pairingShare: PairingShareState;
  onPair: (bundle: string) => Promise<void>;
  onInspectSshHost: (target: SshTarget) => Promise<void>;
  onInstallViaSsh: (input: SshBootstrapInput) => Promise<void>;
  onCreatePairingShare: () => Promise<void>;
}) {
  // Keep the control mounted while its dialog is closed so an operator can
  // resume the selected transfer method and an unsubmitted bundle draft.
  const [pairingOpen, setPairingOpen] = useState(false);
  const [pairingMethod, setPairingMethod] = useState<PairingMethod>("bundle");
  const [bundle, setBundle] = useState("");
  // Escape and Cancel must agree: neither may abandon a pairing request that is
  // already in flight on the agent.
  const closePairing = () => {
    if (pairing.state === "submitting") return;
    setPairingOpen(false);
  };
  const moveTabFocus = (event: React.KeyboardEvent<HTMLDivElement>) => {
    const offset = event.key === "ArrowRight" ? 1 : event.key === "ArrowLeft" ? -1 : 0;
    if (offset === 0) return;
    event.preventDefault();
    const index = pairingMethods.findIndex((method) => method.id === pairingMethod);
    const next = pairingMethods[(index + offset + pairingMethods.length) % pairingMethods.length];
    setPairingMethod(next.id);
    event.currentTarget.querySelector<HTMLElement>(`#pair-tab-${next.id}`)?.focus();
  };
  const submitPairing = async (event: React.FormEvent) => {
    event.preventDefault();
    const normalized = bundle.trim();
    if (normalized.length === 0) return;
    try {
      await onPair(normalized);
      setBundle("");
      setPairingOpen(false);
    } catch {
      // The owning App surfaces the rejected operation through pairing state.
    }
  };
  const importPairingFile = async (event: React.ChangeEvent<HTMLInputElement>) => {
    const file = event.target.files?.[0];
    if (file === undefined) return;
    setBundle((await file.text()).trim());
  };

  return (
    <>
      <button type="button" className="pair-button" onClick={() => setPairingOpen(true)}>
        <span aria-hidden="true">＋</span> Pair machine
      </button>
      {pairingOpen ? (
        <div className="dialog-backdrop">
          <PairDialogSurface onClose={closePairing}>
            <div>
              <p className="eyebrow">E2E ENCRYPTED PAIRING</p>
              <h2 id="pair-title">Pair a machine</h2>
            </div>
            <div
              className="method-tabs"
              role="tablist"
              aria-label="Pairing method"
              onKeyDown={moveTabFocus}
            >
              {pairingMethods.map((method) => (
                <button
                  key={method.id}
                  type="button"
                  role="tab"
                  id={`pair-tab-${method.id}`}
                  aria-selected={pairingMethod === method.id}
                  aria-controls="pair-tabpanel"
                  tabIndex={pairingMethod === method.id ? 0 : -1}
                  onClick={() => setPairingMethod(method.id)}
                >
                  {method.label}
                </button>
              ))}
            </div>
            {/* Every panel contains its own focusable controls, so the panel
                itself stays out of the tab order. */}
            <div role="tabpanel" id="pair-tabpanel" aria-labelledby={`pair-tab-${pairingMethod}`}>
              {pairingMethod === "bundle" ? (
                <>
                  <p>
                    On the machine you want to monitor, run <code>rackio pairing create</code>, then
                    paste its one-time bundle here.
                  </p>
                  <form onSubmit={submitPairing}>
                    <label htmlFor="pairing-bundle">Pairing bundle</label>
                    <textarea
                      id="pairing-bundle"
                      name="pairing-bundle"
                      value={bundle}
                      onChange={(event) => setBundle(event.target.value)}
                      placeholder="rackio-pair:…"
                      autoComplete="off"
                      spellCheck={false}
                      autoFocus
                    />
                    <label className="file-import">
                      Or import a pairing file
                      <input type="file" accept=".txt,text/plain" onChange={importPairingFile} />
                    </label>
                    {pairing.state === "error" ? (
                      <p className="form-message error-message" role="alert">
                        {pairing.message}
                      </p>
                    ) : null}
                    {pairing.state === "success" ? (
                      <p className="form-message success-message" role="status">
                        {pairing.machineName} is paired.
                      </p>
                    ) : null}
                    <div className="dialog-actions">
                      <button
                        type="button"
                        className="secondary-button"
                        onClick={closePairing}
                        disabled={pairing.state === "submitting"}
                      >
                        Cancel
                      </button>
                      <button
                        type="submit"
                        className="pair-button"
                        disabled={pairing.state === "submitting" || bundle.trim().length === 0}
                      >
                        {pairing.state === "submitting" ? "Pairing…" : "Pair machine"}
                      </button>
                    </div>
                  </form>
                </>
              ) : pairingMethod === "ssh" ? (
                <SshBootstrapForm
                  status={sshBootstrap}
                  onInspect={onInspectSshHost}
                  onInstall={onInstallViaSsh}
                  onCancel={closePairing}
                />
              ) : (
                <PairingShare status={pairingShare} onCreate={onCreatePairingShare} />
              )}
            </div>
          </PairDialogSurface>
        </div>
      ) : null}
    </>
  );
}
