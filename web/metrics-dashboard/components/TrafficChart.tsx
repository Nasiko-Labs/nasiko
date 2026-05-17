"use client";

import {
  Bar,
  BarChart,
  CartesianGrid,
  ResponsiveContainer,
  Tooltip,
  type TooltipProps,
  XAxis,
  YAxis,
} from "recharts";
import { formatHourLabel, formatLatencyMs } from "@/lib/format";
import type { MetricsHourlyPoint } from "@/lib/types";

type Props = {
  series: MetricsHourlyPoint[];
  loading?: boolean;
  noData?: boolean;
};

type BarRow = {
  label: string;
  ts: string;
  success: number;
  error: number;
  latency_ms: number | null;
};

function formatBucketRange(iso: string): string {
  const start = new Date(iso);
  const end = new Date(start.getTime() + 60 * 60 * 1000);
  const startLabel = start.toLocaleTimeString(undefined, {
    hour: "numeric",
    minute: "2-digit",
  });
  const endLabel = end.toLocaleTimeString(undefined, {
    hour: "numeric",
    minute: "2-digit",
  });
  return `${startLabel} – ${endLabel}`;
}

function TrafficTooltip({ active, payload }: TooltipProps<number, string>) {
  if (!active || !payload || payload.length === 0) {
    return null;
  }
  const row = payload[0].payload as BarRow;
  const total = row.success + row.error;
  const hasLatency = row.latency_ms != null && Number.isFinite(row.latency_ms);
  return (
    <div className="chart-tooltip" role="tooltip">
      <p className="chart-tooltip__time">{formatBucketRange(row.ts)}</p>
      <dl className="chart-tooltip__rows">
        <div>
          <dt>Total requests</dt>
          <dd>{total.toLocaleString()}</dd>
        </div>
        <div>
          <dt>
            <span className="legend-swatch legend-swatch--success" aria-hidden />
            Successful
          </dt>
          <dd>{row.success.toLocaleString()}</dd>
        </div>
        <div>
          <dt>
            <span className="legend-swatch legend-swatch--error" aria-hidden />
            Failed
          </dt>
          <dd className={row.error > 0 ? "chart-tooltip__err" : ""}>
            {row.error.toLocaleString()}
          </dd>
        </div>
        <div>
          <dt>Typical response</dt>
          <dd>{hasLatency ? formatLatencyMs(row.latency_ms) : "—"}</dd>
        </div>
      </dl>
    </div>
  );
}

export function TrafficChart({ series, loading, noData }: Props) {
  const data: BarRow[] = series.map((point) => ({
    label: formatHourLabel(point.ts),
    ts: point.ts,
    success: Math.max(0, point.success ?? 0),
    error: Math.max(0, point.error ?? 0),
    latency_ms: point.latency_ms,
  }));

  const totalRequests = data.reduce((sum, d) => sum + d.success + d.error, 0);
  const totalErrors = data.reduce((sum, d) => sum + d.error, 0);
  const hasTraffic = totalRequests > 0;

  if (loading) {
    return (
      <section className="panel" aria-busy="true">
        <h2 className="panel__title">How busy is your agent?</h2>
        <p className="panel__hint">Loading traffic chart…</p>
        <div className="chart-skeleton" aria-hidden="true" />
      </section>
    );
  }

  if (noData || !hasTraffic) {
    return (
      <section className="panel">
        <h2 className="panel__title">How busy is your agent?</h2>
        <p className="panel__hint">
          After customers start using this agent, you will see how many requests
          arrived each hour — and which of them succeeded or failed.
        </p>
      </section>
    );
  }

  return (
    <section className="panel" aria-label="Hourly traffic for the last 24 hours">
      <header className="panel__header">
        <div>
          <h2 className="panel__title">How busy is your agent?</h2>
          <p className="panel__hint">
            Requests per hour over the last 24h. Green is successful, red is failed.
          </p>
        </div>
        <ul className="chart-legend chart-legend--inline" aria-label="Chart legend">
          <li>
            <span className="legend-swatch legend-swatch--success" aria-hidden /> Successful
          </li>
          <li>
            <span className="legend-swatch legend-swatch--error" aria-hidden /> Failed
          </li>
        </ul>
      </header>

      <div className="chart-wrap">
        <ResponsiveContainer width="100%" height="100%">
          <BarChart
            data={data}
            margin={{ top: 8, right: 12, left: 0, bottom: 0 }}
            barCategoryGap={2}
          >
            <CartesianGrid stroke="var(--border)" strokeDasharray="3 3" vertical={false} />
            <XAxis
              dataKey="label"
              tick={{ fill: "var(--muted)", fontSize: 11 }}
              tickLine={false}
              axisLine={{ stroke: "var(--border)" }}
              interval="preserveStartEnd"
              minTickGap={28}
            />
            <YAxis
              tick={{ fill: "var(--muted)", fontSize: 11 }}
              tickLine={false}
              axisLine={false}
              width={36}
              allowDecimals={false}
              label={{
                value: "Requests",
                angle: -90,
                position: "insideLeft",
                offset: 12,
                style: { fill: "var(--muted)", fontSize: 11 },
              }}
            />
            <Tooltip
              cursor={{ fill: "var(--border)", opacity: 0.25 }}
              content={<TrafficTooltip />}
            />
            <Bar
              dataKey="success"
              stackId="traffic"
              fill="var(--ok)"
              radius={[0, 0, 0, 0]}
              isAnimationActive={false}
            />
            <Bar
              dataKey="error"
              stackId="traffic"
              fill="var(--bad)"
              radius={[4, 4, 0, 0]}
              isAnimationActive={false}
            />
          </BarChart>
        </ResponsiveContainer>
      </div>

      <p className="chart-summary" aria-live="polite">
        {totalRequests.toLocaleString()} total requests · {totalErrors.toLocaleString()} failed
        {totalRequests > 0 && (
          <>
            {" "}
            ({((totalErrors / totalRequests) * 100).toFixed(1)}% error rate)
          </>
        )}
      </p>
    </section>
  );
}
