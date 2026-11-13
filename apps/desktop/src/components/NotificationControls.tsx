import type { NotificationState, NotificationThreshold } from "../types";

export function NotificationControls({
  status,
  onEnable,
  onDisable,
  onThresholdChange,
}: {
  status: NotificationState;
  onEnable: () => Promise<void>;
  onDisable: () => void;
  onThresholdChange: (threshold: NotificationThreshold) => void;
}) {
  const active = status.state === "enabled";
  return (
    <section className="notification-controls" aria-label="Alert notifications">
      <label>
        Notify at
        <select
          value={status.threshold}
          onChange={(event) => onThresholdChange(event.target.value as NotificationThreshold)}
        >
          <option value="warning">Warning</option>
          <option value="degraded">Degraded</option>
          <option value="critical">Critical</option>
          <option value="offline">Offline</option>
        </select>
      </label>
      <button
        type="button"
        className="secondary-button notification-toggle"
        onClick={() => (active ? onDisable() : void onEnable())}
        disabled={status.state === "requesting"}
        aria-pressed={active}
      >
        {status.state === "requesting"
          ? "Requesting…"
          : active
            ? "Notifications on"
            : "Notifications off"}
      </button>
      {status.state === "denied" || status.state === "error" ? (
        <span className="notification-error" role="status">
          {status.message}
        </span>
      ) : null}
    </section>
  );
}
