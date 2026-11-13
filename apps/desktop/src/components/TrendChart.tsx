import { useState } from "react";

import { type TrendDatum, type TrendLine, type TrendScale, gapThresholdMs } from "../trend-series";

interface TrendChartProps {
  lines: TrendLine[];
  /** How the y axis is bounded and labelled; see `trendScale`. */
  scale: TrendScale;
  label: string;
  /** Left end of the time axis, e.g. "4 min ago" or "09:30". */
  startLabel?: string;
  /** Right end of the time axis, e.g. "now" or "last contact". */
  endLabel?: string;
  emptyText?: string;
  /** Grey the plot out when the samples are no longer live. */
  muted?: boolean;
  /**
   * A real limit the hardware or the operator declared — never an invented
   * one. Drawn so "the line is high" can be read as "how close to the limit".
   */
  threshold?: { value: number; label: string };
  /** Formats a timestamp for the hover readout. */
  formatTime: (timestampMs: number) => string;
}

const plottedPointCount = (lines: TrendLine[]) =>
  lines.reduce((total, line) => total + line.points.length, 0);

/** Split a line wherever the samples skip more than their usual spacing. */
function segmentsOf(points: TrendDatum[]): TrendDatum[][] {
  const threshold = gapThresholdMs(points);
  const segments: TrendDatum[][] = [];
  let current: TrendDatum[] = [];
  for (const point of points) {
    const previous = current[current.length - 1];
    if (previous !== undefined && point.timestampMs - previous.timestampMs > threshold) {
      segments.push(current);
      current = [];
    }
    current.push(point);
  }
  if (current.length > 0) segments.push(current);
  return segments;
}

/**
 * A metric trend on an explicit, labelled scale and a real time axis.
 *
 * X is proportional to the timestamp rather than to the sample index, and a
 * run of samples that skips its usual spacing is drawn as separate segments:
 * a machine that went quiet must leave a hole, not a straight line across the
 * outage. Bounded units keep a fixed ceiling — a flat line at 5% must look
 * calm and a line at 95% must look loaded, which auto-fitting would erase.
 */
export function TrendChart({
  lines,
  scale,
  label,
  startLabel,
  endLabel,
  emptyText = "Collecting samples…",
  muted = false,
  threshold,
  formatTime,
}: TrendChartProps) {
  const [hoverRatio, setHoverRatio] = useState<number | null>(null);
  if (plottedPointCount(lines) < 2) {
    return (
      <div className="trend trend-empty" role="img" aria-label={`${label}: no samples yet`}>
        <span>{emptyText}</span>
      </div>
    );
  }
  const timestamps = lines.flatMap((line) => line.points.map((point) => point.timestampMs));
  const startMs = Math.min(...timestamps);
  const endMs = Math.max(...timestamps);
  const span = endMs - startMs;
  // Every sample landing in the same millisecond still has to be drawable, so
  // a zero span collapses to the right edge rather than dividing by zero.
  const xOf = (timestampMs: number) => (span === 0 ? 100 : ((timestampMs - startMs) / span) * 100);
  const yOf = (value: number) => 100 - (Math.min(scale.max, Math.max(0, value)) / scale.max) * 100;
  const hoverMs = hoverRatio === null ? null : startMs + hoverRatio * span;
  const readouts =
    hoverMs === null
      ? []
      : lines.flatMap((line) => {
          if (line.points.length === 0) return [];
          const nearest = line.points.reduce((best, point) =>
            Math.abs(point.timestampMs - hoverMs) < Math.abs(best.timestampMs - hoverMs)
              ? point
              : best,
          );
          return [{ name: line.name, point: nearest }];
        });
  const hoverTimestamp = readouts[0]?.point.timestampMs;
  return (
    <figure className={`trend${muted ? " trend-muted" : ""}`}>
      <div
        className="trend-plot"
        role="img"
        aria-label={label}
        onPointerMove={(event) => {
          const box = event.currentTarget.getBoundingClientRect();
          setHoverRatio(Math.min(1, Math.max(0, (event.clientX - box.left) / box.width)));
        }}
        onPointerLeave={() => setHoverRatio(null)}
      >
        <svg viewBox="0 0 100 100" preserveAspectRatio="none" aria-hidden="true">
          <line className="trend-gridline" x1="0" y1="50" x2="100" y2="50" />
          {threshold === undefined || threshold.value > scale.max ? null : (
            <line
              className="trend-threshold"
              x1="0"
              y1={yOf(threshold.value)}
              x2="100"
              y2={yOf(threshold.value)}
            />
          )}
          {lines.map((line, lineIndex) =>
            segmentsOf(line.points).map((segment, segmentIndex) => {
              const path = segment
                .map((point) => `${xOf(point.timestampMs)},${yOf(point.value)}`)
                .join(" ");
              // The fill is dropped for multi-line metrics: two translucent
              // areas stacked on each other read as a third value that no
              // sample reports.
              const filled = lines.length === 1 && segment.length > 1;
              const firstX = xOf(segment[0].timestampMs);
              const lastX = xOf(segment[segment.length - 1].timestampMs);
              return (
                <g key={`${line.name}-${segmentIndex}`} className={`trend-series-${lineIndex}`}>
                  {filled ? (
                    <path
                      className="trend-area"
                      d={`M${firstX},100 L${path.split(" ").join(" L")} L${lastX},100 Z`}
                    />
                  ) : null}
                  <polyline
                    className="trend-line"
                    points={path}
                    vectorEffect="non-scaling-stroke"
                  />
                </g>
              );
            }),
          )}
        </svg>
        {threshold === undefined || threshold.value > scale.max ? null : (
          <span className="trend-threshold-label" style={{ top: `${yOf(threshold.value)}%` }}>
            {threshold.label}
          </span>
        )}
        <span className="trend-scale trend-scale-top">{scale.topLabel}</span>
        <span className="trend-scale trend-scale-mid">{scale.midLabel}</span>
        <span className="trend-scale trend-scale-bottom">0</span>
        {lines.map((line, lineIndex) => {
          const last = line.points[line.points.length - 1];
          if (last === undefined) return null;
          return (
            <span
              key={line.name}
              className={`trend-dot trend-series-${lineIndex}`}
              style={{ left: `${xOf(last.timestampMs)}%`, top: `${yOf(last.value)}%` }}
              aria-hidden="true"
            />
          );
        })}
        {hoverTimestamp === undefined ? null : (
          <>
            <span
              className="trend-crosshair"
              style={{ left: `${xOf(hoverTimestamp)}%` }}
              aria-hidden="true"
            />
            {readouts.map(({ name, point }, lineIndex) => (
              <span
                key={`marker-${name}`}
                className={`trend-marker trend-series-${lineIndex}`}
                style={{ left: `${xOf(point.timestampMs)}%`, top: `${yOf(point.value)}%` }}
                aria-hidden="true"
              />
            ))}
            <span
              className={`trend-tooltip${xOf(hoverTimestamp) > 55 ? " trend-tooltip-left" : ""}`}
              style={{ left: `${xOf(hoverTimestamp)}%` }}
              role="status"
            >
              <span className="trend-tooltip-time">{formatTime(hoverTimestamp)}</span>
              {readouts.map(({ name, point }, lineIndex) => (
                <span key={name} className={`trend-tooltip-row trend-series-${lineIndex}`}>
                  {lines.length > 1 ? `${name} ` : ""}
                  {scale.format(point.value)}
                </span>
              ))}
            </span>
          </>
        )}
      </div>
      <figcaption className="trend-axis">
        <span>{startLabel}</span>
        {lines.length > 1 ? (
          <span className="trend-legend">
            {lines.map((line, lineIndex) => (
              <span key={line.name} className={`trend-series-${lineIndex}`}>
                {line.name}
              </span>
            ))}
          </span>
        ) : null}
        <span>{endLabel}</span>
      </figcaption>
    </figure>
  );
}
