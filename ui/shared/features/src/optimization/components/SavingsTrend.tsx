/**
 * The lead's chart (plans/feat-optimization-page.md R5A, R4C, R7C; eng C2, C5): tokens per bucket on the kit's stacked
 * bars, Sent at the bottom and Saved on top, so the full bar is what would have gone without optimization. Buckets that
 * aren't recorded (before reporting began, or under half their requests reported) are hatched slots with no figures.
 * The chart is pointer-only (the kit's marks are aria-hidden); the Table view lists the same buckets with the same
 * action by keyboard. Clicking a bucket picks its interval (`?slice=`, shown and cleared in Biggest senders, R7C); the
 * kit's own motion, at rest under reduced motion.
 */
import { BarChart3, Table2 } from 'lucide-react'
import { useCallback, useId, useRef, useState } from 'react'
import { Bar } from '@nasiko/ui/charts/bar'
import { BarChart } from '@nasiko/ui/charts/bar-chart'
import { BarXAxis } from '@nasiko/ui/charts/bar-x-axis'
import type { TooltipData } from '@nasiko/ui/charts/chart-context'
import { ChartHover } from '@nasiko/ui/charts/chart-hover'
import { Grid } from '@nasiko/ui/charts/grid'
import { HatchPattern } from '@nasiko/ui/charts/hatch-pattern'
import { ChartTooltip } from '@nasiko/ui/charts/tooltip/chart-tooltip'
import { YAxis } from '@nasiko/ui/charts/y-axis'
import { Swatch } from '@nasiko/ui/composites/chart-marks'
import { Button } from '@nasiko/ui/primitives/button'
import {
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeader,
  TableRow,
} from '@nasiko/ui/primitives/table'
import { seriesAt } from '@nasiko/ui/lib/chart'
import { cn } from '@nasiko/ui/lib/utils'
import { copy } from '../copy'
import { pctOf, savedShare, tokensShort, type TrendRow } from '../lead'
import { type BucketSize } from '@nasiko/app-core/slice'
import { Without } from './Without'

const SENT = seriesAt(0)
const SAVED = seriesAt(1)
const MARGIN = { top: 8, right: 8, bottom: 28, left: 48 }
const axisLabel = (v: number) => (v === 0 ? '0' : tokensShort(v))

/** `label` is the axis category (short); `title` heads the tooltip when it needs more (a week). */
export type LabelledRow = TrendRow & { label: string; title?: string }

export function SavingsTrend({
  rows,
  tableRows,
  unit,
  tableUnit,
  period,
  onSelect,
}: {
  /** The chart's buckets: hours on 24h, days, or weeks below 640 px (R6B). */
  rows: LabelledRow[]
  /** The Table view's rows: always the server's own buckets (R6B keeps daily rows there). */
  tableRows: LabelledRow[]
  unit: BucketSize
  tableUnit: BucketSize
  period: string
  /** A bucket was picked: its start and size (C5). */
  onSelect: (iso: string, size: BucketSize) => void
}) {
  const [asTable, setAsTable] = useState(false)
  const hatchId = `hatch-${useId().replace(/:/g, '')}`
  const hovered = useRef<TooltipData | null>(null)
  const pressed = useRef<TooltipData | null>(null)
  const touch = useRef(false)
  const onHover = useCallback((h: TooltipData | null) => {
    hovered.current = h
  }, [])
  const sent = rows.reduce((a, r) => a + r.sent, 0)
  const saved = rows.reduce((a, r) => a + r.saved, 0)
  const summary = copy.lead.chart.summary(period, tokensShort(sent), tokensShort(saved))
  const hatch = (
    <span
      aria-hidden
      className="size-2.5 shrink-0 rounded-xs border border-border bg-[repeating-linear-gradient(45deg,var(--border)_0_1px,transparent_1px_4px)]"
    />
  )

  return (
    // Named by its figcaption (the legend plus the sr-only summary), so the summary is read once (review: design).
    <figure className="m-0 flex flex-col gap-2">
      <div className="flex flex-wrap items-center justify-between gap-2">
        <figcaption className="flex flex-wrap items-center gap-x-4 gap-y-1 text-xs text-muted-foreground">
          <span className="inline-flex items-center gap-1.5">
            <Swatch series={SENT} /> {copy.lead.chart.sent}
          </span>
          <span className="inline-flex items-center gap-1.5">
            <Swatch series={SAVED} /> {copy.lead.chart.saved}
          </span>
          <span>
            {copy.lead.chart.full} <Without label={copy.lead.chart.fullWithout} />
          </span>
          <span className="inline-flex items-center gap-1.5">
            {hatch} {copy.lead.chart.notRecorded}
          </span>
          <span className="sr-only">{summary}</span>
        </figcaption>
        <Button
          variant="ghost"
          size="sm"
          className="pointer-coarse:min-h-11"
          aria-pressed={asTable}
          onClick={() => setAsTable((v) => !v)}
        >
          {asTable ? <BarChart3 aria-hidden /> : <Table2 aria-hidden />}
          {asTable ? copy.lead.chart.chart : copy.lead.chart.table}
        </Button>
      </div>
      {asTable ? (
        <TrendTable rows={tableRows} onSelect={(iso) => onSelect(iso, tableUnit)} />
      ) : (
        // Pointer-only, so presentation: the Table view picks the same buckets by keyboard.
        <div
          role="presentation"
          className="h-55 cursor-pointer md:h-60"
          onPointerDown={(e) => {
            pressed.current = hovered.current
            touch.current = e.pointerType !== 'mouse'
          }}
          onClick={(e) => {
            // A tap has no hover before it (the kit tracks the mouse only), so the bar is the band under the finger:
            // the bands split the plot width evenly (review: red team).
            let i = pressed.current?.index
            if (touch.current) {
              const box = e.currentTarget.getBoundingClientRect()
              const plot = box.width - MARGIN.left - MARGIN.right
              const at = Math.floor(((e.clientX - box.left - MARGIN.left) / plot) * rows.length)
              i = at >= 0 && at < rows.length ? at : undefined
            }
            const r = i === undefined ? undefined : rows[i]
            if (r) onSelect(r.iso, unit)
          }}
        >
          <BarChart
            data={rows as unknown as Record<string, unknown>[]}
            xDataKey="label"
            stacked
            aspectRatio="auto"
            className="h-full"
            margin={MARGIN}
            barGap={0.3}
          >
            <HatchPattern id={hatchId} />
            <Grid />
            <YAxis formatValue={axisLabel} numTicks={3} />
            <Bar
              dataKey="sent"
              fill={SENT.fill}
              edge={SENT.edge}
              stroke={SENT.edge}
              lineCap="butt"
            />
            <Bar
              dataKey="saved"
              fill={SAVED.fill}
              edge={SAVED.edge}
              stroke={SAVED.edge}
              lineCap="butt"
            />
            <Bar
              dataKey="notRecorded"
              fill={`url(#${hatchId})`}
              stroke="var(--border)"
              lineCap="butt"
            />
            <BarXAxis maxLabels={6} />
            <ChartTooltip
              showDatePill={false}
              content={({ point }) => <TrendTooltip row={point as unknown as LabelledRow} />}
            />
            <ChartHover onChange={onHover} />
          </BarChart>
        </div>
      )}
      <p className="m-0 text-xs text-muted-foreground">{copy.lead.chart.footnote(unit)}</p>
    </figure>
  )
}

function TrendTooltip({ row }: { row: LabelledRow }) {
  return (
    <div className="px-3 py-2 text-xs text-popover-foreground">
      <div className="mb-1 font-medium">{row.title ?? row.label}</div>
      {quietText(row) ? (
        <span className="text-muted-foreground">{quietText(row)}</span>
      ) : row.recorded ? (
        <div className="grid grid-cols-[auto_auto] gap-x-3 gap-y-0.5 tabular-nums">
          <span className="text-muted-foreground">{copy.lead.chart.without}</span>
          <span>~{tokensShort(row.pool)}</span>
          <span className="text-muted-foreground">{copy.lead.chart.sent}</span>
          <span>~{tokensShort(row.sent)}</span>
          <span className="text-muted-foreground">{copy.lead.chart.savedCol}</span>
          <span>
            ~{tokensShort(row.saved)} · {pctOf(savedShare(row.pool, row.sent))}%
          </span>
        </div>
      ) : (
        <span className="text-muted-foreground">{notRecordedText(row)}</span>
      )}
    </div>
  )
}

const notRecordedText = (r: TrendRow) =>
  r.reports > 0 ? copy.lead.chart.lowCoverage(r.reports, r.eligible) : copy.lead.chart.notRecorded

/** A recorded bucket with no requests says so instead of showing zeros. */
const quietText = (r: TrendRow) =>
  r.recorded && r.eligible === 0 ? copy.lead.chart.noRequests : null

const HEAD = 'h-8 font-medium text-muted-foreground'
const NUM = 'py-1 text-right tabular-nums'

function TrendTable({ rows, onSelect }: { rows: LabelledRow[]; onSelect: (iso: string) => void }) {
  return (
    // The chart panels' Table view (TokenOps' ChartTable): compact, sticky head, a row header per row.
    <div className="max-h-72 overflow-auto [&>[data-slot=table-container]]:overflow-visible">
      <Table className="text-xs">
        <TableHeader className="sticky top-0 z-10 bg-background">
          <TableRow className="hover:bg-transparent">
            <TableHead scope="col" className={HEAD}>
              {copy.lead.chart.bucket}
            </TableHead>
            <TableHead scope="col" className={cn(HEAD, 'text-right')}>
              <Without label={copy.lead.chart.without} />
            </TableHead>
            <TableHead scope="col" className={cn(HEAD, 'text-right')}>
              {copy.lead.chart.sent}
            </TableHead>
            <TableHead scope="col" className={cn(HEAD, 'text-right')}>
              {copy.lead.chart.savedCol}
            </TableHead>
            <TableHead scope="col" className={cn(HEAD, 'text-right')}>
              {copy.lead.chart.reported}
            </TableHead>
            <TableHead scope="col" className={HEAD}>
              <span className="sr-only">{copy.lead.chart.actionCol}</span>
            </TableHead>
          </TableRow>
        </TableHeader>
        <TableBody>
          {rows.map((r) => (
            <TableRow key={r.iso}>
              <TableHead scope="row" className="h-auto py-1 font-normal">
                {r.label}
              </TableHead>
              {quietText(r) ? (
                <TableCell colSpan={3} className="py-1 text-right text-muted-foreground">
                  {quietText(r)}
                </TableCell>
              ) : r.recorded ? (
                <>
                  <TableCell className={NUM}>~{tokensShort(r.pool)}</TableCell>
                  <TableCell className={NUM}>~{tokensShort(r.sent)}</TableCell>
                  <TableCell className={NUM}>
                    ~{tokensShort(r.saved)} · {pctOf(savedShare(r.pool, r.sent))}%
                  </TableCell>
                </>
              ) : (
                <TableCell colSpan={3} className="py-1 text-right text-muted-foreground">
                  {notRecordedText(r)}
                </TableCell>
              )}
              <TableCell className={NUM}>
                {r.reports.toLocaleString('en-US')} / {r.eligible.toLocaleString('en-US')}
              </TableCell>
              <TableCell className="py-1 text-right">
                <Button
                  variant="link"
                  size="sm"
                  className="h-auto p-0 text-xs pointer-coarse:min-h-11"
                  onClick={() => onSelect(r.iso)}
                >
                  {copy.lead.chart.open}
                  <span className="sr-only"> {r.label}</span>
                </Button>
              </TableCell>
            </TableRow>
          ))}
        </TableBody>
      </Table>
    </div>
  )
}
