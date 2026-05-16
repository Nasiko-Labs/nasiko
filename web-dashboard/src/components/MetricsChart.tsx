import { MetricsHourBucket } from "../api/metrics";

interface MetricsChartProps {
  title: string;
  data: MetricsHourBucket[];
  valueKey: keyof Pick<
    MetricsHourBucket,
    "trace_count" | "total_cost" | "total_tokens" | "session_count"
  >;
  formatValue?: (n: number) => string;
  color?: string;
}

export function MetricsChart({
  title,
  data,
  valueKey,
  formatValue = (n) => String(n),
  color = "var(--nasiko-primary)",
}: MetricsChartProps) {
  const values = data.map((d) => Number(d[valueKey]) || 0);
  const max = Math.max(...values, 1);

  return (
    <div className="chart-card">
      <h3 className="chart-title">{title}</h3>
      <div className="chart-bars" role="img" aria-label={title}>
        {data.map((bucket) => {
          const value = Number(bucket[valueKey]) || 0;
          const heightPct = (value / max) * 100;
          return (
            <div
              key={bucket.hour}
              className="chart-bar-wrap"
              title={`${bucket.label}: ${formatValue(value)}`}
            >
              <div
                className="chart-bar"
                style={{
                  height: `${Math.max(heightPct, value > 0 ? 4 : 0)}%`,
                  background: color,
                }}
              />
              <span className="chart-bar-label">{bucket.label}</span>
            </div>
          );
        })}
      </div>
    </div>
  );
}
