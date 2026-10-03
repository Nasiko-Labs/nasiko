/**
 * Breakdown (plan §6, G3, G11): child units, or developers, × harness. One metric per
 * cell (the toggle), sort per column, residual rows pinned below a divider and never
 * sorted, overlap footnote, CSV, and a card list below NARROW_BREAKPOINT_PX.
 *
 * Built on shadcn `Table` directly, not the shared `DataTable`: that one has no row-header column
 * (`th scope="row"`), no pinned residual group under a divider, and no per-cell props (the tooltip).
 */
import { ArrowDown, ArrowUp, ArrowUpDown, Download } from 'lucide-react'
import { useMemo, useState, type ReactNode } from 'react'
import { Button } from '@/components/ui/button'
import { Skeleton } from '@/components/ui/skeleton'
import {
  Table,
  TableBody,
  TableCaption,
  TableCell,
  TableHead,
  TableHeader,
  TableRow,
} from '@/components/ui/table'
import { ToggleGroup, ToggleGroupItem } from '@/components/ui/toggle-group'
import { downloadCsv, toCsv } from '@/features/tokenops/csv'
import { fmtInt, fmtMoney, fmtShortDay } from '@/lib/format'
import { Panel, PanelEmpty, PanelError } from '@/components/shared/panel'
import { useMediaQuery } from '@/lib/useMediaQuery'
import { cn } from '@/lib/utils'
import { NARROW_BREAKPOINT_PX } from '../constants'
import { copy } from '../copy'
import {
  cellState,
  costPerActiveDev,
  costPerSession,
  harnessStyle,
  metricValue,
  mostlyUnpriced,
  sortRows,
  splitRows,
  type Metric,
  type SortSpec,
} from '../rollup'
import type { HarnessTotals, UsageRow } from '../types'
import { HarnessLabel } from './bits'

const METRIC_LABEL: Record<Metric, string> = {
  active: 'Active devs',
  cost: 'Est. cost',
  sessions: 'Sessions',
}

function fmtMetric(t: HarnessTotals | undefined, metric: Metric): string {
  const v = metricValue(t, metric)
  if (metric === 'cost') return t && mostlyUnpriced(t) ? copy.unpriced : fmtMoney(v)
  return fmtInt(v)
}

function CellValue({
  t,
  metric,
  team,
}: {
  t: HarnessTotals | undefined
  metric: Metric
  team: boolean
}) {
  const state = cellState(t)
  if (team && state === 'idle') return <span className="text-muted-foreground">{copy.idle}</span>
  if (state === 'none')
    return (
      <span className="text-muted-foreground" aria-label={copy.notConnected}>
        —
      </span>
    )
  return <span className="tabular-nums">{fmtMetric(t, metric)}</span>
}

function tip(t: HarnessTotals | undefined): string {
  if (!t) return copy.notConnected
  const priced = !mostlyUnpriced(t)
  const per = priced ? costPerActiveDev(t) : null
  const ps = priced ? costPerSession(t) : null
  return `${t.active_devs} active / ${t.registered_devs} registered · ${priced ? `${fmtMoney(t.cost_usd)} est.` : copy.unpriced} · ${per === null ? '—' : fmtMoney(per)} per active dev · ${ps === null ? '—' : fmtMoney(ps)} per session · ${t.sessions} sessions`
}

export function Breakdown({
  title,
  rows,
  harnesses,
  metric,
  onMetric,
  highlight,
  overlap,
  onOpen,
  loading,
  stale,
  error,
  onRetry,
  team,
  total,
  hasMore,
  onLoadMore,
  loadMoreFailed,
  loadingMore,
}: {
  title: string
  rows: UsageRow[]
  harnesses: string[]
  metric: Metric
  onMetric: (m: Metric) => void
  /** The harness filter highlights its column (never removes the others). */
  highlight?: string
  overlap: number
  onOpen: (row: UsageRow) => void
  loading: boolean
  /** Previous level's rows while the next one loads (dimmed). */
  stale: boolean
  error: unknown
  onRetry: () => void
  /** Developer rows: the idle cell state and last active. */
  team: boolean
  total?: number
  hasMore?: boolean
  onLoadMore?: () => void
  /** The last "Load more" page failed: the button retries it. */
  loadMoreFailed?: boolean
  loadingMore?: boolean
}) {
  const [sort, setSort] = useState<SortSpec>({ metric, dir: 'desc' })
  const sorted = useMemo(() => sortRows(rows, { ...sort, metric }), [rows, sort, metric])
  const { main, residual } = splitRows(sorted)
  const cards = !useMediaQuery(`(min-width: ${NARROW_BREAKPOINT_PX}px)`, true)

  const toggleSort = (column?: string, byLabel?: boolean) =>
    setSort((s) =>
      s.column === column && !!s.byLabel === !!byLabel
        ? { ...s, dir: s.dir === 'desc' ? 'asc' : 'desc' }
        : { metric, column, byLabel, dir: byLabel ? 'asc' : 'desc' },
    )
  const ariaSort = (column?: string, byLabel?: boolean) =>
    (sort.column === column && !!sort.byLabel === !!byLabel
      ? sort.dir === 'asc'
        ? 'ascending'
        : 'descending'
      : 'none') as 'ascending' | 'descending' | 'none'

  const partial = total !== undefined && total > rows.length
  const exportCsv = () => {
    // A mostly-unpriced cost is blank, as on screen; the unpriced turn count travels with it.
    const cost = (t: HarnessTotals | undefined) =>
      t && mostlyUnpriced(t) ? null : (t?.cost_usd ?? 0)
    const cols = [
      { header: team ? 'Developer' : 'Unit', value: (r: UsageRow) => r.label },
      ...harnesses.flatMap((h) => {
        const n = harnessStyle(h).name
        return [
          {
            header: `${n} active devs`,
            value: (r: UsageRow) => r.harness_breakdown[h]?.active_devs ?? 0,
          },
          {
            header: `${n} registered`,
            value: (r: UsageRow) => r.harness_breakdown[h]?.registered_devs ?? 0,
          },
          {
            header: `${n} ${copy.csvCostHeader}`,
            value: (r: UsageRow) => cost(r.harness_breakdown[h]),
          },
          {
            header: `${n} unpriced turns`,
            value: (r: UsageRow) => r.harness_breakdown[h]?.unpriced_calls ?? 0,
          },
          {
            header: `${n} sessions`,
            value: (r: UsageRow) => r.harness_breakdown[h]?.sessions ?? 0,
          },
        ]
      }),
      { header: 'Total active devs', value: (r: UsageRow) => r.totals.active_devs },
      { header: `Total ${copy.csvCostHeader}`, value: (r: UsageRow) => cost(r.totals) },
      { header: 'Total sessions', value: (r: UsageRow) => r.totals.sessions },
    ]
    const csv = toCsv(cols, sorted)
    // Sort and export cover the loaded rows only (review 4a). The note goes in the file name, not a
    // leading line: a free-text first line breaks CSV parsers (pandas, csv readers, Excel headers).
    downloadCsv(partial ? `harnesses-${rows.length}-of-${total}.csv` : 'harnesses.csv', csv)
  }

  const header = (
    <div className="flex flex-wrap items-center gap-2">
      <ToggleGroup
        type="single"
        variant="outline"
        size="sm"
        value={metric}
        onValueChange={(v) => v && onMetric(v as Metric)}
        aria-label="Metric"
        className="max-sm:w-full"
      >
        {(Object.keys(METRIC_LABEL) as Metric[]).map((mm) => (
          <ToggleGroupItem key={mm} value={mm} className="px-2.5 text-xs max-sm:flex-1">
            {METRIC_LABEL[mm]}
          </ToggleGroupItem>
        ))}
      </ToggleGroup>
      <Button variant="ghost" size="sm" onClick={exportCsv} disabled={!rows.length}>
        <Download className="size-4" aria-hidden /> CSV
      </Button>
    </div>
  )

  let body
  if (loading && !rows.length) {
    body = (
      <div aria-busy="true" className="flex flex-col gap-2">
        <span className="sr-only">Loading breakdown</span>
        {[0, 1, 2, 3, 4].map((i) => (
          <Skeleton key={i} className="h-8 w-full" />
        ))}
      </div>
    )
  } else if (error && !rows.length) {
    body = <PanelError error={error} onRetry={onRetry} what="the breakdown" />
  } else if (!rows.length) {
    body = <PanelEmpty title={team ? copy.noDevelopers : copy.noUnits} />
  } else if (cards) {
    body = (
      <ul className="flex flex-col gap-2" aria-label={title}>
        {[...main, ...residual].map((r) => (
          <li key={r.key}>
            <Button
              variant="outline"
              onClick={() => onOpen(r)}
              className={cn(
                'h-auto min-h-11 w-full flex-col items-stretch gap-1.5 p-3 text-left font-normal whitespace-normal',
                r.kind === 'unassigned' || r.kind === 'direct' ? 'text-muted-foreground' : '',
              )}
            >
              <span className="flex items-baseline justify-between gap-2 font-medium">
                <span>{r.label}</span>
                <span className="tabular-nums">{fmtMetric(r.totals, metric)}</span>
              </span>
              <span className="flex flex-wrap gap-1.5">
                {harnesses.map((h) => (
                  <span
                    key={h}
                    className={cn(
                      'inline-flex items-center gap-1 rounded border border-border px-1.5 py-0.5 text-xs',
                      highlight === h && 'border-foreground/50',
                    )}
                  >
                    <HarnessLabel id={h} />{' '}
                    <CellValue t={r.harness_breakdown[h]} metric={metric} team={team} />
                  </span>
                ))}
              </span>
            </Button>
          </li>
        ))}
      </ul>
    )
  } else {
    const th = (label: ReactNode, column?: string, byLabel?: boolean, className?: string) => (
      <TableHead
        key={column ?? (byLabel ? 'label' : 'total')}
        scope="col"
        aria-sort={ariaSort(column, byLabel)}
        className={cn('text-xs font-medium text-muted-foreground', className)}
      >
        <Button
          variant="ghost"
          size="sm"
          onClick={() => toggleSort(column, byLabel)}
          className={cn('-mx-2 h-8 text-xs', !byLabel && 'ml-auto flex')}
        >
          {label}
          {ariaSort(column, byLabel) === 'ascending' ? (
            <ArrowUp aria-hidden />
          ) : ariaSort(column, byLabel) === 'descending' ? (
            <ArrowDown aria-hidden />
          ) : (
            <ArrowUpDown className="opacity-40" aria-hidden />
          )}
        </Button>
      </TableHead>
    )
    const renderRow = (r: UsageRow, residualRow: boolean) => (
      <TableRow key={r.key} className={cn(residualRow && 'text-muted-foreground')}>
        <TableHead scope="row" className="h-auto py-1.5 font-normal text-inherit">
          <Button
            variant="link"
            onClick={() => onOpen(r)}
            className="h-auto min-h-8 p-0 text-left font-normal text-inherit"
          >
            {r.label}
          </Button>
          {team && r.last_active ? (
            <span className="block text-xs text-muted-foreground">
              last active {fmtShortDay(r.last_active)}
            </span>
          ) : null}
        </TableHead>
        {harnesses.map((h) => (
          <TableCell
            key={h}
            title={tip(r.harness_breakdown[h])}
            className={cn('py-1.5 text-right', highlight === h && 'bg-muted/60')}
          >
            <CellValue t={r.harness_breakdown[h]} metric={metric} team={team} />
          </TableCell>
        ))}
        <TableCell className="py-1.5 text-right font-medium tabular-nums">
          {fmtMetric(r.totals, metric)}
        </TableCell>
      </TableRow>
    )
    body = (
      <Table>
        <TableCaption className="sr-only">
          {title}, {METRIC_LABEL[metric]} per harness
        </TableCaption>
        <TableHeader>
          <TableRow className="hover:bg-transparent">
            {th(team ? 'Developer' : 'Unit', undefined, true)}
            {harnesses.map((h) =>
              th(
                <HarnessLabel id={h} />,
                h,
                false,
                cn('text-right', highlight === h && 'bg-muted/60'),
              ),
            )}
            {th('Total', undefined, false, 'text-right')}
          </TableRow>
        </TableHeader>
        <TableBody>
          {main.map((r) => renderRow(r, false))}
          {residual.length ? (
            <TableRow aria-hidden className="border-b-2 hover:bg-transparent">
              <TableCell colSpan={harnesses.length + 2} className="h-2 p-0" />
            </TableRow>
          ) : null}
          {residual.map((r) => renderRow(r, true))}
        </TableBody>
      </Table>
    )
  }

  return (
    <Panel title={title} labelledBy="breakdown-title" actions={header}>
      <div
        className={cn('transition-opacity', stale && 'opacity-60')}
        aria-busy={stale || undefined}
      >
        {partial && rows.length ? (
          <p className="mb-2 text-xs text-muted-foreground" data-testid="partial-sort">
            {copy.partialSort(rows.length, total)}
          </p>
        ) : null}
        {body}
        {overlap > 0 ? (
          <p className="mt-2 text-xs text-muted-foreground">{copy.overlap(overlap)}</p>
        ) : null}
        {(hasMore || loadMoreFailed) && total !== undefined ? (
          <div className="mt-2 flex items-center justify-between gap-2 text-xs text-muted-foreground">
            <span>
              Showing {rows.length} of {total}
              {loadMoreFailed ? ' · the next page failed to load' : ''}
            </span>
            <Button size="sm" variant="outline" onClick={onLoadMore} disabled={loadingMore}>
              {loadMoreFailed ? 'Retry' : 'Load more'}
            </Button>
          </div>
        ) : null}
      </div>
    </Panel>
  )
}
