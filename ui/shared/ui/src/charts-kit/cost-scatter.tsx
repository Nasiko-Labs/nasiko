// Ported from nasiko-ui-lab@1b99bdda common/src/features/tokenops/components/CostScatter.tsx (the visx drawing).
// Local edits: lives in the chart kit because @visx/* and d3 are admitted only here (tooling/architecture); the two
// statistics the plot needs (`logTicks`, `median`, the lab's features/tokenops/stats.ts) come in as props from the
// feature-side wrapper (shared/features/src/tokenops/components/CostScatter.tsx) so the kit never imports a
// feature; `ScatterPoint` is declared here and re-exported by the wrapper; `motion`/`useReducedMotion` as in the
// lab (this directory may import the motion runtime); void arrow bodies braced (lint); the tip stays inside the plot
// (development 49950eb1e: measured, clamped to the plot's height); Prettier formatting.
// Destination: `useReducedMotion()` → `useKitReducedMotion()` (./use-kit-reduced-motion.ts), so the kit's at-rest
// path also follows the motion host's clamped mode (surface `none`/`reduced`, ADR-0006); the OS rule is unchanged.
"use client";

/**
 * The cost × p95 scatter on raw visx (the chart kit has no scatter; docs/lab-vs-react-migration-review.md
 * §2.1). Log x (cost per operation), linear y (p95), bubble area ∝ operations, dashed medians split the
 * quadrants. Points pop in (at rest under reduced motion); hovering shows the point's numbers and a
 * click highlights its F3 row. Pointer-only: the Table view is the keyboard and screen-reader path.
 */
import { AxisBottom, AxisLeft } from "@visx/axis";
import { GridColumns, GridRows } from "@visx/grid";
import { Group } from "@visx/group";
import { ParentSize } from "@visx/responsive";
import { scaleLinear, scaleLog, scaleSqrt } from "@visx/scale";
import * as m from "motion/react-m";
import { useState } from "react";
import { fmtCostPerOp, fmtHours, fmtInt, fmtLatency } from "../lib/format";
import { seriesAt } from "./lib/series";
import { useKitReducedMotion } from "./use-kit-reduced-motion";

export interface ScatterPoint {
  id: string;
  name: string;
  /** Cost per operation (USD), > 0. */
  x: number;
  /** p95 latency (ms). */
  y: number;
  /** Operations. */
  z: number;
  hours: number | null;
}

export interface CostScatterProps {
  points: ScatterPoint[];
  onSelect: (id: string) => void;
  /** Log-axis ticks inside `domain`, thinned for `width` px (the lab's stats.ts `logTicks`). */
  logTicks: (domain: [number, number], width: number) => number[];
  /** The median of `values` (the lab's stats.ts `median`). */
  median: (values: number[]) => number;
}

/** One series: its fill with the edge as the outline, so a pastel dot still reads on white. */
const POINT = seriesAt(1);
const MARGIN = { top: 8, right: 16, bottom: 44, left: 56 };
const TICK = { fill: "var(--muted-foreground)", fontSize: 11 };

export function CostScatter({ points, onSelect, logTicks, median }: CostScatterProps) {
  return (
    <ParentSize>
      {({ width, height }) =>
        width > 10 && height > 10 ? (
          <Plot
            points={points}
            onSelect={onSelect}
            width={width}
            height={height}
            logTicks={logTicks}
            median={median}
          />
        ) : null
      }
    </ParentSize>
  );
}

function Plot({
  points,
  onSelect,
  width,
  height,
  logTicks,
  median,
}: CostScatterProps & {
  width: number;
  height: number;
}) {
  const reduced = useKitReducedMotion();
  const [hover, setHover] = useState<ScatterPoint | null>(null);
  const w = width - MARGIN.left - MARGIN.right;
  const h = height - MARGIN.top - MARGIN.bottom;
  const xs = points.map((p) => p.x);
  const ys = points.map((p) => p.y);
  const zs = points.map((p) => p.z);
  const x = scaleLog({ domain: [Math.min(...xs) / 1.5, Math.max(...xs) * 1.5], range: [0, w] });
  const y = scaleLinear({ domain: [0, Math.max(...ys) * 1.1], range: [h, 0], nice: true });
  // Area ∝ operations: radius on a sqrt scale, 4-12 px.
  const r = scaleSqrt({ domain: [0, Math.max(...zs)], range: [4, 12] });
  const xTicks = logTicks(x.domain() as [number, number], w);
  const mx = x(median(xs));
  const my = y(median(ys));

  return (
    <div className="relative size-full">
      <svg width={width} height={height} aria-hidden>
        <Group left={MARGIN.left} top={MARGIN.top}>
          <GridRows scale={y} width={w} numTicks={4} stroke="var(--border)" />
          <GridColumns scale={x} height={h} tickValues={xTicks} stroke="var(--border)" />
          <line
            x1={mx}
            x2={mx}
            y1={0}
            y2={h}
            stroke="var(--muted-foreground)"
            strokeDasharray="4 4"
          />
          <line
            x1={0}
            x2={w}
            y1={my}
            y2={my}
            stroke="var(--muted-foreground)"
            strokeDasharray="4 4"
          />
          <AxisLeft
            scale={y}
            numTicks={4}
            tickFormat={(v) => fmtLatency(Number(v))}
            stroke="transparent"
            tickStroke="transparent"
            tickLabelProps={{ ...TICK, textAnchor: "end", dx: -4 }}
          />
          <AxisBottom
            top={h}
            scale={x}
            tickValues={xTicks}
            tickFormat={(v) => fmtCostPerOp(Number(v))}
            stroke="transparent"
            tickStroke="transparent"
            tickLabelProps={{ ...TICK, textAnchor: "middle" }}
            label="Cost per operation (log)"
            labelProps={{ ...TICK, textAnchor: "middle" }}
            labelOffset={8}
          />
          {points.map((p, i) => (
            <m.circle
              key={p.id}
              cx={x(p.x)}
              cy={y(p.y)}
              fill={POINT.fill}
              fillOpacity={hover && hover.id !== p.id ? 0.35 : 0.85}
              stroke={POINT.edge}
              strokeWidth={1.5}
              initial={reduced ? false : { r: 0 }}
              animate={{ r: r(p.z) }}
              transition={{
                type: "spring",
                stiffness: 260,
                damping: 20,
                delay: reduced ? 0 : i * 0.025,
              }}
              className="cursor-pointer"
              onPointerEnter={() => {
                setHover(p);
              }}
              onPointerLeave={() => {
                setHover((cur) => (cur?.id === p.id ? null : cur));
              }}
              onClick={() => {
                onSelect(p.id);
              }}
            />
          ))}
        </Group>
      </svg>
      {hover ? (
        <PointTip
          key={hover.id}
          p={hover}
          left={MARGIN.left + x(hover.x)}
          top={MARGIN.top + y(hover.y)}
          bottom={height}
          flip={x(hover.x) > w / 2}
        />
      ) : null}
    </div>
  );
}

/** The tip's top: centred on the point, kept inside the plot (development 49950eb1e). */
function tipTop(pointY: number, tipHeight: number, plotHeight: number) {
  return Math.max(0, Math.min(pointY - tipHeight / 2, plotHeight - tipHeight));
}

function PointTip({
  p,
  left,
  top,
  bottom,
  flip,
}: {
  p: ScatterPoint;
  left: number;
  top: number;
  bottom: number;
  flip: boolean;
}) {
  const [tall, setTall] = useState(0);
  return (
    <div
      ref={(el) => {
        if (el) setTall(el.offsetHeight);
      }}
      className="pointer-events-none absolute z-10 rounded-md border border-border bg-popover px-3 py-2 text-xs text-popover-foreground shadow-md"
      style={{
        left,
        top: tipTop(top, tall, bottom),
        transform: `translateX(${flip ? "calc(-100% - 12px)" : "12px"})`,
      }}
    >
      <div className="mb-1 font-medium">{p.name}</div>
      <div className="grid grid-cols-[auto_auto] gap-x-3 tabular-nums">
        <span className="text-muted-foreground">Cost/op</span>
        <span>{fmtCostPerOp(p.x)}</span>
        <span className="text-muted-foreground">p95</span>
        <span>{fmtLatency(p.y)}</span>
        <span className="text-muted-foreground">Operations</span>
        <span>{fmtInt(p.z)}</span>
        <span className="text-muted-foreground">Container</span>
        <span>{fmtHours(p.hours)}</span>
      </div>
    </div>
  );
}
