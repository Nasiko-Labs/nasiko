"use client";

import * as React from "react";
import { cn } from "../lib/cn";
import {
  Area,
  AreaChart,
  Bar,
  BarChart,
  CartesianGrid,
  ComposedChart,
  Line,
  LineChart,
  XAxis,
  YAxis,
} from "recharts";
import {
  ChartContainer,
  ChartLegend,
  ChartLegendContent,
  ChartTooltip,
  ChartTooltipContent,
  type ChartConfig,
} from "./chart";
import { useMotion } from "../motion/provider";

/**
 * Data-aware series charts (contract §8 "make composition data-aware"). The
 * caller supplies rows and typed series; the wrapper owns colours (tokens),
 * axes, legend, tooltip, the a11y layer and the motion policy. Nothing here
 * accepts a duration, a colour string from outside the token set, or free HTML.
 *
 * Why wrappers rather than exposing Recharts: the generator and authored
 * screens pick a *shape* and a *semantic*; every chart then reads as one system.
 */

export type SeriesPoint = Readonly<Record<string, string | number | null>>;

export interface SeriesSpec {
  /** Key in each row. */
  readonly key: string;
  /** Legend/tooltip label. */
  readonly label: string;
  /** Token slot 1–5 → `--chart-N`. */
  readonly slot?: 1 | 2 | 3 | 4 | 5;
}

export interface SeriesChartProps {
  readonly data: readonly SeriesPoint[];
  readonly series: readonly SeriesSpec[];
  /** Key of the category / time axis in each row. */
  readonly xKey: string;
  /** Formats axis ticks and the tooltip label. */
  readonly formatX?: (v: string | number) => string;
  /** Formats values (units, currency). Money must come through a formatter that keeps zero/unknown distinct. */
  readonly formatY?: (v: number) => string;
  readonly height?: number;
  readonly className?: string;
  /** Accessible summary read by assistive tech. Required: a chart is never an unlabeled picture. */
  readonly description: string;
  /** Stacked bars/areas. */
  readonly stacked?: boolean;
  /** Upper bound on points per series rendered; the caller downsamples above it. */
  readonly maxPoints?: number;
}

export const MAX_SERIES = 5;
export const DEFAULT_MAX_POINTS = 1_000;

function useChartMotion() {
  const { mode, enter } = useMotion();
  // One bounded first reveal under full; none under reduced/none (ADR-0003).
  return { isAnimationActive: mode === "full" && enter, animationDuration: 220 };
}

function configFor(series: readonly SeriesSpec[]): ChartConfig {
  const cfg: Record<string, { label: string; color: string }> = {};
  series.slice(0, MAX_SERIES).forEach((s, i) => {
    cfg[s.key] = { label: s.label, color: `var(--chart-${s.slot ?? (i % 5) + 1})` };
  });
  return cfg;
}

export type ChartShape = "line" | "area" | "bar" | "combo" | "donut";

function Frame({
  description,
  className,
  height = 240,
  config,
  shape,
  children,
}: {
  description: string;
  className?: string | undefined;
  height?: number | undefined;
  config: ChartConfig;
  /** Exposed as `data-chart-shape` so hosts and tests can assert the drawn shape. */
  shape: ChartShape;
  children: React.ReactElement;
}) {
  return (
    <ChartContainer
      config={config}
      className={cn("aspect-auto w-full", className)}
      style={{ height }}
      role="img"
      aria-label={description}
      data-chart-shape={shape}
      initialDimension={{ width: 600, height }}
    >
      {children}
    </ChartContainer>
  );
}

export function TimeSeriesChart({
  data,
  series,
  xKey,
  formatX,
  formatY,
  height,
  className,
  description,
  stacked = false,
  area = false,
  maxPoints = DEFAULT_MAX_POINTS,
}: SeriesChartProps & {
  /** Filled areas instead of lines (`stacked` implies areas, stacked on one axis). */
  readonly area?: boolean;
}) {
  const anim = useChartMotion();
  const rows = data.length > maxPoints ? data.slice(data.length - maxPoints) : data;
  const cfg = configFor(series);
  const filled = stacked || area;
  const Chart = filled ? AreaChart : LineChart;
  const fx = formatX ?? ((v: string | number) => String(v));
  const fy = formatY ?? ((v: number) => String(v));
  return (
    <Frame
      description={description}
      className={className}
      height={height}
      config={cfg}
      shape={filled ? "area" : "line"}
    >
      <Chart data={[...rows]} accessibilityLayer margin={{ left: 8, right: 8, top: 8, bottom: 0 }}>
        <CartesianGrid vertical={false} stroke="var(--chart-grid)" />
        <XAxis
          dataKey={xKey}
          tickLine={false}
          axisLine={false}
          minTickGap={24}
          tickFormatter={(v: string | number) => fx(v)}
        />
        <YAxis tickLine={false} axisLine={false} width={48} tickFormatter={(v: number) => fy(v)} />
        <ChartTooltip
          cursor={{ stroke: "var(--chart-crosshair)" }}
          content={
            <ChartTooltipContent
              labelFormatter={(l) => (typeof l === "string" || typeof l === "number" ? fx(l) : "")}
              formatter={(v) => fy(Number(v))}
            />
          }
        />
        {series.length > 1 ? <ChartLegend content={<ChartLegendContent />} /> : null}
        {series
          .slice(0, MAX_SERIES)
          .map((s) =>
            filled ? (
              <Area
                key={s.key}
                dataKey={s.key}
                type="monotone"
                {...(stacked ? { stackId: "a" } : {})}
                stroke={`var(--color-${s.key})`}
                fill={`var(--color-${s.key})`}
                fillOpacity={0.25}
                strokeWidth={2}
                dot={false}
                connectNulls={false}
                {...anim}
              />
            ) : (
              <Line
                key={s.key}
                dataKey={s.key}
                type="monotone"
                stroke={`var(--color-${s.key})`}
                strokeWidth={2}
                dot={false}
                connectNulls={false}
                {...anim}
              />
            ),
          )}
      </Chart>
    </Frame>
  );
}

export function CategoryBarChart({
  data,
  series,
  xKey,
  formatX,
  formatY,
  height,
  className,
  description,
  stacked = false,
  maxPoints = 60,
}: SeriesChartProps) {
  const anim = useChartMotion();
  // Categorical cardinality: above the cap the caller must already have grouped
  // into top-N + "other"; we never silently drop categories mid-chart.
  const rows = data.slice(0, maxPoints);
  const cfg = configFor(series);
  const fx = formatX ?? ((v: string | number) => String(v));
  const fy = formatY ?? ((v: number) => String(v));
  return (
    <Frame description={description} className={className} height={height} config={cfg} shape="bar">
      <BarChart
        data={[...rows]}
        accessibilityLayer
        margin={{ left: 8, right: 8, top: 8, bottom: 0 }}
      >
        <CartesianGrid vertical={false} stroke="var(--chart-grid)" />
        <XAxis
          dataKey={xKey}
          tickLine={false}
          axisLine={false}
          tickFormatter={(v: string | number) => fx(v)}
          interval={0}
          minTickGap={8}
        />
        <YAxis tickLine={false} axisLine={false} width={48} tickFormatter={(v: number) => fy(v)} />
        <ChartTooltip
          cursor={{ fill: "var(--muted)" }}
          content={<ChartTooltipContent formatter={(v) => fy(Number(v))} />}
        />
        {series.length > 1 ? <ChartLegend content={<ChartLegendContent />} /> : null}
        {series.slice(0, MAX_SERIES).map((s) => (
          <Bar
            key={s.key}
            dataKey={s.key}
            fill={`var(--color-${s.key})`}
            radius={[3, 3, 0, 0]}
            maxBarSize={48}
            {...(stacked ? { stackId: "a" } : {})}
            {...anim}
          />
        ))}
      </BarChart>
    </Frame>
  );
}

/**
 * Volume + trend on one axis: the first series draws as bars, every further
 * series as a line (the catalog's `combo` contract: same unit, first reads as
 * volume). One series renders bars only — there is no trend to draw.
 */
export function ComboChart({
  data,
  series,
  xKey,
  formatX,
  formatY,
  height,
  className,
  description,
  maxPoints = DEFAULT_MAX_POINTS,
}: Omit<SeriesChartProps, "stacked">) {
  const anim = useChartMotion();
  const rows = data.length > maxPoints ? data.slice(data.length - maxPoints) : data;
  const cfg = configFor(series);
  const fx = formatX ?? ((v: string | number) => String(v));
  const fy = formatY ?? ((v: number) => String(v));
  const [volume, ...trend] = series.slice(0, MAX_SERIES);
  return (
    <Frame
      description={description}
      className={className}
      height={height}
      config={cfg}
      shape="combo"
    >
      <ComposedChart
        data={[...rows]}
        accessibilityLayer
        margin={{ left: 8, right: 8, top: 8, bottom: 0 }}
      >
        <CartesianGrid vertical={false} stroke="var(--chart-grid)" />
        <XAxis
          dataKey={xKey}
          tickLine={false}
          axisLine={false}
          minTickGap={24}
          tickFormatter={(v: string | number) => fx(v)}
        />
        <YAxis tickLine={false} axisLine={false} width={48} tickFormatter={(v: number) => fy(v)} />
        <ChartTooltip
          cursor={{ fill: "var(--muted)" }}
          content={
            <ChartTooltipContent
              labelFormatter={(l) => (typeof l === "string" || typeof l === "number" ? fx(l) : "")}
              formatter={(v) => fy(Number(v))}
            />
          }
        />
        {series.length > 1 ? <ChartLegend content={<ChartLegendContent />} /> : null}
        {volume ? (
          <Bar
            key={volume.key}
            dataKey={volume.key}
            fill={`var(--color-${volume.key})`}
            radius={[3, 3, 0, 0]}
            maxBarSize={48}
            {...anim}
          />
        ) : null}
        {trend.map((s) => (
          <Line
            key={s.key}
            dataKey={s.key}
            type="monotone"
            stroke={`var(--color-${s.key})`}
            strokeWidth={2}
            dot={false}
            connectNulls={false}
            {...anim}
          />
        ))}
      </ComposedChart>
    </Frame>
  );
}
