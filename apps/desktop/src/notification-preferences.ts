import type { DesktopPreferencesView, NotificationState, NotificationThreshold } from "./types";

/** The renderer-local keys the notification choices lived under before the native store. */
export const LEGACY_THRESHOLD_KEY = "rackio.notification-threshold";
export const LEGACY_ENABLED_KEY = "rackio.notifications-enabled";

const THRESHOLDS: readonly NotificationThreshold[] = ["warning", "degraded", "critical", "offline"];

export interface LegacyNotificationPreferences {
  enabled: boolean;
  threshold: NotificationThreshold;
}

export function readLegacyNotificationPreferences(
  storage: Pick<Storage, "getItem">,
): LegacyNotificationPreferences {
  const stored = storage.getItem(LEGACY_THRESHOLD_KEY);
  const threshold = THRESHOLDS.find((candidate) => candidate === stored) ?? "critical";
  return { enabled: storage.getItem(LEGACY_ENABLED_KEY) === "true", threshold };
}

export function clearLegacyNotificationPreferences(storage: Pick<Storage, "removeItem">): void {
  storage.removeItem(LEGACY_THRESHOLD_KEY);
  storage.removeItem(LEGACY_ENABLED_KEY);
}

export interface PreferencesClient {
  fetchDesktopPreferences(): Promise<DesktopPreferencesView>;
  migrateLegacyPreferences(
    enabled: boolean,
    threshold: NotificationThreshold,
  ): Promise<DesktopPreferencesView>;
}

/**
 * Load the durable preferences, moving the legacy localStorage choices over
 * first when nothing has been persisted yet. The legacy values are removed
 * only after the migration is confirmed persisted, so a failed migration
 * leaves them in place for the next launch.
 */
export async function loadDesktopPreferences(
  client: PreferencesClient,
  storage: Pick<Storage, "getItem" | "removeItem">,
): Promise<DesktopPreferencesView> {
  let view = await client.fetchDesktopPreferences();
  if (!view.persisted && view.loadError === null) {
    const legacy = readLegacyNotificationPreferences(storage);
    view = await client.migrateLegacyPreferences(legacy.enabled, legacy.threshold);
  }
  if (view.persisted) clearLegacyNotificationPreferences(storage);
  return view;
}

/**
 * The state shown at startup. An enabled preference starts as `requesting` so
 * the operating system's permission is re-checked rather than assumed; an
 * unreadable store is shown as an error and never opts the user in.
 */
export function notificationStateFromPreferences(view: DesktopPreferencesView): NotificationState {
  const threshold = view.notificationThreshold;
  if (view.loadError !== null) {
    return {
      state: "error",
      threshold,
      message: `Saved preferences could not be read: ${view.loadError}`,
    };
  }
  return view.notificationsEnabled
    ? { state: "requesting", threshold }
    : { state: "disabled", threshold };
}
