/**
 * A chart bucket picked into the URL (`?slice=<ISO start>/<ISO end>`, end exclusive): Optimization's bars and the agent
 * hub's buckets use the same format, so the rules live here (plans/feat-optimization-page.md C5; plans/feat-agent-centric.md
 * DX URL keys). Also the one "tokens saved" rule both pages draw. Pure.
 */
const HOUR = 3_600_000
const DAY = 24 * HOUR

export type BucketSize = 'hour' | 'day' | 'week'

const SIZE_MS: Record<BucketSize, number> = { hour: HOUR, day: DAY, week: 7 * DAY }

/** A bar's interval as the `?slice=` value: ISO 8601 `start/end` (end exclusive). */
export function sliceOf(startIso: string, size: BucketSize): string {
  const start = Date.parse(startIso)
  return `${new Date(start).toISOString()}/${new Date(start + SIZE_MS[size]).toISOString()}`
}

export interface Slice {
  from: string
  to: string
  size: BucketSize
}

/**
 * `?slice=` back to an interval, only when it is one bucket (an hour, a day or a week) inside the window; anything
 * else (hand-edited, or left from another window) is ignored, so a stale filter never narrows the list.
 */
export function parseSlice(
  value: string | undefined,
  win: { start: Date; end: Date },
): Slice | null {
  if (!value) return null
  const [a, b, extra] = value.split('/')
  if (!a || !b || extra !== undefined) return null
  const from = Date.parse(a)
  const to = Date.parse(b)
  if (Number.isNaN(from) || Number.isNaN(to)) return null
  const size = (Object.keys(SIZE_MS) as BucketSize[]).find((k) => SIZE_MS[k] === to - from)
  if (!size) return null
  if (to <= win.start.getTime() || from >= win.end.getTime()) return null
  return { from: new Date(from).toISOString(), to: new Date(to).toISOString(), size }
}

/** Tokens saved: without minus sent, never negative (a report can't send more than its pool). */
export const savedOf = (r: { pool_tokens: number; sent_tokens: number }) =>
  Math.max(0, r.pool_tokens - r.sent_tokens)
