import { median } from './stats'
import type { SpendCalendarDay } from './types'

/**
 * Month-to-date and month-end forecast (plan F1, A6, A8), computed client-side from
 * `spend-calendar` — the server has no forecast.
 *
 * The forecast is a band, not a number: remaining days × the lower and × the higher of
 * the median and mean daily rates, over completed UTC days. A single spike day inflates
 * the mean but not the median, so the band shows that uncertainty honestly.
 * Hidden before FORECAST_MIN_DAYS elapsed days or when there is no spend yet.
 */
export interface MonthSummary {
  mtd: number
  /** Fractional UTC days elapsed in the month. */
  elapsedDays: number
  daysInMonth: number
  show: boolean
  low: number | null
  high: number | null
  lastMonthTotal: number
  /** Last month's spend over the same elapsed days (A6), for "vs same days last month". */
  lastMonthSameDays: number
  vsLastMonthPct: number | null
}

const DAY = 86_400_000

/** Elapsed UTC days before the month-end band is shown (the UI copy reads this too). */
export const FORECAST_MIN_DAYS = 3

export function summarizeMonth(
  thisMonth: Pick<SpendCalendarDay, 'date' | 'spend_usd'>[],
  lastMonth: Pick<SpendCalendarDay, 'date' | 'spend_usd'>[],
  now: Date,
): MonthSummary {
  const monthStart = Date.UTC(now.getUTCFullYear(), now.getUTCMonth(), 1)
  const daysInMonth = new Date(
    Date.UTC(now.getUTCFullYear(), now.getUTCMonth() + 1, 0),
  ).getUTCDate()
  const elapsedDays = Math.max(0, (now.getTime() - monthStart) / DAY)
  const completed = Math.floor(elapsedDays)

  const spendByDom = new Map<number, number>()
  for (const d of thisMonth) spendByDom.set(Number(d.date.slice(8, 10)), d.spend_usd)
  const mtd = thisMonth.reduce((s, d) => s + d.spend_usd, 0)

  // Zero-filled completed days: a quiet day is a real zero, not missing data.
  const daily = Array.from({ length: completed }, (_, i) => spendByDom.get(i + 1) ?? 0)
  const show = elapsedDays >= FORECAST_MIN_DAYS && mtd > 0 && daily.length > 0
  let low: number | null = null
  let high: number | null = null
  if (show) {
    const remaining = Math.max(0, daysInMonth - elapsedDays)
    const med = median(daily)
    const mean = daily.reduce((s, v) => s + v, 0) / daily.length
    low = mtd + remaining * Math.min(med, mean)
    high = mtd + remaining * Math.max(med, mean)
  }

  const lastMonthTotal = lastMonth.reduce((s, d) => s + d.spend_usd, 0)
  // Same days: whole days before today's day-of-month, plus today's fraction of that day.
  let lastMonthSameDays = 0
  const frac = elapsedDays - completed
  for (const d of lastMonth) {
    const dom = Number(d.date.slice(8, 10))
    if (dom <= completed) lastMonthSameDays += d.spend_usd
    else if (dom === completed + 1) lastMonthSameDays += d.spend_usd * frac
  }
  const vsLastMonthPct =
    lastMonthSameDays > 0 ? ((mtd - lastMonthSameDays) / lastMonthSameDays) * 100 : null

  return {
    mtd,
    elapsedDays,
    daysInMonth,
    show,
    low,
    high,
    lastMonthTotal,
    lastMonthSameDays,
    vsLastMonthPct,
  }
}

/** Recorded days whose mean is the pace a crossing date projects. */
export const CROSSING_PACE_DAYS = 7

export type Crossing = { kind: 'reached' } | { kind: 'crosses'; date: string }

/**
 * When a monthly limit is reached at this pace (plans/feat-agent-centric.md X5): month-to-date spend plus, per remaining
 * day, the mean of the last CROSSING_PACE_DAYS completed days; the first UTC date the running total reaches the limit.
 * Built on `summarizeMonth`: nothing before FORECAST_MIN_DAYS elapsed days or with no spend, nothing unless the band's
 * high projection reaches the limit, and nothing when the date falls after month end. Already at the limit: `reached`.
 */
export function crossingDate(
  limit: number,
  thisMonth: Pick<SpendCalendarDay, 'date' | 'spend_usd'>[],
  now: Date,
): Crossing | null {
  if (!(limit > 0)) return null
  const m = summarizeMonth(thisMonth, [], now)
  // Whole cents: 0.1 + 0.7 is 0.7999…, and still meets a $0.80 limit.
  if (Math.round(m.mtd * 100) >= Math.round(limit * 100)) return { kind: 'reached' }
  if (!m.show || m.high === null || m.high < limit) return null
  const monthStart = Date.UTC(now.getUTCFullYear(), now.getUTCMonth(), 1)
  const completed = Math.floor(m.elapsedDays)
  const spendByDom = new Map<number, number>()
  for (const d of thisMonth) spendByDom.set(Number(d.date.slice(8, 10)), d.spend_usd)
  const recent = Array.from(
    { length: Math.min(CROSSING_PACE_DAYS, completed) },
    (_, i) => spendByDom.get(completed - i) ?? 0,
  )
  const pace = recent.length ? recent.reduce((s, v) => s + v, 0) / recent.length : 0
  if (pace <= 0) return null
  const at = now.getTime() + ((limit - m.mtd) / pace) * DAY
  if (at >= monthStart + m.daysInMonth * DAY) return null
  return { kind: 'crosses', date: new Date(at).toISOString().slice(0, 10) }
}
