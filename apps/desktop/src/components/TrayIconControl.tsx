import { type FormEvent, useState } from "react";

import type { FleetNode } from "../types";

/** A starting point, not a limit: any single character or emoji is accepted. */
const SUGGESTED_ICONS = ["🖥️", "💻", "🎮", "🗄️", "📦", "🏠"];

type SaveStatus = { state: "idle" } | { state: "saving" } | { state: "error"; message: string };

/**
 * Chooses the one glyph a machine's menu-bar item shows. Machine names are
 * too wide for the menu bar once a rack has a few machines, so the tray shows
 * the name's initial unless the operator picks an icon here. Validation is the
 * desktop shell's: it owns the single-glyph rule and reports any rejection,
 * which is shown as-is.
 */
export function TrayIconControl({
  node,
  onChange,
}: {
  node: FleetNode;
  onChange: (icon: string | null) => Promise<void>;
}) {
  const [draft, setDraft] = useState("");
  const [status, setStatus] = useState<SaveStatus>({ state: "idle" });
  const saving = status.state === "saving";
  const custom = node.trayIcon ?? null;

  const save = async (icon: string | null) => {
    setStatus({ state: "saving" });
    try {
      await onChange(icon);
      setDraft("");
      setStatus({ state: "idle" });
    } catch (error: unknown) {
      setStatus({
        state: "error",
        message: error instanceof Error ? error.message : String(error),
      });
    }
  };

  const submit = (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault();
    void save(draft);
  };

  return (
    <details className="tray-icon">
      <summary title="The glyph this machine shows in the menu bar">
        <span>Menu bar icon</span>
        <span className="tray-icon-preview" data-testid="tray-label">
          {node.trayLabel}
        </span>
        <span className="tray-icon-source">{custom === null ? "initial" : "custom"}</span>
      </summary>
      <div className="tray-icon-body">
        <div className="tray-icon-presets" role="group" aria-label="Suggested icons">
          {SUGGESTED_ICONS.map((icon) => (
            <button
              key={icon}
              type="button"
              aria-label={`Use ${icon}`}
              aria-pressed={custom === icon}
              disabled={saving}
              onClick={() => void save(icon)}
            >
              {icon}
            </button>
          ))}
        </div>
        <form className="tray-icon-form" onSubmit={submit}>
          <input
            aria-label={`Custom menu bar icon for ${node.name}`}
            placeholder="One character or emoji"
            value={draft}
            disabled={saving}
            onChange={(event) => setDraft(event.target.value)}
          />
          <button type="submit" disabled={saving || draft.trim() === ""}>
            Set
          </button>
          <button
            type="button"
            disabled={saving || custom === null}
            onClick={() => void save(null)}
          >
            Use initial
          </button>
        </form>
        {status.state === "error" ? (
          <p className="tray-icon-error" role="alert">
            {status.message}
          </p>
        ) : null}
      </div>
    </details>
  );
}
