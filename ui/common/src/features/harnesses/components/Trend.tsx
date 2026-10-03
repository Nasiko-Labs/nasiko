/**
 * Trend (plan §6): active developers per harness per day, with an Est. cost toggle. On the visx
 * chart kit (src/components/charts): lines sweep in, at rest under reduced motion.
 * Zero-series harnesses stay in the legend (muted) so colours stay stable; a visually
 * hidden summary gives the last value per harness.
 */
import { useMemo, useState } from 'react'
import { ChartDateFormat } from '@/components/charts/chart-formatters'
import { Grid } from '@/components/charts/grid'
import { Line } from '@/components/charts/line'
import { LineChart } from '@/components/charts/line-chart'
import { ChartTooltip } from '@/components/charts/tooltip/chart-tooltip'
import { XAxis } from '@/components/charts/x-axis'
import { YAxis } from '@/components/charts/y-axis'
import { ToggleGroup, ToggleGroupItem } from '@/components/ui/toggle-group'
import { fmtInt, fmtMoney, fmtMoneyAxis, fmtShortDay } from '@/lib/format'
import { Panel } from '@/components/shared/panel'
import { EmptyState } from '@/components/shared/state-card'
import { Button } from '@/components/ui/button'
import { copy } from '../copy'
import { harnessStyle } from '../rollup'
import type { UsageSeriesPoint } from '../types'
import { HarnessLabel } from './bits'

type Mode = 'active' | 'cost'
/** Axis ticks and the tooltip title in UTC (the kit's default is local time). */
const utcDay = (d: Date) => fmtShortDay(d.toISOString())
/** Hide the in-progress UTC day only when the window has more days than this. */
const PARTIAL_DAY_MIN_DAYS = 2

export function Trend({
  series,
  harnesses,
  days,
  today,
  windowLabel,
  onWiden,
  defaultMode = 'active',
}: {
  series: UsageSeriesPoint[]
  harnesses: string[]
  /** The window's UTC days (rollup windowDays): quiet days are drawn as 0, not skipped. */
  days: string[]
  /** The page's frozen "today" (UTC date). */
  today: string
  windowLabel: string
  /** Offered in the empty state (undefined when already on 30 days). */
  onWiden?: () => void
  /** The Individual level opens on cost: "active developers" is a flat 1 for one person. */
  defaultMode?: Mode
}) {
  const [mode, setMode] = useState<Mode>(defaultMode)
  // The current UTC day is still filling in; plotting it reads as a cliff on the right edge. Short
  // windows (24h spans two UTC days, today holding most of it) keep it (QA ISSUE-004).
  const shown = useMemo(
    () => (days.length > PARTIAL_DAY_MIN_DAYS ? days.filter((d) => d < today) : days),
    [days, today],
  )
  const partialHidden = shown.length < days.length
  const byKey = useMemo(() => new Map(series.map((p) => [`${p.date}|${p.harness}`, p])), [series])
  const data = useMemo(
    () =>
      shown.map((date) => {
        const row: Record<string, number | string> = { date, label: fmtShortDay(date) }
        for (const h of harnesses) {
          const p = byKey.get(`${date}|${h}`)
          row[h] = p ? (mode === 'active' ? p.active_devs : p.cost_usd) : 0
        }
        return row
      }),
    [shown, harnesses, byKey, mode],
  )
  const hasData = series.some((p) => p.active_devs > 0)
  const last = (h: string) => {
    const p = [...shown]
      .reverse()
      .map((d) => byKey.get(`${d}|${h}`))
      .find(Boolean)
    return p
      ? mode === 'active'
        ? `${p.active_devs} active`
        : fmtMoney(p.cost_usd)
      : 'no activity'
  }

  return (
    <Panel
      title={mode === 'active' ? 'Active developers per harness' : 'Estimated cost per harness'}
      subtitle={partialHidden ? 'Daily (UTC), through yesterday' : 'Daily (UTC)'}
      labelledBy="trend-title"
      actions={
        <ToggleGroup
          type="single"
          variant="outline"
          size="sm"
          value={mode}
          onValueChange={(v) => v && setMode(v as Mode)}
          aria-label="Trend metric"
        >
          <ToggleGroupItem value="active" className="px-2.5 text-xs">
            Active devs
          </ToggleGroupItem>
          <ToggleGroupItem value="cost" className="px-2.5 text-xs">
            Est. cost
          </ToggleGroupItem>
        </ToggleGroup>
      }
    >
      {!hasData ? (
        <EmptyState
          title={copy.noActivity(windowLabel)}
          action={
            onWiden ? (
              <Button size="sm" variant="outline" onClick={onWiden}>
                {copy.widen}
              </Button>
            ) : undefined
          }
        />
      ) : (
        <>
          <p className="sr-only">
            {harnesses.map((h) => `${harnessStyle(h).name}: ${last(h)}`).join('. ')}.
          </p>
          <div className="h-60 w-full" aria-hidden>
            <ChartDateFormat value={utcDay}>
              <LineChart
                data={data}
                xDataKey="date"
                aspectRatio="auto"
                className="h-full"
                margin={{ top: 8, right: 12, bottom: 28, left: 48 }}
              >
                <Grid />
                <YAxis
                  numTicks={4}
                  formatValue={(v) => (mode === 'cost' ? fmtMoneyAxis(v) : fmtInt(v))}
                />
                {harnesses.map((h) => (
                  <Line
                    key={h}
                    dataKey={h}
                    stroke={harnessStyle(h).edge}
                    strokeWidth={2}
                    fadeEdges={false}
                  />
                ))}
                <XAxis numTicks={6} />
                <ChartTooltip
                  showDatePill={false}
                  rows={(point) =>
                    harnesses.map((h) => ({
                      color: harnessStyle(h).edge,
                      label: harnessStyle(h).name,
                      value:
                        mode === 'cost' ? fmtMoney(Number(point[h])) : fmtInt(Number(point[h])),
                    }))
                  }
                />
              </LineChart>
            </ChartDateFormat>
          </div>
          <ul className="mt-2 flex flex-wrap gap-x-4 gap-y-1 text-xs">
            {harnesses.map((h) => (
              <li
                key={h}
                className={
                  series.some((p) => p.harness === h && p.active_devs > 0)
                    ? ''
                    : 'text-muted-foreground' // No activity this window: muted, not faded (opacity took it under 4.5:1)
                }
              >
                <HarnessLabel id={h} />
              </li>
            ))}
          </ul>
        </>
      )}
    </Panel>
  )
}
