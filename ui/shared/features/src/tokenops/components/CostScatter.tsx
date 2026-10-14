// Ported from nasiko-ui-lab@1b99bdda common/src/features/tokenops/components/CostScatter.tsx. Local edits: the visx
// drawing moved to shared/ui/src/charts-kit/cost-scatter.tsx (@visx/* and d3 are admitted only in the chart kit);
// this wrapper keeps the lab's component name and props, re-exports `ScatterPoint`, and hands the kit the two
// statistics it needs (`logTicks`, `median` from ../stats, which stay feature-side logic); Prettier formatting.
"use client";

/**
 * The cost × p95 scatter on raw visx (the chart kit has no scatter; docs/lab-vs-react-migration-review.md
 * §2.1). Log x (cost per operation), linear y (p95), bubble area ∝ operations, dashed medians split the
 * quadrants. Points pop in (at rest under reduced motion); hovering shows the point's numbers and a
 * click highlights its F3 row. Pointer-only: the Table view is the keyboard and screen-reader path.
 */
import { CostScatter as KitCostScatter, type ScatterPoint } from "@nasiko/ui/charts-kit";
import { logTicks, median } from "../stats";

export type { ScatterPoint };

export function CostScatter({
  points,
  onSelect,
}: {
  points: ScatterPoint[];
  onSelect: (id: string) => void;
}) {
  return <KitCostScatter points={points} onSelect={onSelect} logTicks={logTicks} median={median} />;
}
