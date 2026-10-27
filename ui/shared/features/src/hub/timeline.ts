/**
 * The hub's time axis (plans/feat-agent-centric.md PR1 lanes; design "Lanes"; eng "Sparse buckets", "Markers"). Pure.
 *
 * - Buckets are the server's (`bucket` of the finops timeseries: hourly only for 24h), UTC, zero-filled the way
 *   TokenOps fills them (`tokenops/series.ts`): today's timeseries has no coverage field, so an omitted spend or
 *   requests bucket is 0, and a missing p95 is a gap (null), never 0.
 * - Buckets that end before the agent was created are `before` (hatched, "Not deployed yet").
 * - The previous window (compare on, daily buckets only) lines up by position: bucket i of the previous window under
 *   bucket i of this one.
 * - A marker belongs to the bucket its time falls in (`start <= at < end`); one on a bucket edge belongs to the bucket
 *   it starts. Markers in the same bucket merge into one flag.
 */
import { zeroFillTimeline } from '@nasiko/features/tokenops/series'
import type { FinopsSpendTimeseries } from '@nasiko/features/tokenops/types'
import { fmtShortDay } from '@nasiko/ui/lib/format'
import { parseSlice, savedOf, sliceOf as sliceValue, type BucketSize } from '@nasiko/app-core/slice'
import type { Marker } from './events'

const HOUR = 3_600_000
const DAY = 24 * HOUR

export type Bucket = 'hour' | 'day'

export interface HubRow {
  iso: string
  label: string
  start: number
  end: number
  /** An hour, a day, or a week of days (the phone chart, `weeklyRows`). */
  size: BucketSize
  /** The day a week's links open: its newest day (Sessions and TokenOps work on one UTC day). */
  linkDay?: number
  spend: number
  requests: number
  p95: number | null
  /** Tokens saved in the bucket: only from the proposed per-agent series (mock mode); null otherwise. */
  saved: number | null
  /** The previous window's bucket at the same position (compare on, daily only); null otherwise. */
  prevSpend: number | null
  prevRequests: number | null
  /** The bucket ended before the agent existed. */
  before: boolean
}

export interface SavedPoint {
  bucket_start: string
  pool_tokens: number
  sent_tokens: number
}

const stepOf = (bucket: Bucket) => (bucket === 'hour' ? HOUR : DAY)

export function rowsFor(input: {
  series: FinopsSpendTimeseries
  win: { start: Date; end: Date; prevStart: Date; prevEnd: Date }
  createdAt: string
  previous?: FinopsSpendTimeseries | null
  saved?: readonly SavedPoint[] | null
}): HubRow[] {
  const bucket = input.series.bucket
  const step = stepOf(bucket)
  const current = zeroFillTimeline(input.series.points, bucket, input.win.start, input.win.end)
  // The previous window is daily on the server even for 24h (window.ts prevParams): no ghost there (eng "Compare").
  const prev =
    input.previous && bucket === 'day' && input.previous.bucket === 'day'
      ? zeroFillTimeline(input.previous.points, 'day', input.win.prevStart, input.win.prevEnd)
      : null
  const created = Date.parse(input.createdAt)
  const savedBy = new Map<number, number>()
  for (const p of input.saved ?? []) {
    const t = Date.parse(p.bucket_start)
    savedBy.set(t, (savedBy.get(t) ?? 0) + savedOf(p))
  }
  return current.map((p, i) => {
    const start = Date.parse(p.iso)
    const end = start + step
    const before = !Number.isNaN(created) && end <= created
    const old = prev?.[i]
    return {
      iso: p.iso,
      label: p.label,
      start,
      end,
      size: bucket,
      spend: p.spend,
      requests: p.operations,
      p95: p.p95,
      saved: input.saved ? (savedBy.get(start) ?? 0) : null,
      prevSpend: old ? old.spend : null,
      prevRequests: old ? old.operations : null,
      before,
    }
  })
}

/** Rows before any data has loaded: the window's empty buckets, so the axis and the marker rail can draw. */
export function emptyRows(win: { start: Date; end: Date }, bucket: Bucket, createdAt: string) {
  return rowsFor({
    series: { bucket, points: [] },
    win: { ...win, prevStart: win.start, prevEnd: win.start },
    createdAt,
  })
}

/**
 * Below 640 px the chart draws weeks (design "Responsive", Optimization's `weeklyPoints` rule): counted back from the
 * newest day, so the last bar is the latest 7 days and each week starts 6 days before its newest day. Spend, requests
 * and savings sum; p95 is the week's peak daily p95; a week is hatched only when every day in it is. Daily rows only
 * (24h stays hourly); the Table view keeps the days.
 */
export function weeklyRows(rows: readonly HubRow[]): HubRow[] {
  const last = rows.at(-1)
  if (!last || last.size !== 'day') return [...rows]
  const weeks = new Map<number, HubRow[]>()
  for (const r of rows) {
    const back = Math.floor((last.start - r.start) / (7 * DAY))
    weeks.set(back, [...(weeks.get(back) ?? []), r])
  }
  const sum = (xs: (number | null)[]) =>
    xs.some((x) => x !== null) ? xs.reduce<number>((a, x) => a + (x ?? 0), 0) : null
  return [...weeks.entries()]
    .sort((a, b) => b[0] - a[0])
    .map(([back, days]) => {
      const start = last.start - (back * 7 + 6) * DAY
      const end = start + 7 * DAY
      const live = days.filter((d) => !d.before)
      const p95s = live.map((d) => d.p95).filter((v): v is number => v !== null)
      return {
        iso: new Date(start).toISOString(),
        label: `${fmtShortDay(new Date(start))} – ${fmtShortDay(new Date(end - DAY))}`,
        start,
        end,
        size: 'week' as const,
        linkDay: Math.max(...days.map((d) => d.start)),
        spend: days.reduce((a, d) => a + d.spend, 0),
        requests: days.reduce((a, d) => a + d.requests, 0),
        p95: p95s.length ? Math.max(...p95s) : null,
        saved: sum(days.map((d) => d.saved)),
        prevSpend: sum(days.map((d) => d.prevSpend)),
        prevRequests: sum(days.map((d) => d.prevRequests)),
        before: live.length === 0,
      }
    })
}

/** The bucket size the server uses for this window (only `range=24h` is hourly). */
export const bucketForPreset = (preset: string): Bucket => (preset === '24h' ? 'hour' : 'day')

export interface LaneTotals {
  /** The highest daily (or hourly) p95 in the window, never a derived window p95 (design); buckets before the agent
   *  existed don't count. The window's spend and requests come from the dashboard, as the sentence's do. */
  peakP95: number | null
}

export function totalsOf(rows: readonly HubRow[]): LaneTotals {
  let peak: number | null = null
  for (const r of rows) if (!r.before && r.p95 !== null) peak = Math.max(peak ?? 0, r.p95)
  return { peakP95: peak }
}

/** The bucket a time falls in (`start <= at < end`), or -1 outside the rows. */
export function bucketIndexOf(at: number, rows: readonly HubRow[]): number {
  let lo = 0
  let hi = rows.length - 1
  while (lo <= hi) {
    const mid = (lo + hi) >> 1
    const r = rows[mid]
    if (!r) return -1
    if (at < r.start) hi = mid - 1
    else if (at >= r.end) lo = mid + 1
    else return mid
  }
  return -1
}

export interface Flag {
  /** The bucket the flag's markers fall in. */
  index: number
  /** Oldest first. */
  markers: Marker[]
  /** Where the oldest marker sits inside its bucket, 0–1 (the flag is drawn at its exact time). */
  offset: number
  alert: boolean
}

/** Markers merged per bucket (eng: merge key = bucket index); markers outside the rows are not drawn. */
export function flagsFor(markers: readonly Marker[], rows: readonly HubRow[]): Flag[] {
  const by = new Map<number, Marker[]>()
  for (const m of markers) {
    const i = bucketIndexOf(m.at, rows)
    if (i < 0) continue
    by.set(i, [...(by.get(i) ?? []), m])
  }
  return [...by.entries()]
    .sort((a, b) => a[0] - b[0])
    .flatMap(([index, ms]) => {
      const r = rows[index]
      const first = ms[0]
      if (!r || !first) return []
      return [
        {
          index,
          markers: ms,
          offset: Math.min(1, Math.max(0, (first.at - r.start) / (r.end - r.start))),
          alert: ms.some((m) => m.alert),
        },
      ]
    })
}

// ─── bucket selection (`?slice=`, lib/slice.ts: the same format as Optimization's) ─────────

/** A bucket's interval as the `?slice=` value. */
export const sliceOf = (r: Pick<HubRow, 'iso' | 'size'>) => sliceValue(r.iso, r.size)

/**
 * `?slice=` back to a row, only when it names exactly one of these buckets (same start and size) inside the window;
 * anything else (hand-edited, or left from another window or bucket size) is -1, and the page drops it with replace
 * (eng "URL interactions").
 */
export function rowOfSlice(
  value: string | undefined,
  rows: readonly HubRow[],
  win: { start: Date; end: Date },
): number {
  const s = parseSlice(value, win)
  if (!s) return -1
  const from = Date.parse(s.from)
  return rows.findIndex((r) => r.start === from && r.size === s.size)
}

/** The bucket's UTC day (`YYYY-MM-DD`), for TokenOps' day panel and Sessions' day (hours: their day; weeks: the newest). */
export const dayOf = (r: Pick<HubRow, 'start' | 'linkDay'>) =>
  new Date(r.linkDay ?? r.start).toISOString().slice(0, 10)

/** Sessions search covers the last `days` days: a bucket older than that links to TokenOps' day panel instead. */
export function withinSessions(
  r: Pick<HubRow, 'start' | 'linkDay'>,
  now: Date,
  days: number,
): boolean {
  const dayStart = Date.parse(`${dayOf(r)}T00:00:00Z`)
  return dayStart >= now.getTime() - days * DAY
}
