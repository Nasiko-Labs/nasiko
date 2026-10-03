/**
 * Span tree + waterfall in one list (ARIA tree): name on the left, a bar on a shared time
 * axis on the right. Hue = span class (chart tokens, with a legend); tokens show as a thin
 * inner bar and a label, never opacity alone; errors get a destructive outline AND an icon.
 * Same-name siblings collapse into a "×N" group row. Arrow keys move, ←/→ collapse/expand,
 * Enter selects. The Table view lists the same spans in start order.
 * From Axiom's trace view: each bar carries its duration, the time axis repeats at the
 * bottom, and spans outside a "Filter spans" query are dimmed (the tree stays intact).
 */
import { AlertCircle, ChevronRight, Coins } from 'lucide-react'
import { useEffect, useMemo, useRef, type KeyboardEvent } from 'react'
import { DataTable, type DataTableColumn } from '@/components/shared/data-table'
import { Badge } from '@/components/ui/badge'
import { Button } from '@/components/ui/button'
import { copy } from '@/features/observability/copy'
import {
  CLASS_LABEL,
  isError,
  ownTokens,
  statusText,
  treeRows,
  type FlatSpan,
  type SpanClass,
  type TreeRow,
} from '@/features/observability/spans'
import { DURATION_LABEL_END_PCT, DURATION_LABEL_START_PCT } from '@/features/observability/tuning'
import { fmtLatency, fmtTokens } from '@/lib/format'
import { cn } from '@/lib/utils'

/** Span-kind bars: the pastel fill inside a 1 px edge, so a thin bar still reads on white (DESIGN.md "Charts"). */
const CLASS_COLOR: Record<SpanClass, string> = {
  planner: 'bg-chart-1 border border-chart-1-edge',
  llm: 'bg-chart-2 border border-chart-2-edge',
  tool: 'bg-chart-3 border border-chart-3-edge',
  agent: 'bg-chart-4 border border-chart-4-edge',
  other: 'bg-chart-other border border-chart-other-edge',
}

function Axis({ total }: { total: number }) {
  const ticks = [0, 0.25, 0.5, 0.75, 1]
  return (
    <div className="relative h-5 text-2xs text-muted-foreground" aria-hidden>
      {ticks.map((t) => (
        <span
          key={t}
          className={cn(
            'absolute top-0 -translate-x-1/2 whitespace-nowrap tabular-nums first:translate-x-0 last:-translate-x-full',
            t > 0 && t < 1 && 'hidden sm:inline',
          )}
          style={{ left: `${t * 100}%` }}
        >
          {fmtLatency(total * t)}
        </span>
      ))}
    </div>
  )
}

export function Legend() {
  return (
    <div
      className="flex flex-wrap items-center gap-3 text-xs text-muted-foreground"
      aria-label="Legend"
    >
      {(Object.keys(CLASS_COLOR) as SpanClass[])
        .filter((c) => c !== 'other')
        .map((c) => (
          <span key={c} className="inline-flex items-center gap-1.5">
            <span className={cn('h-2.5 w-4 rounded-sm', CLASS_COLOR[c])} aria-hidden />{' '}
            {CLASS_LABEL[c]}
          </span>
        ))}
      <span className="inline-flex items-center gap-1.5">
        <span className="h-2.5 w-4 rounded-sm border-2 border-destructive" aria-hidden />
        <AlertCircle className="size-3 text-destructive" aria-hidden /> Error
      </span>
      <span className="inline-flex items-center gap-1.5">
        <Coins className="size-3" aria-hidden />
        <span className="h-1 w-4 rounded-full bg-foreground/70" aria-hidden /> Tokens
      </span>
    </div>
  )
}

function rowKey(r: TreeRow): string {
  return r.kind === 'group'
    ? `g:${r.key}`
    : r.span.node.id + (r.attempt ? `#${r.attempt.index}` : '')
}

function describe(s: FlatSpan): string {
  return `starts ${fmtLatency(s.startMs)}, lasts ${fmtLatency(s.durationMs)}, ${fmtTokens(ownTokens(s.node))} tokens${isError(s.node) ? ', error' : ''}`
}

export function Waterfall({
  spans,
  total,
  selectedId,
  onSelect,
  expanded,
  collapsed,
  onToggleGroup,
  onToggleNode,
  highlight,
  dimmed,
}: {
  spans: FlatSpan[]
  total: number
  selectedId?: string
  onSelect: (s: FlatSpan) => void
  expanded: ReadonlySet<string>
  collapsed: ReadonlySet<string>
  onToggleGroup: (key: string) => void
  onToggleNode: (id: string) => void
  highlight: ReadonlySet<string>
  /** Span ids that don't match the current span filter. */
  dimmed?: ReadonlySet<string>
}) {
  const rows = useMemo(() => treeRows(spans, expanded, collapsed), [spans, expanded, collapsed])
  const maxTokens = Math.max(1, ...spans.map((s) => ownTokens(s.node)))
  const refs = useRef(new Map<string, HTMLDivElement>())
  const focusRow = (i: number) => {
    const r = rows[Math.max(0, Math.min(rows.length - 1, i))]
    if (r) refs.current.get(rowKey(r))?.focus()
  }
  // Keep the selected row visible (deep links, default selection).
  useEffect(() => {
    const r = rows.find((x) => x.kind === 'span' && x.span.node.id === selectedId)
    if (r) refs.current.get(rowKey(r))?.scrollIntoView?.({ block: 'nearest' })
  }, [rows, selectedId])

  const onKey = (e: KeyboardEvent, i: number, r: TreeRow) => {
    if (e.key === 'ArrowDown') {
      e.preventDefault()
      focusRow(i + 1)
    } else if (e.key === 'ArrowUp') {
      e.preventDefault()
      focusRow(i - 1)
    } else if (e.key === 'Home') {
      e.preventDefault()
      focusRow(0)
    } else if (e.key === 'End') {
      e.preventDefault()
      focusRow(rows.length - 1)
    } else if (e.key === 'ArrowRight' || e.key === 'ArrowLeft') {
      e.preventDefault()
      const open = e.key === 'ArrowRight'
      if (r.kind === 'group' && expanded.has(r.key) !== open) onToggleGroup(r.key)
      else if (r.kind === 'span' && r.span.childCount && collapsed.has(r.span.node.id) === open)
        onToggleNode(r.span.node.id)
    } else if (e.key === 'Enter' || e.key === ' ') {
      e.preventDefault()
      if (r.kind === 'span') onSelect(r.span)
      else onToggleGroup(r.key)
    }
  }

  const selectedIndex = Math.max(
    0,
    rows.findIndex((r) => r.kind === 'span' && r.span.node.id === selectedId),
  )
  return (
    <div className="min-w-0">
      <div className="grid grid-cols-[minmax(10rem,38%)_1fr] gap-x-3 border-b border-border px-2 pb-1">
        <span className="text-xs text-muted-foreground">Span</span>
        <Axis total={total} />
      </div>
      <div role="tree" aria-label="Spans" className="flex flex-col">
        {rows.map((r, i) => {
          const key = rowKey(r)
          const depth = r.kind === 'group' ? r.depth : r.span.depth
          const start = r.kind === 'group' ? r.startMs : r.span.startMs
          const dur = r.kind === 'group' ? r.durationMs : r.span.durationMs
          const err = r.kind === 'group' ? r.failures > 0 : isError(r.span.node)
          const cls = r.kind === 'group' ? r.cls : r.span.cls
          const tokens =
            r.kind === 'group'
              ? r.members.reduce((a, m) => a + ownTokens(m.node), 0)
              : ownTokens(r.span.node)
          const selected = r.kind === 'span' && r.span.node.id === selectedId
          const lit =
            r.kind === 'span'
              ? highlight.has(r.span.node.id)
              : r.members.some((m) => highlight.has(m.node.id))
          const endPct = ((start + dur) / Math.max(1, total)) * 100
          const startPct = (start / Math.max(1, total)) * 100
          const dim =
            !!dimmed?.size &&
            (r.kind === 'span'
              ? dimmed.has(r.span.node.id)
              : r.members.every((m) => dimmed.has(m.node.id)))
          const isOpen =
            r.kind === 'group'
              ? expanded.has(r.key)
              : r.span.childCount && !r.attempt
                ? !collapsed.has(r.span.node.id)
                : undefined
          const baseLabel =
            r.kind === 'group'
              ? `${r.name} ×${r.members.length}, ${r.failures} failed, ${fmtLatency(r.durationMs)} total`
              : `${r.attempt ? `${r.span.node.name} attempt ${r.attempt.index} of ${r.attempt.of}` : r.span.node.name}, ${CLASS_LABEL[r.span.cls]}, ${describe(r.span)}`
          // Dimming is never the only signal: the accessible name says so too.
          const label = dim ? `${baseLabel}, ${copy.filteredOut}` : baseLabel
          return (
            <div
              key={key}
              ref={(el) => {
                if (el) refs.current.set(key, el)
                else refs.current.delete(key)
              }}
              role="treeitem"
              aria-level={depth + 1}
              aria-selected={selected}
              aria-expanded={isOpen}
              aria-label={label}
              tabIndex={i === selectedIndex ? 0 : -1}
              onKeyDown={(e) => onKey(e, i, r)}
              onClick={() => (r.kind === 'span' ? onSelect(r.span) : onToggleGroup(r.key))}
              className={cn(
                'grid cursor-pointer grid-cols-[minmax(10rem,38%)_1fr] items-center gap-x-3 rounded-md px-2 py-1 text-sm outline-none focus-visible:ring-2 focus-visible:ring-ring',
                selected ? 'bg-accent ring-2 ring-ring' : 'hover:bg-accent/60',
                lit && !selected && 'bg-primary/10',
                dim && 'opacity-50',
              )}
            >
              <span className="flex min-w-0 items-center gap-1" style={{ paddingLeft: depth * 14 }}>
                {isOpen !== undefined ? (
                  // Mouse-only: the treeitem owns focus and ←/→ (tree keyboard model).
                  <Button
                    variant="ghost"
                    size="icon-xs"
                    tabIndex={-1}
                    aria-hidden
                    onClick={(e) => {
                      e.stopPropagation()
                      if (r.kind === 'group') onToggleGroup(r.key)
                      else onToggleNode(r.span.node.id)
                    }}
                    className="size-5 text-muted-foreground"
                  >
                    <ChevronRight
                      className={cn('size-3.5 transition-transform', isOpen && 'rotate-90')}
                    />
                  </Button>
                ) : (
                  <span className="w-5" />
                )}
                {err ? (
                  <AlertCircle className="size-3.5 shrink-0 text-destructive" aria-hidden />
                ) : null}
                <span
                  className="truncate font-mono text-code"
                  title={r.kind === 'group' ? r.name : r.span.node.name}
                >
                  {r.kind === 'group' ? (
                    <>
                      {r.name}{' '}
                      <span className="text-muted-foreground">
                        ×{r.members.length}
                        {r.failures ? ` · ${r.failures} failed` : ''}
                      </span>
                    </>
                  ) : r.attempt ? (
                    <>
                      attempt {r.attempt.index} of {r.attempt.of}
                    </>
                  ) : (
                    r.span.node.name
                  )}
                </span>
                {/* Beside the name, with an icon, so the count reads as this span's tokens. */}
                {tokens ? (
                  <span className="inline-flex shrink-0 items-center gap-0.5 pl-1.5 text-xs text-muted-foreground tabular-nums">
                    <Coins className="size-3" aria-hidden />
                    {fmtTokens(tokens)}
                  </span>
                ) : null}
              </span>
              <span className="relative h-5">
                <span
                  className={cn(
                    'absolute top-0.5 h-4 min-w-1 rounded-sm',
                    CLASS_COLOR[cls],
                    err && 'outline-2 outline-offset-0 outline-destructive',
                  )}
                  style={{
                    left: `${(start / Math.max(1, total)) * 100}%`,
                    width: `${Math.max(0.4, (dur / Math.max(1, total)) * 100)}%`,
                  }}
                >
                  {tokens ? (
                    <span
                      className="absolute inset-x-0.5 bottom-0.5 h-1 rounded-full bg-background/80"
                      style={{ width: `${Math.max(8, (tokens / maxTokens) * 100)}%` }}
                    />
                  ) : null}
                </span>
                {/* Duration beside the bar (inside the row's right edge when the bar ends late). */}
                <span
                  aria-hidden
                  className={cn(
                    'absolute top-0.5 hidden px-1 text-2xs leading-4 whitespace-nowrap tabular-nums sm:inline',
                    endPct >= DURATION_LABEL_END_PCT && startPct <= DURATION_LABEL_START_PCT
                      ? // Inside the bar: a page-coloured chip, since white on a chart fill is under 4.5:1 (axe).
                        'rounded-sm bg-background/90 font-medium text-foreground'
                      : 'text-muted-foreground',
                  )}
                  style={
                    endPct < DURATION_LABEL_END_PCT
                      ? { left: `${endPct}%` }
                      : startPct > DURATION_LABEL_START_PCT
                        ? { right: `${100 - startPct}%` }
                        : { right: `${100 - endPct}%` }
                  }
                >
                  {fmtLatency(dur)}
                </span>
              </span>
            </div>
          )
        })}
      </div>
      <div className="grid grid-cols-[minmax(10rem,38%)_1fr] gap-x-3 border-t border-border px-2 pt-1">
        <span />
        <Axis total={total} />
      </div>
    </div>
  )
}

const STATUS_BADGE = { ok: 'success', error: 'destructive', unset: 'muted' } as const

const tableColumns = (
  selectedId: string | undefined,
  onSelect: (s: FlatSpan) => void,
): DataTableColumn<FlatSpan>[] => [
  {
    id: 'name',
    header: 'Name',
    cell: ({ row }) => (
      <Button
        variant="link"
        size="sm"
        className="h-auto p-0 font-mono text-code font-normal text-foreground"
        onClick={() => onSelect(row.original)}
        aria-pressed={row.original.node.id === selectedId}
      >
        {row.original.node.name}
      </Button>
    ),
  },
  {
    id: 'kind',
    header: 'Kind',
    // The waterfall's swatch, so the two views read alike.
    cell: ({ row }) => (
      <span className="inline-flex items-center gap-1.5">
        <span className={cn('h-2.5 w-4 rounded-sm', CLASS_COLOR[row.original.cls])} aria-hidden />
        {CLASS_LABEL[row.original.cls]}
      </span>
    ),
  },
  {
    id: 'start',
    header: 'Start',
    meta: { numeric: true },
    cell: ({ row }) => fmtLatency(row.original.startMs),
  },
  {
    id: 'duration',
    header: 'Duration',
    meta: { numeric: true },
    cell: ({ row }) => fmtLatency(row.original.durationMs),
  },
  {
    id: 'tokens',
    header: 'Tokens',
    meta: { numeric: true },
    cell: ({ row }) => fmtTokens(ownTokens(row.original.node)),
  },
  {
    id: 'status',
    header: 'Status',
    // Status badges as on Builds and Agents: a tint plus the word, never colour alone.
    cell: ({ row }) => {
      const status = statusText(row.original.node)
      return (
        <Badge variant={STATUS_BADGE[status]}>
          {status === 'error' ? <AlertCircle aria-hidden /> : null}
          {status}
        </Badge>
      )
    },
  },
]

/** Keyboard- and screen-reader-friendly list of the same spans, in start order. */
export function SpanTable({
  spans,
  selectedId,
  onSelect,
}: {
  spans: FlatSpan[]
  selectedId?: string
  onSelect: (s: FlatSpan) => void
}) {
  const ordered = [...spans].sort((a, b) => a.startMs - b.startMs)
  if (!ordered.length)
    return <p className="py-3 text-sm text-muted-foreground">{copy.noSpansMatch}</p>
  return (
    <DataTable
      label="Spans in start order"
      columns={tableColumns(selectedId, onSelect)}
      data={ordered}
      getRowId={(s) => s.node.id}
      rowProps={(s) => ({ className: cn(s.node.id === selectedId && 'bg-accent') })}
    />
  )
}
