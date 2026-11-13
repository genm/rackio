import { useState } from "react";

import { type TrendMetric, trendMetricRegistry } from "./trend-series";

const STORAGE_KEY = "rackio.card-metrics";

function isMetric(value: unknown): value is TrendMetric {
  return typeof value === "string" && value in trendMetricRegistry;
}

/** Discard anything that is not a known metric so a stale or hand-edited
 *  entry cannot select a chart the registry no longer has. */
function readStored(): Record<string, TrendMetric> {
  try {
    const raw = window.localStorage.getItem(STORAGE_KEY);
    if (raw === null) return {};
    const parsed: unknown = JSON.parse(raw);
    if (typeof parsed !== "object" || parsed === null) return {};
    return Object.fromEntries(
      Object.entries(parsed as Record<string, unknown>).filter(([, metric]) => isMetric(metric)),
    ) as Record<string, TrendMetric>;
  } catch {
    // A corrupt or unavailable store must not stop the dashboard rendering;
    // the cards fall back to their default metric.
    return {};
  }
}

/**
 * Remembers which metric each card was left showing, so a rack owner who
 * watches memory on one machine finds it there on the next launch.
 */
export function useStoredMetricMap(): [
  Record<string, TrendMetric>,
  (machineId: string, metric: TrendMetric) => void,
] {
  const [metrics, setMetrics] = useState<Record<string, TrendMetric>>(readStored);
  const select = (machineId: string, metric: TrendMetric) => {
    setMetrics((current) => {
      const next = { ...current, [machineId]: metric };
      try {
        window.localStorage.setItem(STORAGE_KEY, JSON.stringify(next));
      } catch {
        // Persisting is a convenience; losing it must not lose the selection
        // for this session.
      }
      return next;
    });
  };
  return [metrics, select];
}
