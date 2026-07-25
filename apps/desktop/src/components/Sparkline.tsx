interface SparklineProps {
  values: number[];
  label: string;
}

export function Sparkline({ values, label }: SparklineProps) {
  if (values.length < 2) {
    return <div className="sparkline sparkline-empty" aria-label={`${label}: no history yet`} />;
  }
  const max = Math.max(...values, 100);
  const points = values
    .map((value, index) => {
      const x = (index / (values.length - 1)) * 100;
      const y = 28 - (value / max) * 26;
      return `${x},${y}`;
    })
    .join(" ");
  return (
    <svg className="sparkline" viewBox="0 0 100 30" role="img" aria-label={label}>
      <polyline points={points} vectorEffect="non-scaling-stroke" />
    </svg>
  );
}
