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

/**
 * How long a machine has been up, e.g. "12d 4h", "3h 20m", "45s".
 *
 * Two units at most: an operator reads uptime to tell a machine that recovered
 * from one that never restarted, and seconds of precision on a twelve-day
 * uptime serves neither. An unknown uptime is an em dash — the agent withholds
 * the wire's non-optional zero precisely so this cannot render as "0s" on a
 * machine that never reported one.
 */
export function uptime(seconds?: number | null): string {
  if (seconds == null || !Number.isFinite(seconds) || seconds < 0) return "—";
  const whole = Math.floor(seconds);
  const days = Math.floor(whole / 86_400);
  const hours = Math.floor((whole % 86_400) / 3_600);
  const minutes = Math.floor((whole % 3_600) / 60);
  if (days > 0) return `${days}d ${hours}h`;
  if (hours > 0) return `${hours}h ${minutes}m`;
  if (minutes > 0) return `${minutes}m`;
  return `${whole}s`;
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

export function bytesPerSecond(value?: number | null): string {
  if (value == null) return "—";
  return `${bytes(value)}/s`;
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
