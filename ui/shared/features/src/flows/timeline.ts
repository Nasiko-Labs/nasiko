/**
 * Swimlane geometry (plans/feat-flows.md F2, F3, F13, F23, F28), pure so the component only draws.
 *
 * - The axis runs from the flow's start to its end, or to "now" while it moves; while it works (running, finishing)
 *   the drawn work gets 20% headroom so the axis doesn't rescale on every poll (F13). A paused flow gets none: nothing
 *   new can appear until someone answers.
 * - A recorded wait (`awaiting_human`) longer than 25% of the flow collapses into a fixed BREAK_PX break; the bars on
 *   either side keep their scale (F28). Positions are CSS: `calc(frac * (100% - BREAK_PX) + px)`.
 * - Fan-out brackets come from known parent links plus overlap: two or more calls with the same parent that run at the
 *   same time (F3, F23).
 */
import { waitFrom, type Call } from './calls'

export const BREAK_PX = 48
const HEADROOM = 1.2
const WAIT_SHARE = 0.25
/** Ticks this close (a share of the drawn axis) before a break would sit under its label. */
const GAP_CLEAR = 0.15
const MAX_TICKS = 40
const STEPS_MS = [
  100, 200, 500, 1_000, 2_000, 5_000, 10_000, 15_000, 30_000, 60_000, 120_000, 300_000, 600_000,
  1_800_000, 3_600_000,
]

interface Gap {
  fromMs: number
  toMs: number
}

export interface Axis {
  startMs: number
  endMs: number
  gap: Gap | null
}

/** A position on the track: `frac` of the stretchable width plus `px` fixed pixels (the break). */
export interface Pos {
  frac: number
  px: number
}

export function buildAxis(
  calls: readonly Call[],
  flowStartMs: number,
  flowEndMs: number | null,
  now: number,
  working = flowEndMs === null,
): Axis {
  const start = Math.min(flowStartMs, ...calls.map((c) => c.startMs))
  const open = flowEndMs === null
  const lastEnd = Math.max(
    flowEndMs ?? now,
    ...calls.map((c) => c.endMs ?? (open ? now : c.startMs)),
  )
  const span = Math.max(1, lastEnd - start)
  // The longest recorded wait, if it's long enough to squash everything else.
  const wait = calls
    .filter((c) => c.status === 'awaiting_human' || (c.status === 'resumed' && c.endMs !== null))
    .map((c) => ({ fromMs: waitFrom(c), toMs: c.endMs ?? now }))
    .sort((a, b) => b.toMs - b.fromMs - (a.toMs - a.fromMs))[0]
  const gap = wait && wait.toMs - wait.fromMs > span * WAIT_SHARE ? wait : null
  // Headroom on the drawn work only, never on a collapsed wait.
  const drawn = span - (gap ? gap.toMs - gap.fromMs : 0)
  const end = working ? lastEnd + drawn * (HEADROOM - 1) : lastEnd
  return { startMs: start, endMs: Math.max(end, start + 1), gap }
}

/** Where `t` sits on the track. With a gap, the stretchable part excludes the gap and the break adds BREAK_PX. */
export function position(axis: Axis, t: number): Pos {
  const { startMs, endMs, gap } = axis
  if (!gap) return { frac: clamp((t - startMs) / (endMs - startMs)), px: 0 }
  const gapLen = gap.toMs - gap.fromMs
  const stretch = Math.max(1, endMs - startMs - gapLen)
  if (t <= gap.fromMs) return { frac: clamp((t - startMs) / stretch), px: 0 }
  if (t >= gap.toMs) return { frac: clamp((t - startMs - gapLen) / stretch), px: BREAK_PX }
  return {
    frac: clamp((gap.fromMs - startMs) / stretch),
    px: ((t - gap.fromMs) / gapLen) * BREAK_PX,
  }
}

const clamp = (v: number) => Math.min(1, Math.max(0, v))

/** CSS for a position with a gap: the stretchable width is 100% minus the break. */
export function cssAt(axis: Axis, p: Pos): string {
  if (!axis.gap) return `${(p.frac * 100).toFixed(4)}%`
  return `calc((100% - ${BREAK_PX}px) * ${p.frac.toFixed(6)} + ${p.px.toFixed(2)}px)`
}

/** Left and width of an interval, as CSS lengths. */
export function span(axis: Axis, fromMs: number, toMs: number): { left: string; width: string } {
  const a = position(axis, fromMs)
  const b = position(axis, Math.max(fromMs, toMs))
  if (!axis.gap) {
    return {
      left: `${(a.frac * 100).toFixed(4)}%`,
      width: `${((b.frac - a.frac) * 100).toFixed(4)}%`,
    }
  }
  return {
    left: cssAt(axis, a),
    width: `calc((100% - ${BREAK_PX}px) * ${(b.frac - a.frac).toFixed(6)} + ${(b.px - a.px).toFixed(2)}px)`,
  }
}

export interface Tick {
  ms: number
  /** Offset from the axis start. */
  offsetMs: number
  left: string
}

/** About `target` ticks on round offsets from the start, none inside the gap or crowding its label. */
export function ticks(axis: Axis, target = 5): Tick[] {
  const visible = axis.endMs - axis.startMs - (axis.gap ? axis.gap.toMs - axis.gap.fromMs : 0)
  const step = STEPS_MS.find((s) => visible / s <= target) ?? STEPS_MS[STEPS_MS.length - 1] ?? 1
  const out: Tick[] = []
  for (let off = 0; axis.startMs + off <= axis.endMs && out.length <= MAX_TICKS; off += step) {
    const t = axis.startMs + off
    if (axis.gap && t > axis.gap.fromMs - visible * GAP_CLEAR && t < axis.gap.toMs) {
      // Jump the collapsed wait in one step: walking a days-long wait tick by tick would stall every render.
      off = Math.ceil((axis.gap.toMs - axis.startMs) / step) * step - step
      continue
    }
    out.push({ ms: t, offsetMs: off, left: cssAt(axis, position(axis, t)) })
  }
  return out
}

export interface FanOut {
  parentKey: string
  fromMs: number
  toMs: number
  keys: string[]
  /** The most calls running at the same moment: a chain of overlaps can hold more calls than ever ran at once. */
  peak: number
}

/** The most intervals open at one moment (an end at the same time as a start doesn't overlap it). */
function peakOf(calls: readonly Call[], now: number): number {
  const edges = calls.flatMap((c) => [
    { t: c.startMs, d: 1 },
    { t: c.endMs ?? now, d: -1 },
  ])
  edges.sort((a, b) => a.t - b.t || a.d - b.d)
  let open = 0
  let peak = 0
  for (const e of edges) peak = Math.max(peak, (open += e.d))
  // A group is 2+ overlapping calls; a zero-length call's edges can cancel out, so never report fewer than that.
  return Math.max(2, peak)
}

/** Groups of 2+ calls with the same known parent whose run times overlap (F3, F23). */
export function fanOuts(calls: readonly Call[], now: number): FanOut[] {
  const byParent = new Map<string, Call[]>()
  for (const c of calls) {
    if (!c.parentKey) continue
    byParent.set(c.parentKey, [...(byParent.get(c.parentKey) ?? []), c])
  }
  const out: FanOut[] = []
  for (const [parentKey, kids] of byParent) {
    const sorted = [...kids].sort((a, b) => a.startMs - b.startMs)
    let group: Call[] = []
    let groupEnd = -Infinity
    const flush = () => {
      if (group.length >= 2)
        out.push({
          parentKey,
          fromMs: group[0]?.startMs ?? 0,
          toMs: groupEnd,
          keys: group.map((c) => c.key),
          peak: peakOf(group, now),
        })
    }
    for (const c of sorted) {
      const end = c.endMs ?? now
      if (c.startMs < groupEnd) {
        group.push(c)
        groupEnd = Math.max(groupEnd, end)
      } else {
        flush()
        group = [c]
        groupEnd = end
      }
    }
    flush()
  }
  return out.sort((a, b) => a.fromMs - b.fromMs)
}

/** A connector between two drawn bars, by their vertical centres in the chart (px). */
export interface Edge {
  from: { y: number; call: Call }
  to: { y: number; call: Call }
  onPath: boolean
}

/**
 * One soft connector. A call made after its caller ended (a hand-off) is an S-curve from the caller's end to the
 * call's start; a call made while its caller still runs drops from the caller's edge (along the caller's trunk) and
 * curves into the call's start.
 */
export function curve(
  e: Edge,
  xAt: (t: number) => number,
  now: number,
  barPx = 20,
  /** The caller's shared trunk, so several calls made at once branch off one line instead of each drawing its own. */
  trunkX?: number,
): string {
  const aEnd = xAt(e.from.call.endMs ?? now)
  const bx = xAt(e.to.call.startMs)
  const { y: ay } = e.from
  const { y: by } = e.to
  if (aEnd <= bx + 1) {
    const dx = Math.max(10, (bx - aEnd) / 2)
    return `M ${aEnd.toFixed(1)} ${ay} C ${(aEnd + dx).toFixed(1)} ${ay}, ${(bx - dx).toFixed(1)} ${by}, ${bx.toFixed(1)} ${by}`
  }
  const down = by > ay
  const x = trunkX ?? Math.max(xAt(e.from.call.startMs) + 4, bx - 8)
  const y0 = ay + (down ? 1 : -1) * (barPx / 2)
  return `M ${x.toFixed(1)} ${y0} L ${x.toFixed(1)} ${(by + (down ? -6 : 6)).toFixed(1)} Q ${x.toFixed(1)} ${by}, ${bx.toFixed(1)} ${by}`
}
