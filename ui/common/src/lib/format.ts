/** Number formats (plan A10). All pure, all locale-aware via Intl with en-US shapes. */

const money2 = new Intl.NumberFormat('en-US', {
  style: 'currency',
  currency: 'USD',
  minimumFractionDigits: 2,
  maximumFractionDigits: 2,
})
const moneyCompact = new Intl.NumberFormat('en-US', {
  style: 'currency',
  currency: 'USD',
  notation: 'compact',
  maximumFractionDigits: 1,
})
const compact = new Intl.NumberFormat('en-US', { notation: 'compact', maximumFractionDigits: 1 })
const int = new Intl.NumberFormat('en-US')

/** `$1,234.56`; `<$0.01` for tiny non-zero values; `$0.00` for zero. */
export function fmtMoney(v: number | null | undefined): string {
  if (v === null || v === undefined || Number.isNaN(v)) return '—'
  if (v > 0 && v < 0.01) return '<$0.01'
  return money2.format(v)
}

/** Axis labels: `$1.2K`. */
export function fmtMoneyAxis(v: number): string {
  if (Math.abs(v) < 1000) return `$${Math.round(v * 100) / 100}`
  return moneyCompact.format(v)
}

/**
 * Dates and times (plan §7.10): one locale, 'en-US', and UTC like every day key the server sends.
 * Inputs: a "YYYY-MM-DD" day key, an ISO timestamp, epoch ms or a Date.
 */
type DateInput = string | number | Date
const toDate = (input: DateInput): Date =>
  input instanceof Date
    ? input
    : new Date(
        typeof input === 'string' && /^\d{4}-\d{2}-\d{2}$/.test(input)
          ? `${input}T00:00:00Z`
          : input,
      )
const utc = (opts: Intl.DateTimeFormatOptions) =>
  new Intl.DateTimeFormat('en-US', { ...opts, timeZone: 'UTC' })
const shortDay = utc({ month: 'short', day: 'numeric' })
const longDay = utc({ weekday: 'short', month: 'short', day: 'numeric', year: 'numeric' })
const monthYear = utc({ month: 'long', year: 'numeric' })
const hourLabel = utc({ hour: 'numeric' })
// h23, not hour12: false, which writes midnight as "24:05" in en-US.
const clock = utc({ hour: '2-digit', minute: '2-digit', hourCycle: 'h23' })
const clockSec = utc({ hour: '2-digit', minute: '2-digit', second: '2-digit', hourCycle: 'h23' })

/** "Sep 26" (UTC). */
export const fmtShortDay = (input: DateInput): string => shortDay.format(toDate(input))

/** "Wed, Sep 30, 2026" (UTC). */
export const fmtLongDay = (input: DateInput): string => longDay.format(toDate(input))

/** "September 2026" (UTC). */
export const fmtMonthYear = (input: DateInput): string => monthYear.format(toDate(input))

/** "2 PM" (UTC): hourly bucket labels. */
export const fmtUtcHour = (input: DateInput): string => hourLabel.format(toDate(input))

/** "14:05", or "14:05:09" with seconds (UTC, 24 h). */
export const fmtUtcTime = (input: DateInput, seconds = false): string =>
  (seconds ? clockSec : clock).format(toDate(input))

/** "Sep 26 14:05" (UTC). */
export const fmtUtcDayTime = (input: DateInput): string =>
  `${fmtShortDay(input)} ${fmtUtcTime(input)}`

// The viewer's own clock, only where a page deliberately shows local time (always with the zone or said so).
const localClock = new Intl.DateTimeFormat('en-US', { hour: '2-digit', minute: '2-digit' })
const localClockZone = new Intl.DateTimeFormat('en-US', {
  hour: '2-digit',
  minute: '2-digit',
  timeZoneName: 'short',
})
const localDateTime = new Intl.DateTimeFormat('en-US', {
  month: 'short',
  day: 'numeric',
  year: 'numeric',
  hour: 'numeric',
  minute: '2-digit',
  timeZoneName: 'short',
})

/** "02:05 PM", or "02:05 PM GMT+2" with the zone, in the viewer's time zone. */
export const fmtLocalTime = (input: DateInput, zone = false): string =>
  (zone ? localClockZone : localClock).format(toDate(input))

/** "Sep 30, 2026, 2:05 PM GMT+2" in the viewer's time zone. */
export const fmtLocalDateTime = (input: DateInput): string => localDateTime.format(toDate(input))

/** Tight spaces (calendar cells): `$1.2K`, `$42`, `$3.14`, `<$0.01`. Same casing as axes. */
export function fmtMoneyShort(v: number): string {
  if (v > 0 && v < 0.01) return '<$0.01'
  if (v >= 1000) return moneyCompact.format(v)
  if (v >= 10) return `$${Math.round(v)}`
  return `$${v.toFixed(2)}`
}

/** Per-operation costs are small: keep 3 significant digits (`$0.0123`). */
export function fmtCostPerOp(v: number | null | undefined): string {
  if (v === null || v === undefined || Number.isNaN(v)) return '—'
  if (v === 0) return '$0.00'
  if (v >= 1) return money2.format(v)
  return `$${v.toPrecision(3)}`
}

/** `1.2M`, `850`, `12.3K`. */
export function fmtTokens(v: number | null | undefined): string {
  if (v === null || v === undefined || Number.isNaN(v)) return '—'
  return v < 1000 ? int.format(Math.round(v)) : compact.format(v)
}

export function fmtInt(v: number | null | undefined): string {
  if (v === null || v === undefined || Number.isNaN(v)) return '—'
  return int.format(Math.round(v))
}

/** `850 ms` / `2.4 s`. */
export function fmtLatency(ms: number | null | undefined): string {
  if (ms === null || ms === undefined || Number.isNaN(ms)) return '—'
  return ms < 1000 ? `${Math.round(ms)} ms` : `${(ms / 1000).toFixed(1)} s`
}

/** One decimal place. Input is a percentage (12.34 → `12.3%`). */
export function fmtPct(v: number | null | undefined): string {
  if (v === null || v === undefined || Number.isNaN(v)) return '—'
  return `${v.toFixed(1)}%`
}

export function fmtHours(v: number | null | undefined): string {
  if (v === null || v === undefined || Number.isNaN(v)) return '—'
  return `${v.toFixed(1)} h`
}

/** "≥ $1,234.56" when unpriced calls mean the total is a floor, not the truth (A3). */
export function fmtMoneyFloor(v: number, isFloor: boolean): string {
  return isFloor ? `≥ ${fmtMoney(v)}` : fmtMoney(v)
}

/** Session-scale durations: `850 ms`, `38 s`, `12 m 04 s`, `20 h 13 m`. */
export function fmtDuration(ms: number | null | undefined): string {
  if (ms === null || ms === undefined || Number.isNaN(ms)) return '—'
  if (ms < 60_000) return fmtLatency(ms)
  const totalMin = Math.floor(ms / 60_000)
  if (totalMin < 60)
    return `${totalMin} m ${String(Math.floor((ms % 60_000) / 1000)).padStart(2, '0')} s`
  return `${Math.floor(totalMin / 60)} h ${String(totalMin % 60).padStart(2, '0')} m`
}

/** File sizes in binary units (1024): `812 B`, `2.4 MB`, `98 MB` (one decimal below 10). */
export function fmtBytes(n: number): string {
  if (n < 1024) return `${n} B`
  const units = ['KB', 'MB', 'GB'] as const
  let v = n / 1024
  let u = 0
  while (v >= 1024 && u < units.length - 1) {
    v /= 1024
    u++
  }
  return `${v < 10 ? v.toFixed(1) : Math.round(v)} ${units[u]}`
}
