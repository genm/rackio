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
