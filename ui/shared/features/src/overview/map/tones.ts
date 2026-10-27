/**
 * The map's colour language as theme-token classes (DESIGN.md "Colour tokens"): a problem's kind is the only thing
 * colour says — errors `--destructive`, slow `--warning`, cost `--cost`; healthy is `--success`; idle and ghosts are
 * `--muted`. "Needs your input" is a `--info` outline only. Literal class names, so Tailwind sees them.
 */
import type { DayState, NodeState } from '../estate/model'
import type { ProblemKind } from '../estate/types'

/** A node's square. */
export const TINT: Record<NodeState, string> = {
  errors: 'fill-destructive/18',
  slow: 'fill-warning/18',
  cost: 'fill-cost/18',
  ok: 'fill-success/12',
  idle: 'fill-muted',
  ghost: 'fill-muted/50',
  muted: 'fill-muted/70',
}
/** The glyph in a node's square. */
export const INK: Record<NodeState, string> = {
  errors: 'text-destructive',
  slow: 'text-warning',
  cost: 'text-cost',
  ok: 'text-success',
  idle: 'text-muted-foreground/60',
  ghost: 'text-muted-foreground/30',
  muted: 'text-muted-foreground/50',
}
/** A day or session tile. */
export const TILE: Record<DayState, string> = {
  errors: 'fill-destructive/65',
  slow: 'fill-warning/55',
  cost: 'fill-cost/50',
  ok: 'fill-success/30',
  idle: 'fill-muted',
  ghost: 'fill-muted/40',
  muted: 'fill-success/15',
  future: 'fill-muted/50',
}
/** Strong marks: count badges, chips, a problem card's rule. */
export const STRONG_FILL: Record<ProblemKind, string> = {
  errors: 'fill-destructive',
  slow: 'fill-warning',
  cost: 'fill-cost',
}
export const STRONG_STROKE: Record<ProblemKind, string> = {
  errors: 'stroke-destructive',
  slow: 'stroke-warning',
  cost: 'stroke-cost',
}
export const STRONG_BG: Record<ProblemKind, string> = {
  errors: 'bg-destructive',
  slow: 'bg-warning',
  cost: 'bg-cost',
}
export const STRONG_TEXT: Record<ProblemKind, string> = {
  errors: 'text-destructive',
  slow: 'text-warning',
  cost: 'text-cost',
}
export const STRONG_BORDER: Record<ProblemKind, string> = {
  errors: 'border-l-destructive',
  slow: 'border-l-warning',
  cost: 'border-l-cost',
}
/** Swatches in the legend and chips (HTML). */
export const SWATCH: Record<NodeState, string> = {
  errors: 'bg-destructive/60',
  slow: 'bg-warning/55',
  cost: 'bg-cost/50',
  ok: 'bg-success/30',
  idle: 'bg-muted',
  ghost: 'bg-muted/40',
  muted: 'bg-muted',
}
/** A day tile drawn in HTML (the Agent status card), the same tones as `TILE`. */
export const TILE_BG: Record<DayState, string> = {
  errors: 'bg-destructive/65',
  slow: 'bg-warning/55',
  cost: 'bg-cost/50',
  ok: 'bg-success/30',
  idle: 'bg-muted',
  ghost: 'bg-muted/40',
  muted: 'bg-success/15',
  future: 'bg-muted/50',
}
