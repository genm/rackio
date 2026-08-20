export function percent(used?: number | null, total?: number | null): string {
  if (used == null || total == null || total === 0) return "—";
  return `${Math.round((used / total) * 100)}%`;
}

/**
 * A machine with no readable sensor shows an em dash. Rendering it as 0 °C
 * would present an unreadable source as a frozen machine, and the reading is
 * rounded rather than truncated so 61.6 °C does not read as 61 °C.
 */
export function celsius(value?: number | null): string {
  if (value == null || !Number.isFinite(value)) return "—";
  return `${Math.round(value)} °C`;
}

/** "45 s" below 90 seconds, whole minutes above — chart-axis granularity. */
export function shortDuration(seconds: number): string {
  if (seconds < 90) return `${Math.round(seconds)} s`;
  return `${Math.round(seconds / 60)} min`;
}

/** Wall-clock label for a history axis, e.g. "09:30". */
export function timeOfDay(ms: number): string {
  return new Date(ms).toLocaleTimeString(undefined, {
    hour: "2-digit",
    minute: "2-digit",
    hour12: false,
  });
}

export function ago(thenMs: number, nowMs: number): string {
  const seconds = Math.max(0, Math.round((nowMs - thenMs) / 1000));
  if (seconds < 60) return `${seconds} s ago`;
  const minutes = Math.round(seconds / 60);
  if (minutes < 60) return `${minutes} min ago`;
  const hours = Math.round(minutes / 60);
  if (hours < 48) return `${hours} h ago`;
  return `${Math.round(hours / 24)} d ago`;
}

export function bytes(value?: number | null): string {
  if (value == null) return "—";
  const units = ["B", "KiB", "MiB", "GiB", "TiB"];
  let current = value;
  let index = 0;
  while (current >= 1024 && index < units.length - 1) {
    current /= 1024;
    index += 1;
  }
  return `${current.toFixed(index > 1 ? 1 : 0)} ${units[index]}`;
}

/** A byte count rendered as a rate, e.g. "1.2 MiB/s". */
export function bytesPerSecond(value?: number | null): string {
  if (value == null) return "—";
  return `${bytes(value)}/s`;
}
