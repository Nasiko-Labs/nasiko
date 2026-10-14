"use client";

import { cn } from "../lib/cn";
import { Pie, PieChart } from "recharts";
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
 * Parts of a whole. A donut is only readable with few slices, so the
 * foundation caps it at `DONUT_MAX_SLICES`; the CALLER decides what to do
 * above the cap (group into "other", or refuse) — this component never drops
 * a slice silently: passing more than the cap throws in development and
 * renders nothing in production, so a caller cannot rely on silent truncation.
 *
 * Accessibility: the figure is `role="img"` with the caller's description,
 * and a visually hidden list carries every slice's label, value and share —
 * a screen reader gets the numbers, not a picture.
 */
export const DONUT_MAX_SLICES = 6;

export interface DonutSlice {
  readonly label: string;
  readonly value: number;
}

export interface DonutChartProps {
  readonly slices: readonly DonutSlice[];
  readonly description: string;
  readonly formatValue?: (v: number) => string;
  readonly height?: number;
  readonly className?: string;
}

export function DonutChart({
  slices,
  description,
  formatValue,
  height = 240,
  className,
}: DonutChartProps) {
  const { mode, enter } = useMotion();
  const anim = { isAnimationActive: mode === "full" && enter, animationDuration: 220 };
  const fv = formatValue ?? ((v: number) => String(v));
  if (slices.length > DONUT_MAX_SLICES) {
    if (process.env.NODE_ENV !== "production")
      throw new Error(`DonutChart: ${slices.length} slices exceed ${DONUT_MAX_SLICES}`);
    return null;
  }
  const total = slices.reduce((a, s) => a + (s.value > 0 ? s.value : 0), 0);
  const data = slices.map((s, i) => ({
    key: `s${i}`,
    label: s.label,
    value: s.value,
    fill: `var(--color-s${i})`,
  }));
  const config: ChartConfig = Object.fromEntries(
    data.map((d, i) => [d.key, { label: d.label, color: `var(--chart-${(i % 5) + 1})` }]),
  );
  return (
    <div className={cn("w-full", className)} data-chart-shape="donut">
      <ChartContainer
        config={config}
        className="aspect-auto w-full"
        style={{ height }}
        role="img"
        aria-label={description}
        initialDimension={{ width: 600, height }}
      >
        <PieChart accessibilityLayer margin={{ left: 8, right: 8, top: 8, bottom: 0 }}>
          <ChartTooltip content={<ChartTooltipContent formatter={(v) => fv(Number(v))} />} />
          <ChartLegend content={<ChartLegendContent nameKey="key" />} />
          <Pie
            data={data}
            dataKey="value"
            nameKey="key"
            innerRadius="55%"
            outerRadius="85%"
            paddingAngle={1}
            strokeWidth={0}
            {...anim}
          />
        </PieChart>
      </ChartContainer>
      <ul className="sr-only" data-donut-values="">
        {data.map((d) => (
          <li key={d.key}>
            {d.label}: {fv(d.value)}
            {total > 0 ? ` (${Math.round((Math.max(d.value, 0) / total) * 100)}%)` : ""}
          </li>
        ))}
      </ul>
    </div>
  );
}
