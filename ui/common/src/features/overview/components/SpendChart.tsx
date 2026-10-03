/**
 * The Spend card's chart: fleet spend per UTC day over the page's range, stacked by the top drivers plus Other (the
 * rest of the fleet), with the daily average as a dashed line; or the same series as lines. Colour follows the driver
 * into the table under it (DESIGN.md "Charts"), whose eye buttons hide a series here. The chart is pointer-only, so a
 * Table view lists the same days by keyboard; while a driver's series is missing it draws the fleet total alone.
 */
import { BarChart3, Table2 } from 'lucide-react'
import { useState } from 'react'
import { Bar } from '@/components/charts/bar'
import { ChartDateFormat } from '@/components/charts/chart-formatters'
import { Line } from '@/components/charts/line'
import { LineChart } from '@/components/charts/line-chart'
import { XAxis } from '@/components/charts/x-axis'
import { BarChart } from '@/components/charts/bar-chart'
import { BarXAxis } from '@/components/charts/bar-x-axis'
import { Grid } from '@/components/charts/grid'
import { ReferenceLine } from '@/components/charts/reference-line'
import { ChartTooltip } from '@/components/charts/tooltip/chart-tooltip'
import { YAxis } from '@/components/charts/y-axis'
import { Swatch } from '@/components/shared/chart-marks'
import { Button } from '@/components/ui/button'
import { ScrollArea } from '@/components/ui/scroll-area'
import { Skeleton } from '@/components/ui/skeleton'
import {
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeader,
  TableRow,
} from '@/components/ui/table'
import type { TimelinePoint } from '@/features/tokenops/series'
import { OTHER_SERIES, seriesAt, type Series } from '@/lib/chart'
import { fmtMoney, fmtMoneyAxis, fmtShortDay } from '@/lib/format'
import type { Stack, StackRow } from '../stack'
import { copy } from '../copy'
import { TOUCH } from './Card'

interface Layer {
  key: string
  name: string
  series: Series
}

const MARGIN = { top: 8, right: 8, bottom: 28, left: 48 }
/** No "$0": the baseline says it, and on a narrow card it collides with the first day's label. */
const axisLabel = (v: number) => (v === 0 ? '' : fmtMoneyAxis(v))
/** Axis ticks and the tooltip title in UTC (the kit's default is local time). */
const utcDay = (d: Date) => fmtShortDay(d.toISOString())

export type ChartMode = 'bars' | 'lines'

export function SpendChart({
  days,
  stack,
  pending,
  rangeDays,
  mode,
  hidden,
}: {
  days: readonly TimelinePoint[]
  stack: Stack | null
  pending: boolean
  rangeDays: number
  mode: ChartMode
  /** Layer keys (`s0`…, `other`) left out of the chart; the Table view keeps every layer. */
  hidden: ReadonlySet<string>
}) {
  const [asTable, setAsTable] = useState(false)
  if (pending) return <Skeleton className="h-55 w-full motion-reduce:animate-none" />
  const layers: Layer[] = stack
    ? [
        ...stack.series.map((s, i) => ({ key: s.key, name: s.name, series: seriesAt(i) })),
        { key: 'other', name: copy.spend.other, series: OTHER_SERIES },
      ]
    : [{ key: 'total', name: copy.spend.colTotal, series: seriesAt(0) }]
  const shown = layers.filter((l) => !hidden.has(l.key))
  const rows: StackRow[] = (
    stack?.rows ?? days.map((p) => ({ iso: p.iso, label: p.label, total: p.spend, other: 0 }))
  ).map((r) => ({ ...r, date: r.iso.slice(0, 10) }))
  const total = rows.reduce((s, r) => s + r.total, 0)
  const peak = rows.reduce<StackRow | null>((m, r) => (!m || r.total > m.total ? r : m), null)
  const summary = copy.spend.chartSummary(
    rangeDays,
    fmtMoney(total),
    peak && peak.total > 0 ? peak.label : null,
    fmtMoney(peak?.total ?? 0),
  )
  return (
    <figure className="m-0 flex flex-col gap-2" aria-label={summary}>
      {asTable ? (
        <DaysTable rows={rows} layers={layers} />
      ) : mode === 'lines' ? (
        <div role="presentation" className="h-55">
          <ChartDateFormat value={utcDay}>
            <LineChart
              data={rows as unknown as Record<string, unknown>[]}
              xDataKey="date"
              aspectRatio="auto"
              className="h-full"
              margin={MARGIN}
            >
              <Grid />
              <YAxis formatValue={axisLabel} numTicks={4} />
              {shown.map((l) => (
                <Line
                  key={l.key}
                  dataKey={l.key}
                  stroke={l.series.edge}
                  strokeWidth={2}
                  fadeEdges={false}
                />
              ))}
              <XAxis numTicks={6} />
              <ChartTooltip
                showDatePill={false}
                content={({ point }) => (
                  <DayTooltip row={point as unknown as StackRow} layers={shown} />
                )}
              />
            </LineChart>
          </ChartDateFormat>
        </div>
      ) : (
        // Pointer-only (the kit's marks are aria-hidden): the Table view carries the same days by keyboard.
        <div role="presentation" className="h-55">
          <BarChart
            data={rows as unknown as Record<string, unknown>[]}
            xDataKey="label"
            stacked
            stackGap={2}
            aspectRatio="auto"
            className="h-full"
            margin={MARGIN}
            barGap={0.3}
          >
            <Grid />
            <YAxis formatValue={axisLabel} numTicks={4} />
            {shown.map((l) => (
              <Bar
                key={l.key}
                dataKey={l.key}
                fill={l.series.fill}
                edge={l.series.edge}
                stroke={l.series.edge}
                lineCap="butt"
              />
            ))}
            <ReferenceLine
              value={total / (rows.length || 1)}
              label={copy.spend.avg(fmtMoney(total / (rows.length || 1)))}
            />
            <BarXAxis maxLabels={6} />
            <ChartTooltip
              showDatePill={false}
              content={({ point }) => (
                <DayTooltip row={point as unknown as StackRow} layers={shown} />
              )}
            />
          </BarChart>
        </div>
      )}
      <figcaption className="flex flex-wrap items-center justify-between gap-2 text-xs text-muted-foreground">
        {stack ? (
          <span className="inline-flex items-center gap-1.5">
            <Swatch series={OTHER_SERIES} /> {copy.spend.other}: {copy.spend.otherNote}
          </span>
        ) : (
          <span />
        )}
        <Button
          variant="ghost"
          size="sm"
          className={`h-7 text-xs ${TOUCH}`}
          aria-pressed={asTable}
          onClick={() => setAsTable((v) => !v)}
        >
          {asTable ? <BarChart3 aria-hidden /> : <Table2 aria-hidden />}
          {asTable ? copy.spend.showChart : copy.spend.showTable}
        </Button>
      </figcaption>
    </figure>
  )
}

function DayTooltip({ row, layers }: { row: StackRow; layers: readonly Layer[] }) {
  return (
    <div className="px-3 py-2 text-xs text-popover-foreground">
      <div className="mb-1 font-medium tabular-nums">
        {row.label} · {fmtMoney(row.total)}
      </div>
      {layers.length > 1 ? (
        <div className="grid grid-cols-[auto_1fr_auto] items-center gap-x-2 gap-y-0.5 tabular-nums">
          {layers.map((l) => (
            <span key={l.key} className="contents">
              <Swatch series={l.series} />
              <span className="truncate text-muted-foreground">{l.name}</span>
              <span className="text-right">{fmtMoney(Number(row[l.key]))}</span>
            </span>
          ))}
        </div>
      ) : null}
    </div>
  )
}

function DaysTable({ rows, layers }: { rows: readonly StackRow[]; layers: readonly Layer[] }) {
  const cols = layers.length > 1 ? layers : []
  return (
    <ScrollArea className="[&>[data-slot=scroll-area-viewport]]:max-h-55">
      <Table className="text-xs">
        <TableHeader>
          <TableRow className="hover:bg-transparent">
            <TableHead scope="col" className="h-8 px-1 text-xs text-muted-foreground">
              {copy.spend.colDay}
            </TableHead>
            {cols.map((l) => (
              <TableHead
                key={l.key}
                scope="col"
                className="h-8 max-w-32 truncate px-1 text-right text-xs text-muted-foreground"
              >
                {l.name}
              </TableHead>
            ))}
            <TableHead scope="col" className="h-8 px-1 text-right text-xs text-muted-foreground">
              {copy.spend.colTotal}
            </TableHead>
          </TableRow>
        </TableHeader>
        <TableBody>
          {rows
            .filter((r) => r.total > 0)
            .map((r) => (
              <TableRow key={r.iso}>
                <TableHead scope="row" className="h-8 px-1 text-xs font-normal">
                  {r.label}
                </TableHead>
                {cols.map((l) => (
                  <TableCell
                    key={l.key}
                    className="px-1 text-right text-muted-foreground tabular-nums"
                  >
                    {fmtMoney(Number(r[l.key]))}
                  </TableCell>
                ))}
                <TableCell className="px-1 text-right font-medium tabular-nums">
                  {fmtMoney(r.total)}
                </TableCell>
              </TableRow>
            ))}
        </TableBody>
      </Table>
    </ScrollArea>
  )
}
