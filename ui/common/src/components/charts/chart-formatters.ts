import { createContext } from 'react'

export const shortDateFmt = new Intl.DateTimeFormat('en-US', {
  month: 'short',
  day: 'numeric',
})

export const weekdayDateFmt = new Intl.DateTimeFormat('en-US', {
  weekday: 'short',
  month: 'short',
  day: 'numeric',
})

export const hmsTimeFmt = new Intl.DateTimeFormat('en-US', {
  hour: '2-digit',
  minute: '2-digit',
  second: '2-digit',
  hour12: false,
})

// `Intl.NumberFormat.prototype.format` is a bound getter — safe to extract.
export const intFmt = new Intl.NumberFormat('en-US').format

/**
 * Nasiko: how a time-series chart labels its x values (axis ticks, tooltip
 * title). Defaults to a local "Sep 9"; provide one per chart for UTC day
 * buckets or hourly data.
 */
export const ChartDateFormat = createContext<(d: Date) => string>((d) => shortDateFmt.format(d))
