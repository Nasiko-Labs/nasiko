/**
 * F2 — "Why did spend spike?" (plan A9, A11, A16, A20).
 *
 * Spend as bars (left axis) + operations as a line (right axis), on the visx chart kit
 * (src/components/charts): bars grow and the line sweeps in, at rest under reduced motion.
 * The tooltip names the bucket's top agent. Clicking a bucket opens that UTC day's hourly
 * breakdown; clicks resolve by index to the raw ISO bucket start, never to the formatted label.
 * A Table toggle gives a keyboard/screen-reader alternative with the same actions.
 */
import { Table2, BarChart3 } from 'lucide-react'
import { useCallback, useRef, useState } from 'react'
import { ChartDateFormat } from '@/components/charts/chart-formatters'
import type { TooltipData } from '@/components/charts/chart-context'
import { ChartHover } from '@/components/charts/chart-hover'
import { ComposedChart } from '@/components/charts/composed-chart'
import { Grid } from '@/components/charts/grid'
import { Line } from '@/components/charts/line'
import { SeriesBar } from '@/components/charts/series-bar'
import { ChartTooltip } from '@/components/charts/tooltip/chart-tooltip'
import { XAxis } from '@/components/charts/x-axis'
import { YAxis } from '@/components/charts/y-axis'
import { Button } from '@/components/ui/button'
import { TableBody, TableCell, TableHead, TableHeader, TableRow } from '@/components/ui/table'
import { fmtInt, fmtLatency, fmtMoney, fmtMoneyAxis, fmtPct, fmtShortDay } from '@/lib/format'
import { cn } from '@/lib/utils'
import { bucketLabel, type TimelinePoint } from '../series'
import { Panel, PanelEmpty, PanelError, PanelSkeleton } from '@/components/shared/panel'
import { CELL, ChartTable, HEAD, ROW_HEAD, STICKY_HEAD } from './ChartTable'
import { Swatch } from '@/components/shared/chart-marks'
import { seriesAt } from '@/lib/chart'

const SPEND = seriesAt(0)
const OPS = seriesAt(1)

export function SpendTimeline({
  points,
  bucket,
  loading,
  error,
  onRetry,
  onOpenDay,
  onLast30,
  windowLabel,
}: {
  points: TimelinePoint[]
  bucket: 'hour' | 'day'
  loading: boolean
  error: unknown
  onRetry: () => void
  onOpenDay: (date: string) => void
  /** Offered in the empty state; undefined when already on 30d (A2). */
  onLast30?: () => void
  windowLabel: string
}) {
  const [asTable, setAsTable] = useState(false)
  const hovered = useRef<TooltipData | null>(null)
  const pressed = useRef<TooltipData | null>(null)
  const onHover = useCallback((h: TooltipData | null) => {
    hovered.current = h
  }, [])
  // UTC labels (the kit's default is local time); hourly buckets say the hour.
  const fmtX = useCallback((d: Date) => bucketLabel(d, bucket), [bucket])
  const total = points.reduce((s, p) => s + p.spend, 0)
  const peak = points.reduce<TimelinePoint | null>(
    (m, p) => (!m || p.spend > m.spend ? p : m),
    null,
  )
  const summary =
    peak && total > 0
      ? `Spend over ${windowLabel}: ${fmtMoney(total)}. Peak ${peak.label} at ${fmtMoney(peak.spend)}${peak.topAgent ? `, driven by ${peak.topAgent}` : ''}.`
      : `No spend over ${windowLabel}.`

  return (
    <Panel
      title="Spend over time"
      subtitle={`${bucket === 'hour' ? 'Hourly' : 'Daily'} buckets (UTC) · click a bar, or use the Table view by keyboard, to see that day hour by hour`}
      labelledBy="spend-timeline-title"
      actions={
        <Button variant="ghost" size="sm" onClick={() => setAsTable((v) => !v)}>
          {asTable ? (
            <BarChart3 className="size-4" aria-hidden />
          ) : (
            <Table2 className="size-4" aria-hidden />
          )}
          {asTable ? 'Chart' : 'Table'}
        </Button>
      }
    >
      {loading && !points.length ? (
        <PanelSkeleton height={260} />
      ) : error && !points.length ? (
        <PanelError error={error} onRetry={onRetry} what="spend over time" />
      ) : total === 0 ? (
        <PanelEmpty title="No spend in this window">
          Try a longer window, or clear filters.
          {onLast30 ? (
            <div>
              <Button variant="outline" size="sm" className="mt-2" onClick={onLast30}>
                Show last 30 days
              </Button>
            </div>
          ) : null}
        </PanelEmpty>
      ) : asTable ? (
        <TimelineTable points={points} onOpenDay={onOpenDay} />
      ) : (
        <figure aria-label={summary} className="m-0">
          {/* The chart is pointer-only (aria-hidden children), so presentation: the Table view opens the same days by keyboard. */}
          <div
            role="presentation"
            className="h-55 cursor-pointer md:h-70"
            onPointerDown={() => {
              pressed.current = hovered.current
            }}
            onClick={() => {
              const p = pressed.current ? points[pressed.current.index] : undefined
              if (p) onOpenDay(p.iso.slice(0, 10))
            }}
          >
            <ChartDateFormat value={fmtX}>
              <ComposedChart
                data={points as unknown as Record<string, unknown>[]}
                xDataKey="iso"
                aspectRatio="auto"
                className="h-full"
                margin={{ top: 8, right: 44, bottom: 36, left: 56 }}
                barGap={2}
              >
                <Grid />
                <SeriesBar
                  dataKey="spend"
                  fill={SPEND.fill}
                  edge={SPEND.edge}
                  stroke={SPEND.edge}
                />
                <Line
                  dataKey="operations"
                  yAxisId="ops"
                  stroke={OPS.edge}
                  strokeWidth={2}
                  fadeEdges={false}
                />
                <YAxis formatValue={fmtMoneyAxis} numTicks={4} />
                <YAxis
                  yAxisId="ops"
                  orientation="right"
                  formatValue={(v) => fmtInt(v)}
                  numTicks={4}
                />
                <XAxis numTicks={6} />
                <ChartTooltip
                  showDatePill={false}
                  content={({ point }) => <TimelineTooltip p={point as unknown as TimelinePoint} />}
                />
                <ChartHover onChange={onHover} />
              </ComposedChart>
            </ChartDateFormat>
          </div>
          <figcaption className="mt-2 flex flex-wrap gap-4 text-xs text-muted-foreground">
            <span className="inline-flex items-center gap-1.5">
              <Swatch series={SPEND} /> Spend (left)
            </span>
            <span className="inline-flex items-center gap-1.5">
              <Swatch series={OPS} line /> Operations (right)
            </span>
            <span className="sr-only">{summary}</span>
          </figcaption>
        </figure>
      )}
    </Panel>
  )
}

function TimelineTooltip({ p }: { p: TimelinePoint }) {
  const share = p.topAgentSpend !== null && p.spend > 0 ? (p.topAgentSpend / p.spend) * 100 : null
  return (
    <div className="px-3 py-2 text-xs text-popover-foreground">
      <div className="mb-1 font-medium">{p.label}</div>
      <div className="grid grid-cols-[auto_auto] gap-x-3 gap-y-0.5 tabular-nums">
        <span className="text-muted-foreground">Spend</span>
        <span>{fmtMoney(p.spend)}</span>
        <span className="text-muted-foreground">Operations</span>
        <span>{fmtInt(p.operations)}</span>
        <span className="text-muted-foreground">p95</span>
        <span>{fmtLatency(p.p95)}</span>
      </div>
      {p.topAgent ? (
        <div className="mt-1 border-t border-border pt-1">
          Top: {p.topAgent} ({fmtMoney(p.topAgentSpend)}
          {share !== null ? `, ${fmtPct(share)}` : ''})
        </div>
      ) : null}
    </div>
  )
}

function TimelineTable({
  points,
  onOpenDay,
}: {
  points: TimelinePoint[]
  onOpenDay: (date: string) => void
}) {
  return (
    <ChartTable>
      <TableHeader className={STICKY_HEAD}>
        <TableRow className="hover:bg-transparent">
          <TableHead scope="col" className={HEAD}>
            Bucket
          </TableHead>
          <TableHead scope="col" className={HEAD}>
            Spend
          </TableHead>
          <TableHead scope="col" className={HEAD}>
            Ops
          </TableHead>
          <TableHead scope="col" className={HEAD}>
            Top agent
          </TableHead>
          <TableHead scope="col" className={HEAD}>
            <span className="sr-only">Action</span>
          </TableHead>
        </TableRow>
      </TableHeader>
      <TableBody>
        {points
          .filter((p) => p.spend > 0)
          .map((p) => (
            <TableRow key={p.iso}>
              <TableHead scope="row" className={ROW_HEAD}>
                {p.label}
              </TableHead>
              <TableCell className={cn(CELL, 'tabular-nums')}>{fmtMoney(p.spend)}</TableCell>
              <TableCell className={cn(CELL, 'tabular-nums')}>{fmtInt(p.operations)}</TableCell>
              <TableCell className={cn(CELL, 'truncate')}>{p.topAgent ?? '—'}</TableCell>
              <TableCell className={cn(CELL, 'text-right')}>
                <Button
                  variant="link"
                  size="sm"
                  className="h-auto p-0 text-xs"
                  onClick={() => onOpenDay(p.iso.slice(0, 10))}
                >
                  Open day<span className="sr-only"> {fmtShortDay(p.iso)}</span>
                </Button>
              </TableCell>
            </TableRow>
          ))}
      </TableBody>
    </ChartTable>
  )
}
