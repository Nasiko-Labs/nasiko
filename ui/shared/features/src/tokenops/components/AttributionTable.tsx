/**
 * F3 — "Who or what is driving cost?" (plan A1, A2b, A7, A9, A11, A12, A14, A18).
 *
 * Rows come from the ACL-scoped dashboard (current + previous window), joined by id.
 * Sort is a URL param (history replace); search is a URL param (replace). CSV exports
 * exactly the visible rows in the current sort, with formula-safe cells.
 * On narrow screens the table becomes a card list.
 */
import { Crosshair, Download, ListTree } from 'lucide-react'
import { createContext, use, useEffect, useMemo, useRef, useState } from 'react'
import { Badge } from '@nasiko/ui/primitives/badge'
import { Button } from '@nasiko/ui/primitives/button'
import { Card } from '@nasiko/ui/primitives/card'
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from '@nasiko/ui/primitives/select'
import { AgentLink } from '@nasiko/features/agents/components/AgentLink'
import { Toggle } from '@nasiko/ui/primitives/toggle'
import { pageCount, pageOf } from '@nasiko/ui/lib/paging'
import { prefersReducedMotion } from '@nasiko/ui/lib/useMediaQuery'
import { cn } from '@nasiko/ui/lib/utils'
import { searchRows, sortRows, type AttributionRow } from '../attribution'
import { downloadCsv, toCsv, type CsvColumn } from '../csv'
import { POLARITY } from '@nasiko/ui/lib/delta'
import {
  fmtCostPerOp,
  fmtHours,
  fmtInt,
  fmtLatency,
  fmtMoney,
  fmtPct,
  fmtTokens,
} from '@nasiko/ui/lib/format'
import type { SortKey } from '../search'
import { Panel, PanelEmpty, PanelError, PanelSkeleton } from '@nasiko/ui/composites/panel'
import { Delta } from '@nasiko/ui/composites/delta'
import { Pager } from '@nasiko/ui/composites/pager'
import { DataTable, type DataTableColumn } from '@nasiko/ui/composites/data-table'
import { SearchInput } from '@nasiko/ui/composites/search-input'
import { Segmented } from './Segmented'

const VIEWS = [
  { value: 'agent', label: 'Agents' },
  { value: 'workflow', label: 'Workflows' },
] as const

const SORT_LABELS: Record<SortKey, string> = {
  cost: 'Highest spend',
  tokens: 'Most tokens',
  operations: 'Most operations',
  latency: 'Slowest',
  hours: 'Most container hours',
  name: 'Name',
}

/** Rows shown before Show all. */
const ROW_CAP = 10

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
  onSelect,
  onLast30,
  unpriced,
  pageSize,
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
  onSelect?: (agentId: string) => void
  /** Offered in the empty-window state; undefined when already on 30d (A2). */
  onLast30?: () => void
  unpriced: boolean
  pageSize?: number
}) {
  const visible = useMemo(() => sortRows(searchRows(rows ?? [], q), sort), [rows, q, sort])
  // The top rows answer "who"; the long tail waits behind Show all, so the section stays one screen. A search or a
  // highlight from the scatter below the fold shows everything (CSV always exports every visible row).
  const [expanded, setExpanded] = useState(false)
  const highlightBelow =
    !!highlightId && visible.slice(0, ROW_CAP).every((r) => r.id !== highlightId)
  const all = expanded || !!q || highlightBelow || visible.length <= ROW_CAP + 2
  const firstPage = () => {
    const at = highlightId ? visible.findIndex((r) => r.id === highlightId) : -1
    return pageSize && at >= 0 ? Math.floor(at / pageSize) : 0
  }
  const [page, setPage] = useState(firstPage)
  const [pagedFor, setPagedFor] = useState({ visible, highlightId })
  if (pagedFor.visible !== visible || pagedFor.highlightId !== highlightId) {
    setPagedFor({ visible, highlightId })
    setPage(firstPage())
  }
  const pages = pageSize ? pageCount(visible.length, pageSize) : 1
  const current = Math.min(page, pages - 1)
  // A page size (Weave's views) pages the rows; without one, the top rows and Show all.
  const shown = pageSize
    ? pageOf(visible, current, pageSize)
    : all
      ? visible
      : visible.slice(0, ROW_CAP)
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
          <Segmented label="Group by" value={view} options={VIEWS} onChange={onView} />
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
            rows={shown}
            view={view}
            sort={sort}
            prevUnavailable={prevUnavailable}
            highlightId={highlightId}
            onViewTraces={onViewTraces}
            onSelect={onSelect}
          />
          <MobileCards
            rows={shown}
            view={view}
            prevUnavailable={prevUnavailable}
            highlightId={highlightId}
            onViewTraces={onViewTraces}
            onSelect={onSelect}
          />
          {!pageSize && visible.length > ROW_CAP + 2 && !q && !highlightBelow ? (
            <Button
              variant="ghost"
              size="sm"
              className="w-fit text-xs"
              aria-expanded={expanded}
              onClick={() => setExpanded((v) => !v)}
            >
              {expanded ? `Show top ${ROW_CAP}` : `Show all ${visible.length}`}
            </Button>
          ) : null}
          <Pager page={current} pages={pages} onPage={setPage} />
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
const CellCtx = createContext<{
  prevUnavailable: boolean
  onViewTraces: (id: string) => void
  onSelect?: (id: string) => void
  highlightId?: string
}>({
  prevUnavailable: true,
  onViewTraces: () => {},
})

function DeltaCell({ row }: { row: AttributionRow }) {
  return <RowDelta row={row} prevUnavailable={use(CellCtx).prevUnavailable} />
}

function FocusToggle({ row: r }: { row: AttributionRow }) {
  const { onSelect, highlightId } = use(CellCtx)
  if (!onSelect) return null
  const on = r.id === highlightId
  return (
    <Toggle
      size="sm"
      pressed={on}
      onPressedChange={() => onSelect(on ? '' : r.id)}
      className="h-7 px-2 data-pressed:bg-primary/10 data-pressed:text-primary-text"
      aria-label={on ? `Stop focusing on ${r.name}` : `Focus on ${r.name}`}
      title={on ? 'In focus' : 'Focus'}
    >
      <Crosshair className="size-3.5" aria-hidden />
    </Toggle>
  )
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
            cell: ({ row }) => (
              <div className="flex items-center justify-end">
                <FocusToggle row={row.original} />
                <TracesCell row={row.original} />
              </div>
            ),
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
  onSelect,
}: {
  rows: AttributionRow[]
  view: 'agent' | 'workflow'
  sort: SortKey
  prevUnavailable: boolean
  highlightId?: string
  onViewTraces: (id: string) => void
  onSelect?: (id: string) => void
}) {
  const highlightRef = useScrollToHighlight<HTMLTableRowElement>(highlightId)
  return (
    <CellCtx value={{ prevUnavailable, onViewTraces, onSelect, highlightId }}>
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
  onSelect,
}: {
  rows: AttributionRow[]
  view: 'agent' | 'workflow'
  prevUnavailable: boolean
  highlightId?: string
  onViewTraces: (id: string) => void
  onSelect?: (id: string) => void
}) {
  const highlightRef = useScrollToHighlight<HTMLLIElement>(highlightId)
  return (
    <ul className="flex flex-col gap-2 md:hidden">
      {rows.map((r) => (
        <Card
          key={r.id}
          className={cn('gap-0 rounded-md p-3 shadow-none', r.id === highlightId && HIGHLIGHT)}
          render={<li ref={r.id === highlightId ? highlightRef : undefined} />}
        >
          <div className="flex items-center justify-between gap-2">
            <span className="truncate font-medium">{r.name}</span>
            <span className="tabular-nums">{fmtMoney(r.cost)}</span>
          </div>
          <div className="mt-1 flex items-center justify-between text-xs text-muted-foreground">
            <span>{fmtPct(r.sharePct)} of spend</span>
            <RowDelta row={r} prevUnavailable={prevUnavailable} />
          </div>
          {view === 'agent' ? (
            <div className="mt-1 flex gap-4">
              <Button
                variant="link"
                size="sm"
                className="h-auto min-h-11 w-fit p-0 text-xs"
                onClick={() => onViewTraces(r.id)}
              >
                View traces
              </Button>
              {onSelect ? (
                <Button
                  variant="link"
                  size="sm"
                  className="h-auto min-h-11 w-fit p-0 text-xs"
                  aria-pressed={r.id === highlightId}
                  onClick={() => onSelect(r.id === highlightId ? '' : r.id)}
                >
                  {r.id === highlightId ? 'In focus' : 'Focus'}
                </Button>
              ) : null}
            </div>
          ) : null}
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
