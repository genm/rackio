interface TrendChartProps {
  /** Percentage samples on a fixed 0–100 scale, oldest first. */
  values: number[];
  label: string;
  /** Left end of the time axis, e.g. "4 min ago" or "09:30". */
  startLabel?: string;
  /** Right end of the time axis, e.g. "now" or "last contact". */
  endLabel?: string;
  emptyText?: string;
  /** Grey the plot out when the samples are no longer live. */
  muted?: boolean;
}

/**
 * A percentage trend on a fixed 0–100 scale. The scale never auto-fits: a
 * flat line at 5% must look calm and a line at 95% must look loaded, which
 * auto-scaling would erase.
 */
export function TrendChart({
  values,
  label,
  startLabel,
  endLabel,
  emptyText = "Collecting samples…",
  muted = false,
}: TrendChartProps) {
  if (values.length < 2) {
    return (
      <div className="trend trend-empty" role="img" aria-label={`${label}: no samples yet`}>
        <span>{emptyText}</span>
      </div>
    );
  }
  const clamped = values.map((value) => Math.min(100, Math.max(0, value)));
  const coordinates = clamped.map((value, index) => ({
    x: (index / (clamped.length - 1)) * 100,
    y: 100 - value,
  }));
  const line = coordinates.map(({ x, y }) => `${x},${y}`).join(" ");
  const area = `M0,100 L${line.split(" ").join(" L")} L100,100 Z`;
  const last = coordinates[coordinates.length - 1];
  return (
    <figure className={`trend${muted ? " trend-muted" : ""}`} role="img" aria-label={label}>
      <div className="trend-plot">
        <svg viewBox="0 0 100 100" preserveAspectRatio="none" aria-hidden="true">
          <line className="trend-gridline" x1="0" y1="50" x2="100" y2="50" />
          <path className="trend-area" d={area} />
          <polyline className="trend-line" points={line} vectorEffect="non-scaling-stroke" />
        </svg>
        <span className="trend-scale trend-scale-top">100%</span>
        <span className="trend-scale trend-scale-mid">50</span>
        <span className="trend-scale trend-scale-bottom">0</span>
        <span
          className="trend-dot"
          style={{ left: `${last.x}%`, top: `${last.y}%` }}
          aria-hidden="true"
        />
      </div>
      {startLabel !== undefined || endLabel !== undefined ? (
        <figcaption className="trend-axis">
          <span>{startLabel}</span>
          <span>{endLabel}</span>
        </figcaption>
      ) : null}
    </figure>
  );
}
