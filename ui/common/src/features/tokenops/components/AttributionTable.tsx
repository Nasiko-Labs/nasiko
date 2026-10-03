/**
 * F3 — "Who or what is driving cost?" (plan A1, A2b, A7, A9, A11, A12, A14, A18).
 *
 * Rows come from the ACL-scoped dashboard (current + previous window), joined by id.
 * Sort is a URL param (history replace); search is a URL param (replace). CSV exports
 * exactly the visible rows in the current sort, with formula-safe cells.
 * On narrow screens the table becomes a card list.
 */
import { Download, ListTree } from 'lucide-react'
import { createContext, use, useEffect, useMemo, useRef } from 'react'
import { Badge } from '@/components/ui/badge'
import { Button } from '@/components/ui/button'
import { Card } from '@/components/ui/card'
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from '@/components/ui/select'
import { AgentLink } from '@/features/agents/components/AgentLink'
import { ToggleGroup, ToggleGroupItem } from '@/components/ui/toggle-group'
import { prefersReducedMotion } from '@/lib/useMediaQuery'
import { cn } from '@/lib/utils'
import { searchRows, sortRows, type AttributionRow } from '../attribution'
import { downloadCsv, toCsv, type CsvColumn } from '../csv'
import { POLARITY } from '@/lib/delta'
import {
  fmtCostPerOp,
  fmtHours,
  fmtInt,
  fmtLatency,
  fmtMoney,
  fmtPct,
  fmtTokens,
} from '@/lib/format'
import type { SortKey } from '../search'
import { Panel, PanelEmpty, PanelError, PanelSkeleton } from '@/components/shared/panel'
import { Delta } from '@/components/shared/delta'
import { DataTable, type DataTableColumn } from '@/components/shared/data-table'
import { SearchInput } from '@/components/shared/search-input'

const SORT_LABELS: Record<SortKey, string> = {
  cost: 'Highest spend',
  tokens: 'Most tokens',
  operations: 'Most operations',
  latency: 'Slowest',
  hours: 'Most container hours',
  name: 'Name',
}

const HIGHLIGHT = 'bg-primary/10 ring-1 ring-primary/40'

export function AttributionTable({
  rows,
  view,
  sort,
  q,
  loading,
  error,
  onRetry,
  prevUnavailable,
  highlightId,
  agentFilter,
  onView,
  onSort,
  onSearch,
  onViewTraces,
  onLast30,
  unpriced,
}: {
  rows: AttributionRow[] | undefined
  view: 'agent' | 'workflow'
  sort: SortKey
  q?: string
  loading: boolean
  error: unknown
  onRetry: () => void
  prevUnavailable: boolean
  highlightId?: string
  agentFilter?: string
  onView: (v: 'agent' | 'workflow') => void
  onSort: (s: SortKey) => void
  onSearch: (q: string) => void
  onViewTraces: (agentId: string) => void
  /** Offered in the empty-window state; undefined when already on 30d (A2). */
  onLast30?: () => void
  unpriced: boolean
}) {
  const visible = useMemo(() => sortRows(searchRows(rows ?? [], q), sort), [rows, q, sort])
  const noun = view === 'agent' ? 'Agent' : 'Workflow'
  const sortOptions = (Object.keys(SORT_LABELS) as SortKey[]).filter(
    (s) => view === 'agent' || s !== 'hours',
  )

  const columns: CsvColumn<AttributionRow>[] = [
    { header: noun, value: (r) => r.name },
    { header: 'Spend (USD)', value: (r) => r.cost },
    { header: 'Share (%)', value: (r) => Math.round(r.sharePct * 10) / 10 },
    {
      header: 'Change vs previous (%)',
      value: (r) => (r.deltaPct === null ? null : Math.round(r.deltaPct * 10) / 10),
    },
    { header: 'Tokens', value: (r) => r.tokens },
    { header: view === 'agent' ? 'Operations' : 'Executions', value: (r) => r.operations },
    { header: 'Cost per operation (USD)', value: (r) => r.costPerOp },
    { header: `${view === 'agent' ? 'p50' : 'Average'} latency (ms)`, value: (r) => r.latency },
    {
      header: 'Cache-read ratio (%)',
      value: (r) => (r.cacheRatioPct === null ? null : Math.round(r.cacheRatioPct * 10) / 10),
    },
    ...(view === 'agent'
      ? [{ header: 'Container hours', value: (r: AttributionRow) => r.containerHours }]
      : []),
  ]

  return (
    <Panel
      title="Who is driving cost"
      subtitle={`Share and change vs the previous period of equal length · source: agent dashboard (agents you can access)${unpriced ? ' · excludes unpriced calls' : ''}`}
      labelledBy="attribution-title"
      actions={
        <>
          <ToggleGroup
            type="single"
            variant="outline"
            size="sm"
            value={view}
            onValueChange={(v) => v && onView(v as 'agent' | 'workflow')}
            aria-label="Group by"
          >
            <ToggleGroupItem value="agent" className="px-2.5 text-xs">
              Agents
            </ToggleGroupItem>
            <ToggleGroupItem value="workflow" className="px-2.5 text-xs">
              Workflows
            </ToggleGroupItem>
          </ToggleGroup>
          <Select value={sort} onValueChange={(v) => onSort(v as SortKey)}>
            <SelectTrigger size="sm" className="w-44" aria-label="Sort by">
              <SelectValue />
            </SelectTrigger>
            <SelectContent>
              {sortOptions.map((s) => (
                <SelectItem key={s} value={s}>
                  {SORT_LABELS[s]}
                </SelectItem>
              ))}
            </SelectContent>
          </Select>
          <SearchInput
            value={q ?? ''}
            onChange={(e) => onSearch(e.target.value)}
            placeholder={`Search ${noun.toLowerCase()}s…`}
            aria-label={`Search ${noun.toLowerCase()}s`}
            className="h-8 w-40"
          />
          <Button
            variant="outline"
            size="sm"
            disabled={!visible.length}
            onClick={() => downloadCsv(`tokenops-${view}s.csv`, toCsv(columns, visible))}
          >
            <Download className="size-4" aria-hidden /> CSV
          </Button>
        </>
      }
    >
      {loading && !rows ? (
        <PanelSkeleton height={240} />
      ) : error && !rows ? (
        <PanelError error={error} onRetry={onRetry} what="cost attribution" />
      ) : !rows?.length ? (
        agentFilter ? (
          <PanelEmpty title="This agent isn't in the current agent list">
            It may be deleted or not accessible to you. Its spend still appears in the calendar and
            timeline, which are fleet-wide.
          </PanelEmpty>
        ) : (
          <PanelEmpty title={`No ${noun.toLowerCase()} activity in this window`}>
            {onLast30 ? (
              <Button variant="outline" size="sm" className="mt-2" onClick={onLast30}>
                Show last 30 days
              </Button>
            ) : null}
          </PanelEmpty>
        )
      ) : !visible.length ? (
        <PanelEmpty title={`No ${noun.toLowerCase()}s match “${q}”`} />
      ) : (
        <>
          <DesktopTable
            rows={visible}
            view={view}
            sort={sort}
            prevUnavailable={prevUnavailable}
            highlightId={highlightId}
            onViewTraces={onViewTraces}
          />
          <MobileCards
            rows={visible}
            view={view}
            prevUnavailable={prevUnavailable}
            highlightId={highlightId}
            onViewTraces={onViewTraces}
          />
        </>
      )}
    </Panel>
  )
}

type Col = DataTableColumn<AttributionRow>
const NUM = { numeric: true, headerClassName: 'px-1.5 2xl:px-2', cellClassName: 'px-1.5 2xl:px-2' }

/**
 * What cells need beyond their row. FlexRender renders each `cell` as a component type, so column
 * defs must keep their identity across renders or every cell remounts (and a focused Traces button
 * is lost when the drawer closes): the defs live at module level and read these through context.
 */
const CellCtx = createContext<{ prevUnavailable: boolean; onViewTraces: (id: string) => void }>({
  prevUnavailable: true,
  onViewTraces: () => {},
})

function DeltaCell({ row }: { row: AttributionRow }) {
  return <RowDelta row={row} prevUnavailable={use(CellCtx).prevUnavailable} />
}

function TracesCell({ row: r }: { row: AttributionRow }) {
  const { onViewTraces } = use(CellCtx)
  // Icon-only: the 11-column table doesn't fit its 8/12 slot with a label (aria-label names it).
  return (
    <Button
      variant="ghost"
      size="sm"
      className="h-7 px-2 text-xs"
      onClick={() => onViewTraces(r.id)}
      aria-label={`View traces for ${r.name}`}
      title="View traces"
    >
      <ListTree className="size-3.5" aria-hidden />
    </Button>
  )
}

/** Column ids are the SortKey the column is ordered by, so `sorting` marks aria-sort on it whatever its label. */
function columnsFor(view: 'agent' | 'workflow'): Col[] {
  return [
    {
      id: 'name',
      header: view === 'agent' ? 'Agent' : 'Workflow',
      meta: { cellClassName: 'max-w-55' },
      cell: ({ row: { original: r } }) => (
        <div className="flex items-center gap-1.5">
          {view === 'agent' ? (
            <AgentLink id={r.id} name={r.name} className="truncate font-medium">
              {r.name}
            </AgentLink>
          ) : (
            <span className="truncate font-medium">{r.name}</span>
          )}
          {r.capped ? (
            <Badge
              variant="outline"
              className="text-2xs"
              title="Operations were capped by the aggregation limit; totals undercount."
            >
              ~approx
            </Badge>
          ) : null}
        </div>
      ),
    },
    { id: 'cost', header: 'Spend', meta: NUM, cell: ({ row }) => fmtMoney(row.original.cost) },
    {
      id: 'share',
      header: 'Share',
      meta: NUM,
      cell: ({ row: { original: r } }) => (
        <div className="flex items-center justify-end gap-2">
          <div className="hidden h-1.5 w-12 rounded-full bg-muted 2xl:block" aria-hidden>
            <div
              className="h-1.5 rounded-full bg-chart-1-edge"
              style={{ width: `${Math.min(100, r.sharePct)}%` }}
            />
          </div>
          <span>{fmtPct(r.sharePct)}</span>
        </div>
      ),
    },
    {
      id: 'delta',
      header: () => <span title="Change vs previous period">Δ</span>,
      meta: NUM,
      cell: ({ row }) => <DeltaCell row={row.original} />,
    },
    {
      id: 'tokens',
      header: 'Tokens',
      meta: NUM,
      cell: ({ row }) => fmtTokens(row.original.tokens),
    },
    {
      id: 'operations',
      header: view === 'agent' ? 'Operations' : 'Executions',
      meta: NUM,
      cell: ({ row }) => fmtInt(row.original.operations),
    },
    {
      id: 'costPerOp',
      header: 'Cost/op',
      meta: NUM,
      cell: ({ row }) => fmtCostPerOp(row.original.costPerOp),
    },
    // Agents carry p50; workflows carry the server's arithmetic mean (WorkflowFinopsRow.avg_latency_ms).
    {
      id: 'latency',
      header: view === 'agent' ? 'p50' : 'Avg latency',
      meta: NUM,
      cell: ({ row }) => fmtLatency(row.original.latency),
    },
    {
      id: 'cache',
      header: 'Cache read',
      meta: NUM,
      cell: ({ row }) =>
        row.original.cacheRatioPct === null ? '—' : fmtPct(row.original.cacheRatioPct),
    },
    ...(view === 'agent'
      ? ([
          {
            id: 'hours',
            header: 'Hours',
            meta: NUM,
            cell: ({ row }) => fmtHours(row.original.containerHours),
          },
          {
            id: 'traces',
            header: '',
            meta: NUM,
            cell: ({ row }) => <TracesCell row={row.original} />,
          },
        ] satisfies Col[])
      : []),
  ]
}

const COLUMNS = { agent: columnsFor('agent'), workflow: columnsFor('workflow') }

/** Rows arrive sorted (`sortRows`); DataTable only marks the sorted column. */
function DesktopTable({
  rows,
  view,
  sort,
  prevUnavailable,
  highlightId,
  onViewTraces,
}: {
  rows: AttributionRow[]
  view: 'agent' | 'workflow'
  sort: SortKey
  prevUnavailable: boolean
  highlightId?: string
  onViewTraces: (id: string) => void
}) {
  const highlightRef = useScrollToHighlight<HTMLTableRowElement>(highlightId)
  return (
    <CellCtx value={{ prevUnavailable, onViewTraces }}>
      <DataTable
        className="hidden md:block"
        label="Who is driving cost"
        columns={COLUMNS[view]}
        data={rows}
        getRowId={(r) => r.id}
        sorting={[{ id: sort, desc: sort !== 'name' }]}
        rowProps={(r) => (r.id === highlightId ? { ref: highlightRef, className: HIGHLIGHT } : {})}
      />
    </CellCtx>
  )
}

/** F5 → F3 cross-highlight: the highlighted row scrolls into view (both layouts). */
function useScrollToHighlight<T extends HTMLElement>(highlightId: string | undefined) {
  const ref = useRef<T | null>(null)
  useEffect(() => {
    // Native smooth scrolling isn't covered by MotionConfig reducedMotion="user".
    ref.current?.scrollIntoView?.({
      block: 'nearest',
      behavior: prefersReducedMotion() ? 'auto' : 'smooth',
    })
  }, [highlightId])
  return ref
}

function MobileCards({
  rows,
  view,
  prevUnavailable,
  highlightId,
  onViewTraces,
}: {
  rows: AttributionRow[]
  view: 'agent' | 'workflow'
  prevUnavailable: boolean
  highlightId?: string
  onViewTraces: (id: string) => void
}) {
  const highlightRef = useScrollToHighlight<HTMLLIElement>(highlightId)
  return (
    <ul className="flex flex-col gap-2 md:hidden">
      {rows.map((r) => (
        <Card
          asChild
          key={r.id}
          className={cn('gap-0 rounded-md p-3 shadow-none', r.id === highlightId && HIGHLIGHT)}
        >
          <li ref={r.id === highlightId ? highlightRef : undefined}>
            <div className="flex items-center justify-between gap-2">
              <span className="truncate font-medium">{r.name}</span>
              <span className="tabular-nums">{fmtMoney(r.cost)}</span>
            </div>
            <div className="mt-1 flex items-center justify-between text-xs text-muted-foreground">
              <span>{fmtPct(r.sharePct)} of spend</span>
              <RowDelta row={r} prevUnavailable={prevUnavailable} />
            </div>
            {view === 'agent' ? (
              <Button
                variant="link"
                size="sm"
                className="mt-1 h-auto min-h-11 w-fit p-0 text-xs"
                onClick={() => onViewTraces(r.id)}
              >
                View traces
              </Button>
            ) : null}
          </li>
        </Card>
      ))}
    </ul>
  )
}

function RowDelta({ row, prevUnavailable }: { row: AttributionRow; prevUnavailable: boolean }) {
  if (prevUnavailable || row.deltaKind === 'unavailable')
    return <Delta changePct={null} polarity={POLARITY.spend} unavailable />
  if (row.deltaKind === 'new')
    return (
      <Badge variant="secondary" className="text-2xs">
        new
      </Badge>
    )
  if (row.deltaKind === 'no-spend' && row.deltaPct === null)
    return <span className="text-xs text-muted-foreground">—</span>
  return <Delta changePct={row.deltaPct} polarity={POLARITY.spend} />
}
