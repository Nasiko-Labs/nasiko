/**
 * The Spend card's stacked days (pure): the fleet-wide daily total split into the top drivers' own series (the same
 * timeseries filtered by `agent_id`) plus Other, the rest of the fleet. Other is what the drivers don't explain, never
 * negative (the filtered reads can round a cent above the total).
 */
import type { TimelinePoint } from '@/features/tokenops/series'

export interface StackSeries {
  /** The row field this series is drawn from: `s0`, `s1`, … */
  key: string
  id: string
  name: string
}

export interface StackRow {
  iso: string
  label: string
  total: number
  other: number
  [series: string]: number | string
}

export interface Stack {
  series: StackSeries[]
  rows: StackRow[]
}

const cents = (v: number) => Math.round(v * 100) / 100

export function stackSpend(
  total: readonly TimelinePoint[],
  drivers: readonly { id: string; name: string; days: readonly TimelinePoint[] }[],
): Stack {
  const series = drivers.map((d, i) => ({ key: `s${i}`, id: d.id, name: d.name }))
  const byDay = drivers.map((d) => new Map(d.days.map((p) => [p.iso.slice(0, 10), p.spend])))
  const rows = total.map((p) => {
    const row: StackRow = { iso: p.iso, label: p.label, total: p.spend, other: 0 }
    let explained = 0
    byDay.forEach((m, i) => {
      // A driver can't exceed the day it is part of.
      const v = Math.min(m.get(p.iso.slice(0, 10)) ?? 0, p.spend - explained)
      row[`s${i}`] = Math.max(0, v)
      explained += Math.max(0, v)
    })
    row.other = Math.max(0, cents(p.spend - explained))
    return row
  })
  return { series, rows }
}
