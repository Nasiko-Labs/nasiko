import type { FinopsDayDrilldown } from './types'

/**
 * Hourly spend concentration for one UTC day (plan F2, A9).
 *
 * Series are the day-level top 4 agents (raw trace names — the day drill-down does not
 * return display names or ids), plus "Others". Per hour, Others is the remainder
 * `max(hour.spend − Σ matched top-4 slices, 0)` so it can never go negative when the
 * server's per-hour slices and totals disagree by rounding.
 */
interface ConcentrationSeries {
  key: string
  label: string
  isOthers: boolean
}

export interface ConcentrationRow {
  hour: number
  label: string
  total: number
  [seriesKey: string]: number | string
}

export interface Concentration {
  series: ConcentrationSeries[]
  rows: ConcentrationRow[]
  total: number
  avgHourly: number
  isEmpty: boolean
}

export const OTHERS_KEY = '__others'

function hourLabel(hour: number): string {
  if (hour === 0) return '12am'
  if (hour === 12) return '12pm'
  return hour < 12 ? `${hour}am` : `${hour - 12}pm`
}

export function buildConcentration(day: FinopsDayDrilldown): Concentration {
  const top = day.top_agents.slice(0, 4)
  const series: ConcentrationSeries[] = top.map((a, i) => ({
    key: `s${i}`,
    label: a.agent_name,
    isOthers: false,
  }))
  series.push({ key: OTHERS_KEY, label: 'Others', isOthers: true })

  const rows: ConcentrationRow[] = day.hours.map((h) => {
    const row: ConcentrationRow = { hour: h.hour, label: hourLabel(h.hour), total: h.spend_usd }
    let matched = 0
    top.forEach((a, i) => {
      const slice = h.top_agents.find((s) => s.agent_name === a.agent_name)
      const v = slice ? slice.spend_usd : 0
      row[`s${i}`] = v
      matched += v
    })
    row[OTHERS_KEY] = Math.max(h.spend_usd - matched, 0)
    return row
  })

  const total = day.hours.reduce((s, h) => s + h.spend_usd, 0)
  return { series, rows, total, avgHourly: day.avg_hourly_spend_usd, isEmpty: total === 0 }
}

/**
 * The stacked segment at plot height `y` in one column (DayPanel clicks). `tops` holds each series'
 * top edge in plot pixels (0 is the top of the plot), as the chart kit reports it for the hovered
 * column; series stack bottom-up in `series` order, so a segment spans from its own top down to the
 * top of the non-empty series below it (or the baseline). Empty series and clicks above the stack
 * find nothing.
 */
export function segmentAt<S extends { key: string }>(
  series: readonly S[],
  tops: Record<string, number>,
  row: Record<string, unknown>,
  y: number,
): S | null {
  let below = Number.POSITIVE_INFINITY
  for (const s of series) {
    const top = tops[s.key]
    if (top === undefined || !(Number(row[s.key]) > 0)) continue
    if (y >= top && y < below) return s
    below = top
  }
  return null
}
