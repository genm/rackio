import { describe, expect, it, vi } from "vitest";

import {
  LEGACY_ENABLED_KEY,
  LEGACY_THRESHOLD_KEY,
  loadDesktopPreferences,
  notificationStateFromPreferences,
  readLegacyNotificationPreferences,
} from "./notification-preferences";
import type { DesktopPreferencesView } from "./types";

function view(overrides: Partial<DesktopPreferencesView> = {}): DesktopPreferencesView {
  return {
    version: 1,
    notificationsEnabled: false,
    notificationThreshold: "critical",
    appearance: "system",
    loadError: null,
    persisted: true,
    ...overrides,
  };
}

function storage(entries: Record<string, string> = {}) {
  const map = new Map(Object.entries(entries));
  return {
    getItem: (key: string) => map.get(key) ?? null,
    removeItem: (key: string) => void map.delete(key),
    has: (key: string) => map.has(key),
  };
}

describe("legacy notification preferences", () => {
  it("reads the stored choice and ignores a threshold it does not recognise", () => {
    expect(
      readLegacyNotificationPreferences(
        storage({ [LEGACY_ENABLED_KEY]: "true", [LEGACY_THRESHOLD_KEY]: "offline" }),
      ),
    ).toEqual({ enabled: true, threshold: "offline" });
    expect(readLegacyNotificationPreferences(storage({ [LEGACY_THRESHOLD_KEY]: "bogus" }))).toEqual(
      { enabled: false, threshold: "critical" },
    );
  });
});

describe("loading desktop preferences", () => {
  it("migrates the legacy choice once, then drops the legacy keys", async () => {
    const local = storage({ [LEGACY_ENABLED_KEY]: "true", [LEGACY_THRESHOLD_KEY]: "warning" });
    const client = {
      fetchDesktopPreferences: vi.fn().mockResolvedValue(view({ persisted: false })),
      migrateLegacyPreferences: vi
        .fn()
        .mockResolvedValue(view({ notificationsEnabled: true, notificationThreshold: "warning" })),
    };
    const loaded = await loadDesktopPreferences(client, local);
    expect(client.migrateLegacyPreferences).toHaveBeenCalledWith(true, "warning");
    expect(loaded.notificationsEnabled).toBe(true);
    expect(local.has(LEGACY_ENABLED_KEY)).toBe(false);
    expect(local.has(LEGACY_THRESHOLD_KEY)).toBe(false);
  });

  it("does not migrate again once preferences are persisted", async () => {
    const local = storage({ [LEGACY_ENABLED_KEY]: "true" });
    const client = {
      fetchDesktopPreferences: vi.fn().mockResolvedValue(view()),
      migrateLegacyPreferences: vi.fn(),
    };
    const loaded = await loadDesktopPreferences(client, local);
    expect(client.migrateLegacyPreferences).not.toHaveBeenCalled();
    expect(loaded.notificationsEnabled).toBe(false);
  });

  it("keeps the legacy values when the migration fails", async () => {
    const local = storage({ [LEGACY_ENABLED_KEY]: "true" });
    const client = {
      fetchDesktopPreferences: vi.fn().mockResolvedValue(view({ persisted: false })),
      migrateLegacyPreferences: vi.fn().mockRejectedValue(new Error("disk full")),
    };
    await expect(loadDesktopPreferences(client, local)).rejects.toThrow("disk full");
    expect(local.has(LEGACY_ENABLED_KEY)).toBe(true);
  });

  it("leaves the legacy values alone when the saved file is unreadable", async () => {
    const local = storage({ [LEGACY_ENABLED_KEY]: "true" });
    const client = {
      fetchDesktopPreferences: vi
        .fn()
        .mockResolvedValue(view({ persisted: false, loadError: "not valid" })),
      migrateLegacyPreferences: vi.fn(),
    };
    await loadDesktopPreferences(client, local);
    expect(client.migrateLegacyPreferences).not.toHaveBeenCalled();
    expect(local.has(LEGACY_ENABLED_KEY)).toBe(true);
  });
});

describe("startup notification state", () => {
  it("re-checks permission for an enabled preference instead of assuming it", () => {
    expect(
      notificationStateFromPreferences(
        view({ notificationsEnabled: true, notificationThreshold: "degraded" }),
      ),
    ).toEqual({ state: "requesting", threshold: "degraded" });
  });

  it("starts disabled by default", () => {
    expect(notificationStateFromPreferences(view())).toEqual({
      state: "disabled",
      threshold: "critical",
    });
  });

  it("shows an unreadable store as an error and never opts the user in", () => {
    const state = notificationStateFromPreferences(
      view({ loadError: "bad json", notificationsEnabled: true }),
    );
    expect(state.state).toBe("error");
  });
});
