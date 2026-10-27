/**
 * The Flows list's pure rules (plans/feat-flows.md §3, F8, F11, F16): the window and kind filters the page applies to
 * the rows it read (the server filters only status and search), requests per bucket by status, and duration
 * percentiles. Until FL-6 these describe "your latest N flows", the rows read back to the window start (A2).
 */
import { flowKind } from './kind'
import type { KindFilter } from './search'
import type { Flow } from './types'

const HOUR = 3_600_000
const DAY = 24 * HOUR
/** p99 means little under this many finished flows (F16). */
export const P99_MIN = 100

export const STATUSES = ['completed', 'failed', 'paused', 'running'] as const
export type ListStatus = (typeof STATUSES)[number]

const at = (iso: string | null | undefined) => (iso ? Date.parse(iso) : NaN)

/** Rows started inside [start, end), of the chosen kind. */
export function filterRows(
  rows: readonly Flow[],
  start: number,
  end: number,
  kind: KindFilter,
): Flow[] {
  return rows.filter((f) => {
    const t = at(f.created_at)
    return t >= start && t < end && (kind === 'all' || flowKind(f) === kind)
  })
}

/** A finished flow's duration from its exact timestamps; `duration_ms` (whole seconds, FL-4) only as a fallback. */
export function durationOf(f: Flow): number | null {
  if (f.status !== 'completed' && f.status !== 'failed') return null
  const d = at(f.completed_at) - at(f.created_at)
  return Number.isFinite(d) ? Math.max(0, d) : (f.duration_ms ?? null)
}

export interface Bucket extends Record<ListStatus, number> {
  iso: string
  label: string
  total: number
}

/** At most this many chart buckets: a long custom window groups days, never allocates one bucket per day of it. */
export const MAX_BUCKETS = 120

const DAY_LABEL = new Intl.DateTimeFormat('en-US', {
  month: 'short',
  day: 'numeric',
  timeZone: 'UTC',
})
const YEAR_LABEL = new Intl.DateTimeFormat('en-US', {
  month: 'short',
  day: 'numeric',
  year: 'numeric',
  timeZone: 'UTC',
})
const HOUR_LABEL = new Intl.DateTimeFormat('en-US', {
  month: 'short',
  day: 'numeric',
  hour: 'numeric',
  timeZone: 'UTC',
})

/**
 * Requests per UTC day (per hour on a 24-hour window), stacked by the server's status, every bucket present. A window
 * longer than MAX_BUCKETS days groups several days per bucket. Labels are unique (they are the chart's categories):
 * hours carry their day, and days carry their year when the window spans more than one.
 */
export function buckets(
  rows: readonly Flow[],
  start: number,
  end: number,
  hourly: boolean,
): Bucket[] {
  const span = Math.max(0, end - start)
  const size = hourly ? HOUR : DAY * Math.max(1, Math.ceil(span / DAY / MAX_BUCKETS))
  const first = Math.floor(start / size) * size
  const years = new Date(first).getUTCFullYear() !== new Date(end).getUTCFullYear()
  const label = hourly ? HOUR_LABEL : years ? YEAR_LABEL : DAY_LABEL
  const out: Bucket[] = []
  for (let t = first; t < end && out.length < MAX_BUCKETS + 2; t += size) {
    const d = new Date(t)
    out.push({
      iso: d.toISOString(),
      label: label.format(d),
      completed: 0,
      failed: 0,
      paused: 0,
      running: 0,
      total: 0,
    })
  }
  for (const f of rows) {
    const b = out[Math.floor((at(f.created_at) - first) / size)]
    const s = (STATUSES as readonly string[]).includes(f.status) ? (f.status as ListStatus) : null
    if (!b || !s) continue
    b[s] += 1
    b.total += 1
  }
  return out
}

export interface Percentiles {
  p50: number
  p90: number
  /** Null under P99_MIN finished flows. */
  p99: number | null
  count: number
}

/** Nearest-rank percentiles over finished flows (F16: running and paused ones have no duration yet). */
export function percentiles(rows: readonly Flow[]): Percentiles | null {
  const ds = rows.flatMap((f) => {
    const d = durationOf(f)
    return d === null ? [] : [d]
  })
  if (!ds.length) return null
  ds.sort((a, b) => a - b)
  const rank = (p: number) => ds[Math.min(ds.length - 1, Math.ceil(p * ds.length) - 1)] ?? 0
  return {
    p50: rank(0.5),
    p90: rank(0.9),
    p99: ds.length >= P99_MIN ? rank(0.99) : null,
    count: ds.length,
  }
}
