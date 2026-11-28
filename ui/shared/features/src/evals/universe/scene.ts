/** Pure layout/color/size math for the Eval Universe scene — kept out of the page component file
 * so fast refresh stays happy (react-refresh/only-export-components: a page file must only export
 * components). See `EvalUniversePage.tsx` for where these are used, and the tests alongside this
 * file for what each one guarantees.
 *
 * Layout: one flat shared recall (X) / precision (Y) plane — every attempt from every task plots
 * on that same 0..1 scale, no Z depth. An earlier version separated tasks into Z lanes, but with
 * usually only 1-2 tasks that third dimension added nothing readable; which task an attempt
 * belongs to is carried by its hover tooltip and the sidebar's task filter instead. */
import type { UniverseAttempt } from './types'

export const AXIS_SPAN = 3.4

const UNGRADED_XY: [number, number] = [-0.5, -0.5]

/**
 * An attempt's position on the shared recall/precision plane. Ungraded, pending, or failed
 * attempts have no real score to plot — they sit in a fixed spot just outside the 0..1 quadrant,
 * never at a fabricated (0, 0) that would read as a real zero score.
 */
export function pointXY(a: UniverseAttempt): [number, number] {
  if (a.status !== 'done' || a.metrics == null) return UNGRADED_XY
  return [a.metrics.recall * AXIS_SPAN, a.metrics.precision * AXIS_SPAN]
}

const PALETTE = [
  '#38bdf8',
  '#f472b6',
  '#facc15',
  '#34d399',
  '#a78bfa',
  '#fb923c',
  '#2dd4bf',
  '#f87171',
]

/**
 * One color per distinct model key, assigned by sorted position in the palette — not a hash.
 * A hash of short, similar-looking model ids into 8 buckets collides often (several of our own
 * real model ids landed on the same color in testing); indexing into a sorted, deduplicated list
 * of the models actually present guarantees distinct colors as long as there are no more distinct
 * models than palette entries, which is the common case here. Beyond that, colors repeat —
 * an honest, visible limitation rather than a silent one.
 */
export function buildModelPalette(modelKeys: Iterable<string>): Map<string, string> {
  const sorted = Array.from(new Set(modelKeys)).sort()
  return new Map(sorted.map((key, i) => [key, PALETTE[i % PALETTE.length]]))
}

const GRAY = '#9ca3af'
const FALLBACK = '#64748b'

/** Gray for anything not scored yet (ungraded, pending, or failed) — never a fabricated color;
 * otherwise colored by model identity (via `palette`, built from the models actually in view),
 * since position now carries precision/recall. A model key missing from `palette` (shouldn't
 * happen in practice — the caller builds it from the same attempts) falls back to neutral slate
 * rather than crashing or silently reusing another model's color. */
export function pointColor(a: UniverseAttempt, palette: Map<string, string>): string {
  if (a.status !== 'done' || a.metrics == null) return GRAY
  return palette.get(a.model ?? a.model_name) ?? FALLBACK
}

/**
 * A small ring offset for points that land on the exact same recall/precision score (common at
 * the (0, 0) and (1, 1) corners) or the same ungraded/failed spot — without this, several attempts
 * can sit fully on top of each other on the flat shared plane and look like a single point.
 */
export function overlapOffset(index: number, total: number): [number, number] {
  if (total <= 1) return [0, 0]
  const radius = 0.16
  const angle = (2 * Math.PI * index) / total
  return [radius * Math.cos(angle), radius * Math.sin(angle)]
}

const MIN_RADIUS = 0.14
const MAX_RADIUS = 0.4

/**
 * Log-scaled by total tokens, not cost — several models here are free or near-zero cost, which
 * would make most points visually indistinguishable if size encoded cost instead. Floors at
 * `MIN_RADIUS` so an attempt with 0 recorded tokens (a known display bug, not this page's doing —
 * see `store::attempt_usage`'s doc comment) still renders as a visible point, not a vanished one.
 */
export function pointRadius(a: UniverseAttempt): number {
  const tokens = (a.input_tokens ?? 0) + (a.output_tokens ?? 0)
  const scaled = Math.log1p(tokens) / Math.log1p(200_000)
  return MIN_RADIUS + Math.min(1, Math.max(0, scaled)) * (MAX_RADIUS - MIN_RADIUS)
}
