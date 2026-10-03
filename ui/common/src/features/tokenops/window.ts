import { fmtMonthYear, fmtShortDay } from '@/lib/format'
import { isRealDate, type Preset } from '@/lib/search'

/**
 * Time window resolution (plan: single time control, A6, A15).
 *
 * - 24h / 7d / 30d send `range`; the server computes the window relative to its own
 *   "now" and picks hourly buckets only for 24h.
 * - This month / Last month / Custom send explicit UTC `start_time`/`end_time` (the
 *   server's calendar and day drill-down are UTC).
 * - The previous window is `[start − len, start)`, identical to the server's KPI
 *   comparison, so per-agent Δ and KPI Δ agree.
 * - `now` is rounded down to the minute so query keys stay stable across renders.
 */
export interface ApiWindowParams {
  range?: '24h' | '7d' | '30d'
  start_time?: string
  end_time?: string
}

export interface ResolvedWindow {
  preset: Preset
  label: string
  start: Date
  end: Date
  prevStart: Date
  prevEnd: Date
  /** Params for the current window. */
  params: ApiWindowParams
  /** Params for the previous window (always explicit bounds). */
  prevParams: ApiWindowParams
  /** Stable key for queries: changes only when the window changes. */
  key: string
}

const MINUTE = 60_000
const HOUR = 60 * MINUTE
const DAY = 24 * HOUR
/** One day in milliseconds, for callers that turn spans into day counts. */
export const DAY_MS = DAY
const RANGE_HOURS = { '24h': 24, '7d': 168, '30d': 720 } as const

function floorToMinute(d: Date): Date {
  return new Date(Math.floor(d.getTime() / MINUTE) * MINUTE)
}

export function utcMonthStart(d: Date, offsetMonths = 0): Date {
  return new Date(Date.UTC(d.getUTCFullYear(), d.getUTCMonth() + offsetMonths, 1))
}

export function monthKey(d: Date): string {
  return d.toISOString().slice(0, 7)
}

export function resolveWindow(
  input: { preset: Preset; from?: string; to?: string },
  nowInput: Date,
): ResolvedWindow {
  const now = floorToMinute(nowInput)
  let preset = input.preset
  let start: Date
  let end: Date
  let params: ApiWindowParams
  let label: string

  // Defence in depth: the search schema already rejects impossible dates, but an invalid
  // custom range must never reach `new Date(...).toISOString()` (it throws).
  // A future `from` (stale or hand-edited link) would clamp end below start: fall back too.
  const today = now.toISOString().slice(0, 10)
  if (
    preset === 'custom' &&
    !(
      input.from &&
      input.to &&
      isRealDate(input.from) &&
      isRealDate(input.to) &&
      input.from <= input.to &&
      input.from <= today
    )
  )
    preset = '30d'

  switch (preset) {
    case '24h':
    case '7d':
    case '30d': {
      end = now
      start = new Date(end.getTime() - RANGE_HOURS[preset] * HOUR)
      params = { range: preset }
      label = preset === '24h' ? 'Last 24 hours' : preset === '7d' ? 'Last 7 days' : 'Last 30 days'
      break
    }
    case 'mtd': {
      start = utcMonthStart(now)
      end = now
      params = { start_time: start.toISOString(), end_time: end.toISOString() }
      label = 'This month'
      break
    }
    case 'last-month': {
      start = utcMonthStart(now, -1)
      end = utcMonthStart(now)
      params = { start_time: start.toISOString(), end_time: end.toISOString() }
      label = fmtMonthYear(start)
      break
    }
    case 'custom': {
      start = new Date(`${input.from}T00:00:00Z`)
      const toEnd = new Date(`${input.to}T00:00:00Z`).getTime() + DAY
      end = new Date(Math.min(toEnd, now.getTime()))
      params = { start_time: start.toISOString(), end_time: end.toISOString() }
      label = `${fmtShortDay(start)} – ${fmtShortDay(new Date(toEnd - DAY))} (UTC)`
      break
    }
  }

  return withPrevious(preset, label, start, end, params)
}

/**
 * One UTC day: the scope of traces opened while the day panel is showing that day.
 * Clamped to now; a future day (hand-edited link, clock skew) is an empty window rather
 * than a silent fallback to 30 days.
 */
export function dayWindow(date: string, nowInput: Date): ResolvedWindow {
  const now = floorToMinute(nowInput)
  const start = new Date(`${date}T00:00:00Z`)
  const end = new Date(Math.max(start.getTime(), Math.min(start.getTime() + DAY, now.getTime())))
  return withPrevious('custom', `${fmtShortDay(start)} (UTC)`, start, end, {
    start_time: start.toISOString(),
    end_time: end.toISOString(),
  })
}

function withPrevious(
  preset: Preset,
  label: string,
  start: Date,
  end: Date,
  params: ApiWindowParams,
): ResolvedWindow {
  const len = end.getTime() - start.getTime()
  const prevStart = new Date(start.getTime() - len)
  const prevEnd = start
  return {
    preset,
    label,
    start,
    end,
    prevStart,
    prevEnd,
    params,
    prevParams: { start_time: prevStart.toISOString(), end_time: prevEnd.toISOString() },
    key: `${preset}|${start.toISOString()}|${end.toISOString()}`,
  }
}
