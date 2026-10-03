/**
 * Session trace: why one run cost what it did.
 *
 *   breadcrumb ─ header (session line, trace line) ─ narrative (≤ 3 sentences + takeaway)
 *   trace switcher (2+ traces) ─ tree + waterfall | span panel      (375 px: panel is a sheet)
 *
 * Default trace: the failing one if known, else the one with the most tokens. Default span:
 * failing, then most tokens, then slowest. `?span=` is the HEX span id.
 */
import { useQueryClient } from '@tanstack/react-query'
import { Link, useRouter } from '@tanstack/react-router'
import { Link2, ListTree, Table2, Workflow } from 'lucide-react'
import { useDeferredValue, useMemo, useState } from 'react'
import { PageHeader } from '@/components/shared/page-header'
import { PageLoader } from '@/components/shared/page-loader'
import { EmptyState } from '@/components/shared/state-card'
import { SearchInput } from '@/components/shared/search-input'
import {
  Breadcrumb,
  BreadcrumbItem,
  BreadcrumbLink,
  BreadcrumbList,
  BreadcrumbSeparator,
} from '@/components/ui/breadcrumb'
import { Button } from '@/components/ui/button'
import { Toggle } from '@/components/ui/toggle'
import { OpenChatLink } from '@/features/chat/components/OpenChatLink'
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from '@/components/ui/select'
import { Sheet, SheetContent, SheetTitle } from '@/components/ui/sheet'
import { pickShared, withoutWindow } from '@/app/shell/context'
import {
  narrativeText,
  traceNarrative,
  wasteCostSpans,
  wasteFetchable,
  type Clause,
} from '@/features/narrative/trace'
import { copy } from '@/features/observability/copy'
import { bySize, entryCost } from '@/features/observability/sessions'
import {
  defaultSpan,
  findRetryLoops,
  flattenSpans,
  groupKeysOf,
  isToolCall,
  isTraceFailing,
  traceDurationMs,
  type FlatSpan,
} from '@/features/observability/spans'
import { ErrorState } from '@/features/observability/StateCard'
import { IN_PROGRESS_MS } from '@/features/observability/tuning'
import type { TraceDetail, TraceEntry } from '@/features/observability/types'
import { AgentLink } from '@/features/agents/components/AgentLink'
import { useAgentsDirectory } from '@/features/agents/api'
import { sessionKeys, useSessionDetail } from '@/features/sessions/api'
import type { SessionsSearch, TraceSearch } from '@/features/sessions/search'
import { fmtLatency, fmtMoney, fmtShortDay, fmtTokens, fmtUtcDayTime } from '@/lib/format'
import { useMediaQuery } from '@/lib/useMediaQuery'
import { useAgentCallTargets, useKnownFailing, useTraceDetail, useWasteCosts } from './api'
import { SessionOptimisation } from './SessionOptimisation'
import { SpanPanel, type SpanTab } from './SpanPanel'
import { Legend, SpanTable, Waterfall } from './Waterfall'
import { useCopy } from '@/lib/useCopy'
import { useFrozenNow } from '@/lib/useReturnTick'

type SetTraceSearch = (patch: Partial<TraceSearch>, opts?: { replace?: boolean }) => void

export function SessionTracePage({
  sessionId,
  search,
  setSearch,
}: {
  sessionId: string
  search: TraceSearch
  setSearch: SetTraceSearch
}) {
  const client = useQueryClient()
  const router = useRouter()
  const session = useSessionDetail(sessionId)
  const agents = useAgentsDirectory()
  const traces = useMemo(() => session.data?.traces ?? [], [session.data])

  // Default trace: a failing one we already know about, else the most tokens.
  const defaultTraceId = useMemo(() => {
    const ranked = bySize(traces)
    const known = ranked.find((t) => {
      const cached = client.getQueryData<TraceDetail>(sessionKeys.trace(t.trace_id))
      return cached && isTraceFailing(cached)
    })
    return (known ?? ranked[0])?.trace_id
  }, [traces, client])
  const defaultFailing = useKnownFailing(defaultTraceId)
  const traceId =
    search.trace && traces.some((t) => t.trace_id === search.trace) ? search.trace : defaultTraceId
  const entry = traces.find((t) => t.trace_id === traceId)
  const lastStart = entry?.root_span.start_time
    ? Date.parse(entry.root_span.start_time) + entry.root_span.latency_ms
    : 0
  const now = useFrozenNow(traceId).getTime()
  const trace = useTraceDetail(traceId, !!lastStart && now - lastStart < IN_PROGRESS_MS)

  const spans = useMemo(() => (trace.data ? flattenSpans(trace.data) : []), [trace.data])
  const total = traceDurationMs(spans)
  const loop = useMemo(() => findRetryLoops(spans)[0], [spans])
  const costSpans = useMemo(
    () => (loop && wasteFetchable(loop) ? wasteCostSpans(loop) : []),
    [loop],
  )
  const wasteCosts = useWasteCosts(traceId, costSpans, costSpans.length > 0)
  // Agent-call callees are only on SpanDetail attributes (agent.id): fetch ≤ 3.
  const callSpans = useMemo(() => spans.filter((s) => s.cls === 'agent').slice(0, 3), [spans])
  const callTargets = useAgentCallTargets(traceId, callSpans)
  const narrative = useMemo(() => {
    if (!trace.data) return null
    const agentName = (s: FlatSpan) => {
      const id = callTargets.get(s.node.id)
      if (!id) return undefined
      const a = agents.byId.get(id)
      return a ? a.display_name || a.name : undefined
    }
    return traceNarrative({
      totalCost: trace.data.cost_summary.total.cost,
      spans,
      wasteCosts,
      agentName,
    })
  }, [trace.data, spans, wasteCosts, agents.byId, callTargets])

  // Selection: ?span (hex) if it exists in this trace, else the default span.
  const wantSpan = search.span?.toLowerCase()
  const deepLinked = wantSpan
    ? spans.find((s) => s.node.span_id.toLowerCase() === wantSpan)
    : undefined
  const fallback = useMemo(() => defaultSpan(spans), [spans])
  const selected = deepLinked ?? fallback
  const unknownSpan = !!search.span && spans.length > 0 && !deepLinked

  // The group containing the selection starts open; users can open or close any group.
  const [openGroups, setOpenGroups] = useState<ReadonlySet<string>>(new Set())
  const [closedGroups, setClosedGroups] = useState<ReadonlySet<string>>(new Set())
  const [collapsed, setCollapsed] = useState<ReadonlySet<string>>(new Set())
  const selectedGroups = useMemo(
    () => (selected ? groupKeysOf(spans, selected.node.id) : []),
    [spans, selected],
  )
  const expanded = useMemo(() => {
    const s = new Set(openGroups)
    for (const g of selectedGroups) if (!closedGroups.has(g)) s.add(g)
    return s
  }, [openGroups, closedGroups, selectedGroups])
  const toggleGroup = (key: string) => {
    const open = expanded.has(key)
    setOpenGroups((prev) => {
      const n = new Set(prev)
      if (open) n.delete(key)
      else n.add(key)
      return n
    })
    setClosedGroups((prev) => {
      const n = new Set(prev)
      if (open) n.add(key)
      else n.delete(key)
      return n
    })
  }
  const toggleNode = (id: string) =>
    setCollapsed((prev) => {
      const next = new Set(prev)
      if (next.has(id)) next.delete(id)
      else next.add(id)
      return next
    })

  const [highlight, setHighlight] = useState<ReadonlySet<string>>(new Set())
  const [asTable, setAsTable] = useState(false)
  // "Filter spans" (Axiom): non-matching spans are dimmed in the waterfall, hidden in the table.
  // Deferred so typing stays responsive on large traces.
  const [spanQuery, setSpanQuery] = useState('')
  const needle = useDeferredValue(spanQuery).trim().toLowerCase()
  const { matches, dimmed } = useMemo(() => {
    if (!needle) return { matches: spans, dimmed: new Set<string>() }
    const hit: FlatSpan[] = []
    const miss = new Set<string>()
    for (const s of spans) {
      if (
        s.node.name.toLowerCase().includes(needle) ||
        (s.node.model ?? '').toLowerCase().includes(needle)
      )
        hit.push(s)
      else miss.add(s.node.id)
    }
    return { matches: hit, dimmed: miss }
  }, [spans, needle])
  // Previous/next span (Braintrust) walks what's on screen: the filter's matches, in tree
  // order (or start order in the Table view). Stepping into a collapsed node opens its ancestors.
  const [panelTab, setPanelTab] = useState<SpanTab>('summary')
  const walk = useMemo(
    () => (asTable ? [...matches].sort((a, b) => a.startMs - b.startMs) : matches),
    [asTable, matches],
  )
  const selectedIndex = selected ? walk.findIndex((s) => s.node.id === selected.node.id) : -1
  const step = (delta: number) => {
    const s = walk[selectedIndex + delta]
    if (!s) return
    const byId = new Map(spans.map((x) => [x.node.id, x]))
    const ancestors = new Set<string>()
    for (
      let p = s.node.parent_id ? byId.get(s.node.parent_id) : undefined;
      p && !ancestors.has(p.node.id);
      p = p.node.parent_id ? byId.get(p.node.parent_id) : undefined
    )
      ancestors.add(p.node.id)
    setCollapsed((prev) =>
      ancestors.size && [...prev].some((id) => ancestors.has(id))
        ? new Set([...prev].filter((id) => !ancestors.has(id)))
        : prev,
    )
    setSearch({ trace: traceId, span: s.node.span_id })
  }
  const position = selectedIndex >= 0 ? { index: selectedIndex + 1, of: walk.length } : undefined
  const parentName = selected?.node.parent_id
    ? spans.find((s) => s.node.id === selected.node.parent_id)?.node.name
    : undefined
  const counts = useMemo(
    () => ({
      llm: spans.filter((s) => s.cls === 'llm').length,
      // Real tool invocations only: HTTP, DB and memory client spans are coloured as tools too.
      tool: spans.filter((s) => isToolCall(s.node)).length,
      agent: spans.filter((s) => s.cls === 'agent').length,
    }),
    [spans],
  )
  const [showDetails, setShowDetails] = useState(false)
  const [copied, copyText] = useCopy()
  const narrow = !useMediaQuery('(min-width: 64rem)', true)
  const [sheetOpen, setSheetOpen] = useState(false)

  const select = (s: FlatSpan) => {
    // Pin the trace too: the default trace depends on this browser's cache, a shared link must not.
    setSearch({ trace: traceId, span: s.node.span_id })
    if (narrow) setSheetOpen(true)
  }
  const copyLink = () => {
    // The copied link names the trace and span explicitly (see select()).
    const loc = router.buildLocation({
      from: '/sessions/$sessionId',
      to: '.',
      search: (prev) => ({ ...prev, trace: traceId, span: selected?.node.span_id ?? prev.span }),
    })
    void copyText(new URL(loc.href, window.location.origin).toString())
  }

  const back = {
    ...pickShared(search),
    day: search.day,
    sort: search.sort,
    lane: search.lane,
    status: search.status,
    live: search.live,
  }
  const agentRaw = session.data?.agent_name ?? ''
  const agentLabel = agentRaw
    ? (agents.byName.get(agentRaw)?.display_name ?? agentRaw)
    : copy.unknownAgent
  const sessionCost = session.data?.cost_summary.total.cost
  const numTraces = session.data?.num_traces ?? traces.length
  const firstStart = traces[0]?.root_span.start_time
  const failing = trace.data ? isTraceFailing(trace.data) : false

  if (session.isError) {
    return (
      <div className="flex flex-col gap-4">
        <Crumbs back={back} day={search.day} title="Trace" />
        <ErrorState
          error={session.error}
          onRetry={() => void session.refetch()}
          notFound={copy.notFoundSession}
        />
      </div>
    )
  }
  // The header is built from the session, so the whole page waits for it.
  if (session.isPending) return <PageLoader label="Loading session" />

  return (
    <div className="flex flex-col gap-4">
      <PageHeader
        breadcrumb={
          <Crumbs back={back} day={search.day} title={session.data?.title ?? sessionId} />
        }
        title={
          <span className="inline-block">
            {session.data ? (
              <>
                Session ·{' '}
                {agentRaw ? <AgentLink name={agentRaw}>{agentLabel}</AgentLink> : agentLabel}
                {firstStart ? ` · ${fmtUtcDayTime(firstStart)}` : ''} ·{' '}
                {sessionCost == null ? '—' : fmtMoney(sessionCost)} · {numTraces} trace
                {numTraces === 1 ? '' : 's'}
                {failing ? (
                  <>
                    {' '}
                    · <span className="text-destructive">✕ failed</span>
                  </>
                ) : null}
              </>
            ) : (
              'Loading session…'
            )}
          </span>
        }
        // One-line rollup of the selected trace (Extend's "4 tool calls, 11s thinking").
        description={
          trace.data ? (
            <>
              This trace:{' '}
              <span className="font-medium text-foreground tabular-nums">
                {fmtMoney(trace.data.cost_summary.total.cost)}
              </span>{' '}
              · {copy.rollup({ spans: trace.data.num_spans, ...counts })}
              {total ? ` · ${fmtLatency(total)}` : ''}
              <SessionOptimisation sessionId={sessionId} />
            </>
          ) : undefined
        }
        actions={
          <>
            <OpenChatLink sessionId={sessionId} />
            {firstStart ? (
              <Button asChild variant="outline" size="sm">
                <Link
                  to="/tokenops"
                  search={{
                    ...withoutWindow(search),
                    day: firstStart.slice(0, 10),
                    agent: agentRaw || undefined,
                    open: 'spend',
                  }}
                >
                  <Workflow className="size-3.5" aria-hidden /> Open in TokenOps
                </Link>
              </Button>
            ) : null}
            <Button variant="outline" size="sm" onClick={copyLink}>
              <Link2 className="size-3.5" aria-hidden /> Copy link
            </Button>
            <span role="status" className="text-xs text-muted-foreground">
              {copied === 'copied' ? copy.linkCopied : copied === 'failed' ? copy.copyFailed : ''}
            </span>
          </>
        }
      />

      {traces.length === 0 ? (
        <EmptyState icon={ListTree} title={copy.noTraces}>
          {copy.noTracesBody}
        </EmptyState>
      ) : (
        <>
          {traces.length > 1 ? (
            <TraceSwitcher
              traces={traces}
              traceId={traceId}
              defaultTraceId={defaultTraceId}
              reasonFailing={defaultFailing}
              onPick={(id) => {
                setSpanQuery('')
                setSearch({ trace: id, span: undefined })
              }}
            />
          ) : null}

          {trace.isError ? (
            <ErrorState
              error={trace.error}
              onRetry={() => void trace.refetch()}
              notFound={copy.traceNotFound}
            />
          ) : !trace.data ? (
            <PageLoader label="Loading trace" />
          ) : (
            <>
              <Narrative
                narrative={narrative}
                onHighlight={setHighlight}
                onPick={(ids) => {
                  const s = spans.find((x) => x.node.id === ids[0])
                  if (s) select(s)
                }}
                showDetails={showDetails}
                onToggleDetails={() => setShowDetails((v) => !v)}
                stillCollecting={trace.isFetching && !trace.isPending}
              />
              {unknownSpan ? (
                <p
                  role="status"
                  className="rounded-md border border-border bg-muted/40 p-2 text-sm"
                >
                  {copy.unknownSpan}
                </p>
              ) : null}

              <div className="grid gap-4 lg:grid-cols-[minmax(0,1fr)_22rem]">
                <section
                  aria-labelledby="spans-title"
                  className="flex min-w-0 flex-col gap-2 rounded-lg border border-border p-3"
                >
                  {/* Axiom-style card header: trace id, span count, filter, view toggle. */}
                  <div className="flex flex-wrap items-center gap-x-3 gap-y-2">
                    <h2 id="spans-title" className="text-sm font-semibold">
                      Trace{' '}
                      <span className="font-mono font-normal text-muted-foreground">
                        {traceId?.slice(0, 12)}
                      </span>
                    </h2>
                    <span role="status" className="text-xs text-muted-foreground tabular-nums">
                      {copy.spanCount(matches.length, spans.length, !!needle)}
                    </span>
                    <SearchInput
                      value={spanQuery}
                      onChange={(e) => setSpanQuery(e.target.value)}
                      placeholder={copy.filterSpans}
                      aria-label={copy.filterSpans}
                      className="ml-auto h-8 min-w-40 flex-1 sm:max-w-56"
                    />
                    <Button variant="ghost" size="sm" onClick={() => setAsTable((v) => !v)}>
                      <Table2 className="size-4" aria-hidden /> {asTable ? 'Waterfall' : 'Table'}
                    </Button>
                  </div>
                  {/* The legend explains the bars; the table names each kind in its own column. */}
                  {asTable ? null : <Legend />}
                  {asTable ? (
                    <SpanTable spans={matches} selectedId={selected?.node.id} onSelect={select} />
                  ) : (
                    <Waterfall
                      spans={spans}
                      total={total}
                      selectedId={selected?.node.id}
                      onSelect={select}
                      expanded={expanded}
                      collapsed={collapsed}
                      onToggleGroup={toggleGroup}
                      onToggleNode={toggleNode}
                      highlight={highlight}
                      dimmed={dimmed}
                    />
                  )}
                </section>
                {selected && traceId && !narrow ? (
                  // Sticky: on a long trace the panel (and its ↑/↓) stays in view while the tree scrolls.
                  <div className="sticky top-4 max-h-[calc(100dvh-2rem)] self-start overflow-y-auto">
                    <SpanPanel
                      traceId={traceId}
                      span={selected}
                      tab={panelTab}
                      onTabChange={setPanelTab}
                      parentName={parentName}
                      position={position}
                      onPrev={() => step(-1)}
                      onNext={() => step(1)}
                    />
                  </div>
                ) : null}
              </div>
              {selected && traceId && narrow ? (
                <Sheet open={sheetOpen} onOpenChange={setSheetOpen}>
                  {/* The panel has its own close button; the sheet's would be a second one. */}
                  <SheetContent
                    side="bottom"
                    showCloseButton={false}
                    className="max-h-[90vh] overflow-y-auto p-3"
                  >
                    <SheetTitle className="sr-only">Span details</SheetTitle>
                    <SpanPanel
                      traceId={traceId}
                      span={selected}
                      tab={panelTab}
                      onTabChange={setPanelTab}
                      parentName={parentName}
                      position={position}
                      onPrev={() => step(-1)}
                      onNext={() => step(1)}
                      onClose={() => setSheetOpen(false)}
                    />
                  </SheetContent>
                </Sheet>
              ) : null}
            </>
          )}
        </>
      )}
    </div>
  )
}

function Crumbs({
  back,
  day,
  title,
}: {
  back: Partial<SessionsSearch>
  day?: string
  title: string
}) {
  return (
    <Breadcrumb aria-label="Breadcrumb">
      <BreadcrumbList className="gap-1 sm:gap-1">
        <BreadcrumbItem>
          <BreadcrumbLink asChild className="underline-offset-2 hover:underline">
            <Link
              to="/sessions"
              search={{ ...back, day: undefined, sort: undefined, lane: undefined }}
            >
              Sessions
            </Link>
          </BreadcrumbLink>
        </BreadcrumbItem>
        {day ? (
          <>
            <BreadcrumbSeparator />
            <BreadcrumbItem>
              <BreadcrumbLink asChild className="underline-offset-2 hover:underline">
                <Link to="/sessions" search={back}>
                  {fmtShortDay(day)}
                </Link>
              </BreadcrumbLink>
            </BreadcrumbItem>
          </>
        ) : null}
        <BreadcrumbSeparator />
        <BreadcrumbItem className="min-w-0">
          {/* Plain text, not shadcn's BreadcrumbPage (role="link" on a non-link). */}
          <span aria-current="page" className="truncate text-foreground">
            {title}
          </span>
        </BreadcrumbItem>
      </BreadcrumbList>
    </Breadcrumb>
  )
}

function Narrative({
  narrative,
  onHighlight,
  onPick,
  showDetails,
  onToggleDetails,
  stillCollecting,
}: {
  narrative: ReturnType<typeof traceNarrative> | null
  onHighlight: (ids: ReadonlySet<string>) => void
  onPick: (ids: string[]) => void
  showDetails: boolean
  onToggleDetails: () => void
  stillCollecting: boolean
}) {
  if (!narrative) return null
  // Inline spans with button semantics: a real button element wraps like a block mid-sentence.
  const clause = (c: Clause, i: number) =>
    c.spanIds.length ? (
      <span
        key={i}
        role="button"
        tabIndex={0}
        className="rounded-sm underline decoration-muted-foreground/40 decoration-dotted underline-offset-4 outline-none hover:decoration-foreground focus-visible:ring-2 focus-visible:ring-ring"
        onMouseEnter={() => onHighlight(new Set(c.spanIds))}
        onMouseLeave={() => onHighlight(new Set())}
        onFocus={() => onHighlight(new Set(c.spanIds))}
        onBlur={() => onHighlight(new Set())}
        onClick={() => onPick(c.spanIds)}
        onKeyDown={(e) => {
          if (e.key === 'Enter' || e.key === ' ') {
            e.preventDefault()
            onPick(c.spanIds)
          }
        }}
      >
        {c.text}
      </span>
    ) : (
      <span key={i}>{c.text}</span>
    )
  return (
    <section aria-label="What happened" className="flex max-w-[70ch] flex-col gap-2">
      <p className="text-lead leading-7" aria-describedby="narrative-plain">
        {narrative.sentences.map((s) => (
          // A sentence's text is its identity: each comes from a different template.
          <span key={s.map((c) => c.text).join('')}>{s.map(clause)}. </span>
        ))}
      </p>
      <span id="narrative-plain" className="sr-only">
        {narrativeText(narrative).join(' ')}
      </span>
      {narrative.takeaway ? <p className="text-sm font-medium">{narrative.takeaway.text}</p> : null}
      {stillCollecting ? (
        <p role="status" className="text-sm text-muted-foreground">
          {copy.stillCollecting}
        </p>
      ) : null}
      {narrative.details.length ? (
        <div>
          <Button
            variant="link"
            size="sm"
            className="h-auto p-0"
            aria-expanded={showDetails}
            onClick={onToggleDetails}
          >
            {showDetails ? 'Hide details' : 'Details'}
          </Button>
          {showDetails ? (
            <ul className="mt-1 list-disc pl-5 text-sm text-muted-foreground">
              {narrative.details.map((d) => (
                <li key={d.text}>{d.text}.</li>
              ))}
            </ul>
          ) : null}
        </div>
      ) : null}
    </section>
  )
}

/** Tabs for a few traces; a compact select for many (sessions can hold dozens). */
function TraceSwitcher({
  traces,
  traceId,
  defaultTraceId,
  reasonFailing,
  onPick,
}: {
  traces: TraceEntry[]
  traceId?: string
  defaultTraceId?: string
  reasonFailing: boolean
  onPick: (id: string) => void
}) {
  const reason = (t: TraceEntry) =>
    t.trace_id === defaultTraceId ? ` · ${reasonFailing ? 'failing' : 'largest'}` : ''
  const label = (t: TraceEntry, i: number) =>
    `Trace ${i + 1} · ${fmtMoney(entryCost(t))} · ${fmtTokens(t.root_span.cumulative_token_count_total)} tokens${reason(t)}`
  if (traces.length > 6) {
    return (
      <Select value={traceId} onValueChange={onPick}>
        <SelectTrigger
          size="sm"
          className="h-9 w-fit max-w-full text-sm"
          aria-label={`Trace (${traces.length} in this session)`}
        >
          <span className="text-muted-foreground">{traces.length} traces:</span> <SelectValue />
        </SelectTrigger>
        <SelectContent>
          {traces.map((t, i) => (
            <SelectItem key={t.trace_id} value={t.trace_id}>
              {label(t, i)}
            </SelectItem>
          ))}
        </SelectContent>
      </Select>
    )
  }
  return (
    <div role="group" aria-label="Traces in this session" className="flex gap-1 overflow-x-auto">
      {traces.map((t, i) => (
        <Toggle
          key={t.trace_id}
          variant="outline"
          pressed={t.trace_id === traceId}
          onClick={() => onPick(t.trace_id)}
          className="h-auto min-h-11 shrink-0 flex-col items-start gap-0 px-3 py-1 text-left text-xs font-normal text-foreground data-[state=on]:border-foreground"
        >
          <span className="font-medium">
            Trace {i + 1}
            <span className="font-normal text-muted-foreground">{reason(t)}</span>
          </span>
          <span className="text-muted-foreground tabular-nums">
            {fmtMoney(entryCost(t))} · {fmtTokens(t.root_span.cumulative_token_count_total)} tokens
          </span>
        </Toggle>
      ))}
    </div>
  )
}
