/**
 * Lab addition (plans/feat-agent-centric.md eng, chart spike): the band layout of the stacked lanes (`lanes.tsx`):
 * bars, the crosshair and the pointer's bucket. Every bucket is an equal slot of the width, so the marker rail's flags
 * and the keyboard axis (percentages and an n-column grid) land on the same slots. Pure.
 */

/** Share of a bucket's slot left empty between bars. */
const LANE_BAR_PADDING = 0.25

export interface LaneBand {
  /** Width of one bucket's slot (bar plus padding). */
  step: number
  /** Width of a bar. */
  bar: number
  /** Left edge of bucket `i`'s slot. */
  slot: (i: number) => number
  /** Left edge of bucket `i`'s bar. */
  barX: (i: number) => number
  /** Centre of bucket `i`. */
  center: (i: number) => number
  /** The bucket under `x` (clamped), or -1 when there are no buckets. */
  indexAt: (x: number) => number
}

export function laneBand(count: number, width: number): LaneBand {
  const step = count > 0 && width > 0 ? width / count : 0
  const bar = Math.max(1, step * (1 - LANE_BAR_PADDING))
  const slot = (i: number) => i * step
  return {
    step,
    bar,
    slot,
    barX: (i) => slot(i) + (step - bar) / 2,
    center: (i) => slot(i) + step / 2,
    indexAt: (x) =>
      count > 0 && step > 0 ? Math.min(count - 1, Math.max(0, Math.floor(x / step))) : -1,
  }
}
