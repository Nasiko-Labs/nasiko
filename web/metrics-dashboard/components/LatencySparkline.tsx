"use client";

import { Area, AreaChart, ResponsiveContainer } from "recharts";
import type { MetricsHourlyPoint } from "@/lib/types";

type Props = {
  series: MetricsHourlyPoint[] | undefined | null;
  ariaLabel?: string;
};

type Row = { i: number; latency_ms: number | null };

/**
 * Tiny 24-point area chart embedded inside the "Typical response time" KPI card.
 * No axes, no tooltips — its job is to show whether latency is steady, climbing,
 * or spiky at a glance. `connectNulls` keeps the line continuous even when an
 * hour has no traffic.
 */
export function LatencySparkline({ series, ariaLabel }: Props) {
  const points = series ?? [];
  const data: Row[] = points.map((p, i) => ({ i, latency_ms: p.latency_ms }));
  const hasPoints = data.some(
    (d) => d.latency_ms != null && Number.isFinite(d.latency_ms),
  );

  if (!hasPoints) {
    return (
      <div
        className="sparkline sparkline--empty"
        role="img"
        aria-label={ariaLabel ?? "No latency data yet"}
      >
        <span aria-hidden>—</span>
      </div>
    );
  }

  return (
    <div
      className="sparkline"
      role="img"
      aria-label={ariaLabel ?? "24-hour latency trend"}
    >
      <ResponsiveContainer width="100%" height="100%">
        <AreaChart data={data} margin={{ top: 4, right: 0, left: 0, bottom: 0 }}>
          <defs>
            <linearGradient id="sparkline-fill" x1="0" y1="0" x2="0" y2="1">
              <stop offset="0%" stopColor="var(--accent)" stopOpacity={0.45} />
              <stop offset="100%" stopColor="var(--accent)" stopOpacity={0} />
            </linearGradient>
          </defs>
          <Area
            type="monotone"
            dataKey="latency_ms"
            stroke="var(--accent)"
            strokeWidth={1.5}
            fill="url(#sparkline-fill)"
            connectNulls
            isAnimationActive={false}
            dot={false}
          />
        </AreaChart>
      </ResponsiveContainer>
    </div>
  );
}
