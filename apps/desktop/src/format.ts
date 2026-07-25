export function percent(used?: number, total?: number): string {
  if (used === undefined || total === undefined || total === 0) return "—";
  return `${Math.round((used / total) * 100)}%`;
}

export function bytes(value?: number): string {
  if (value === undefined) return "—";
  const units = ["B", "KiB", "MiB", "GiB", "TiB"];
  let current = value;
  let index = 0;
  while (current >= 1024 && index < units.length - 1) {
    current /= 1024;
    index += 1;
  }
  return `${current.toFixed(index > 1 ? 1 : 0)} ${units[index]}`;
}
