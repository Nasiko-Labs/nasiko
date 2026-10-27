/**
 * Your flows (plans/feat-flows.md §3, §2a; T5): the window in the shell's top bar, search and the Kind and Status
 * dropdowns at the right of the title, requests per day by status and the Duration panel over the table.
 *
 * The server filters only status and search (`q`), newest first, so the page reads pages back to the window start
 * (A2, cap 500) and applies the window and Kind itself; the charts and the table always share the same rows, and the
 * charts say "From your latest N flows" until the server summary (FL-6) replaces them. Search, Kind and Status replace
 * history; the window pushes it, as on Sessions.
 */
import { Link } from '@tanstack/react-router'
import { ArrowRight, Network, Workflow } from 'lucide-react'
import { useQueryClient } from '@tanstack/react-query'
import { useEffect, useRef, useState } from 'react'
import { PageHeader } from '@nasiko/ui/composites/page-header'
import { PageLoader } from '@nasiko/ui/composites/page-loader'
import { Panel, PanelError } from '@nasiko/ui/composites/panel'
import { SearchInput } from '@nasiko/ui/composites/search-input'
import { EmptyState, StateCard } from '@nasiko/ui/composites/state-card'
import { CustomRange, TimePresets } from '@nasiko/ui/composites/time-control'
import { TopBar } from '@nasiko/app-core/shell/TopBar'
import { Badge } from '@nasiko/ui/primitives/badge'
import { Button, asButton } from '@nasiko/ui/primitives/button'
import {
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeader,
  TableRow,
} from '@nasiko/ui/primitives/table'
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from '@nasiko/ui/primitives/select'
import { relTime } from '@nasiko/features/agents/format'
import { resolveWindow } from '@nasiko/features/tokenops/window'
import { isEndpointAbsent } from '@nasiko/data-access/detect'
import { useSiblingState } from '@nasiko/app-core/siblings'
import { useMediaQuery } from '@nasiko/ui/lib/useMediaQuery'
import { useFrozenNow, useReturnTick } from '@nasiko/app-core/useReturnTick'
import { flowKeys, useFlowsList } from './api'
import { DurationPanel } from './components/DurationPanel'
import { RequestsChart } from './components/RequestsChart'
import { copy, own } from './copy'
import { flowKind, type FlowKind } from './kind'
import { buckets, durationOf, filterRows, percentiles } from './list'
import { rememberListSearch } from './listSearch'
import { LIST_CAP, LIVE_EDGE_MS, SEARCH_DEBOUNCE_MS } from './tuning'
import { fmtDuration } from './precision'
import {
  KIND_FILTERS,
  STATUS_FILTERS,
  type FlowsSearch,
  type KindFilter,
  type StatusFilter,
} from './search'
import type { Flow } from './types'

const STATUS_BADGE: Record<string, 'success' | 'info' | 'warning' | 'destructive' | 'muted'> = {
  completed: 'success',
  running: 'info',
  paused: 'warning',
  failed: 'destructive',
}
const KIND_ICON: Record<FlowKind, typeof Network> = {
  orchestrated: Network,
  direct: ArrowRight,
  workflow: Workflow,
}

export function FlowsPage({
  search,
  setSearch,
}: {
  search: FlowsSearch
  setSearch: (patch: Partial<FlowsSearch>, opts?: { replace?: boolean }) => void
}) {
  // "Now" is frozen per window, as on Sessions and TokenOps; coming back after a while moves it and re-reads the list
  // (a fixed custom window keeps its key, so the read is explicit).
  const returnTick = useReturnTick()
  const queryClient = useQueryClient()
  const nowDate = useFrozenNow(search.preset, search.from, search.to, returnTick)
  const win = resolveWindow({ preset: search.preset, from: search.from, to: search.to }, nowDate)
  // A window whose start moves with "now" (24h, 7d, 30d) gets a new key, so a new read. One whose start stays (this
  // month, last month, custom) keeps its key: on a return tick that didn't move it, re-read explicitly. A window
  // change alone never invalidates (its new key is already reading).
  const sinceMs = win.start.getTime()
  const seenStart = useRef(sinceMs)
  const seenTick = useRef(returnTick)
  useEffect(() => {
    if (returnTick !== seenTick.current && sinceMs === seenStart.current)
      void queryClient.invalidateQueries({ queryKey: [...flowKeys.all, 'list'] })
    seenStart.current = sinceMs
    seenTick.current = returnTick
  }, [returnTick, sinceMs, queryClient])
  const today = nowDate.toISOString().slice(0, 10)
  const status = search.status === 'all' ? undefined : search.status
  const list = useFlowsList({ status, q: search.q, sinceMs })

  // "← Flows" on a flow page comes back to the list as it is now (F20).
  useEffect(() => rememberListSearch(search), [search])

  // The search box commits after a pause; Clear filters or Back (a URL change) resets it. Its own commit doesn't: the
  // trimmed value would eat a typed trailing space, and a key typed before the URL caught up would be lost.
  const [draft, setDraft] = useState(search.q ?? '')
  const [shownQ, setShownQ] = useState(search.q)
  // The value the box last committed, until the URL shows it (null: none in flight).
  const [committed, setCommitted] = useState<string | undefined | null>(null)
  if (shownQ !== search.q) {
    setShownQ(search.q)
    if (committed !== null && committed === search.q) setCommitted(null)
    else {
      if (committed !== null) setCommitted(null)
      if ((draft.trim() || undefined) !== search.q) setDraft(search.q ?? '')
    }
  }
  useEffect(() => {
    const next = draft.trim() || undefined
    if (next === search.q) return
    const t = setTimeout(() => {
      setCommitted(next)
      setSearch({ q: next }, { replace: true })
    }, SEARCH_DEBOUNCE_MS)
    return () => clearTimeout(t)
  }, [draft, search.q, setSearch])

  const time = {
    preset: search.preset,
    from: search.from,
    to: search.to,
    today,
    onChange: (n: Pick<FlowsSearch, 'preset' | 'from' | 'to'>) =>
      setSearch({ preset: n.preset, from: n.from, to: n.to }),
  }
  // Below md the bar has room for the presets only: the custom dates, search and filters sit under the title.
  const isMd = useMediaQuery('(min-width: 48rem)')
  // The search and filters stay mounted while results load, so typing keeps its focus.
  const searchBox = (
    <SearchInput
      className="max-w-none md:h-8 md:w-64"
      aria-label={copy.list.search}
      placeholder={copy.list.search}
      value={draft}
      onChange={(e) => setDraft(e.target.value)}
    />
  )
  const filters = (
    <>
      <Filter
        label={copy.list.kind}
        value={search.kind}
        options={KIND_FILTERS}
        names={copy.list.kinds}
        onChange={(kind) => setSearch({ kind: kind as KindFilter }, { replace: true })}
      />
      <Filter
        label={copy.list.statusLabel}
        value={search.status}
        options={STATUS_FILTERS}
        names={copy.list.statuses}
        onChange={(s) => setSearch({ status: s as StatusFilter }, { replace: true })}
      />
    </>
  )
  // Left: what the page is. Right: what narrows it, under the window in the top bar.
  const header = (
    <>
      <TopBar
        actions={
          <>
            <TimePresets {...time} />
            {isMd ? <CustomRange {...time} /> : null}
          </>
        }
        pinActions
      />
      <PageHeader
        title={copy.list.title}
        description={copy.list.description}
        actions={
          isMd ? (
            <>
              {filters}
              {searchBox}
            </>
          ) : null
        }
        className="mb-2"
      />
      {isMd ? null : (
        <div className="flex flex-wrap items-center gap-2">
          <CustomRange {...time} />
          <div className="w-full">{searchBox}</div>
          {filters}
        </div>
      )}
    </>
  )

  if (list.isPending)
    return (
      <Frame>
        {header}
        <PageLoader label={copy.list.loading} inline className="h-80" />
      </Frame>
    )
  // A failed re-read keeps the last good list (TanStack keeps `data`); only a first read that failed replaces it.
  if (list.isError && !list.data)
    return (
      <Frame>
        {header}
        {isEndpointAbsent(list.error) ? (
          <StateCard tone="warning" title={copy.list.absent}>
            {copy.list.absentBody}
          </StateCard>
        ) : (
          <PanelError
            error={list.error}
            onRetry={() => void list.refetch()}
            what={copy.list.what}
          />
        )}
      </Frame>
    )

  const { rows: read, capped, error: partial } = list.data
  // A window that ends now has no upper bound: its end is rounded to the minute, and a flow that started since then
  // belongs in it. A past window (custom, last month) stops at its end.
  const nowMs = nowDate.getTime()
  const end =
    win.end.getTime() >= nowMs - LIVE_EDGE_MS ? Number.POSITIVE_INFINITY : win.end.getTime()
  const rows = filterRows(read, win.start.getTime(), end, search.kind)
  // The read starts at the newest flow: past the cap with every row still newer than a past window's end, nothing in
  // the window was reached, which no filter can fix.
  const oldestRead = read.reduce((m, f) => Math.min(m, Date.parse(f.created_at) || m), Infinity)
  const outOfReach = capped && end !== Number.POSITIVE_INFINITY && oldestRead > end
  const filtered = !!search.q || search.kind !== 'all' || search.status !== 'all'
  const hourly = search.preset === '24h'
  // The chart covers the same rows as the table: an open window reaches the newest row read, even one from after the
  // frozen "now" (a refetch).
  const newest = rows.reduce((m, f) => Math.max(m, Date.parse(f.created_at) || 0), 0)
  const chartEnd = end === Number.POSITIVE_INFINITY ? Math.max(nowMs, newest) + 1 : end
  const chartRows = buckets(rows, win.start.getTime(), chartEnd, hourly)
  const stats = percentiles(rows)
  const sample = copy.list.sample(rows.length)

  return (
    <Frame>
      {header}

      {list.isError ? (
        <StateCard
          tone="warning"
          title={copy.list.stale}
          action={
            <Button size="sm" variant="outline" onClick={() => void list.refetch()}>
              {copy.list.retry}
            </Button>
          }
        />
      ) : null}

      {partial ? (
        <StateCard
          tone="warning"
          title={copy.list.readFailed}
          action={
            <Button size="sm" variant="outline" onClick={() => void list.refetch()}>
              {copy.list.retry}
            </Button>
          }
        />
      ) : null}

      {outOfReach ? (
        <StateCard tone="warning" title={copy.list.outOfReach(LIST_CAP)}>
          {copy.list.outOfReachBody}
        </StateCard>
      ) : !read.length && !filtered ? (
        <EmptyState
          title={copy.list.firstRun}
          action={
            <Link to="/chat" {...asButton({ size: 'sm' })}>
              {copy.list.openChat}
            </Link>
          }
        >
          {copy.list.firstRunBody}
        </EmptyState>
      ) : (
        <>
          <div data-tour="flows-charts" className="grid gap-4 lg:grid-cols-[2fr_1fr]">
            <Panel title={copy.list.requests} labelledBy="flows-requests" subtitle={sample}>
              <RequestsChart rows={chartRows} />
            </Panel>
            <Panel
              title={copy.list.duration}
              labelledBy="flows-duration"
              subtitle={`${sample}. ${copy.list.durationSub}`}
            >
              <DurationPanel stats={stats} />
            </Panel>
          </div>
          {capped ? (
            <p className="text-xs text-muted-foreground">{copy.list.capped(LIST_CAP)}</p>
          ) : null}
          {rows.length ? (
            <FlowsTable rows={rows} now={nowMs} />
          ) : (
            <EmptyState
              title={copy.list.filteredEmpty(
                search.status === 'all' ? null : copy.list.statuses[search.status],
                search.q ?? null,
              )}
              action={
                <Button
                  size="sm"
                  variant="outline"
                  onClick={() =>
                    setSearch({ q: undefined, kind: 'all', status: 'all' }, { replace: true })
                  }
                >
                  {copy.list.clear}
                </Button>
              }
            />
          )}
        </>
      )}
    </Frame>
  )
}

/** A filter as a Select whose box names it ("Kind: All"): two boxes that both read "All" wouldn't say which is which. */
function Filter<T extends string>({
  label,
  value,
  options,
  names,
  onChange,
}: {
  label: string
  value: T
  options: readonly T[]
  names: Record<T, string>
  onChange: (v: T) => void
}) {
  return (
    <Select value={value} onValueChange={(v) => onChange(v as T)}>
      <SelectTrigger
        size="sm"
        aria-label={label}
        className="h-8 text-xs max-md:flex-1 pointer-coarse:min-h-11"
      >
        {/* One group, so a full-width box on phones keeps the value beside its label, not at the far end. */}
        <span className="flex min-w-0 items-center gap-1">
          <span className="text-muted-foreground">{label}:</span>
          <SelectValue />
        </span>
      </SelectTrigger>
      <SelectContent>
        {options.map((o) => (
          <SelectItem key={o} value={o}>
            {names[o]}
          </SelectItem>
        ))}
      </SelectContent>
    </Select>
  )
}

function FlowsTable({ rows, now }: { rows: Flow[]; now: number }) {
  // Previous/Next on the flow page walks these rows as drawn when one is clicked.
  const linkState = useSiblingState(rows.map((f) => f.flow_id))
  return (
    <Table data-tour="flows-table" aria-label={copy.list.tableLabel}>
      <TableHeader>
        <TableRow>
          <TableHead>{copy.list.cols.question}</TableHead>
          <TableHead className="max-sm:hidden">{copy.list.cols.agent}</TableHead>
          <TableHead className="max-sm:hidden">{copy.list.cols.status}</TableHead>
          <TableHead className="text-right max-sm:hidden">{copy.list.cols.duration}</TableHead>
          <TableHead className="text-right max-sm:hidden">{copy.list.cols.started}</TableHead>
        </TableRow>
      </TableHeader>
      <TableBody>
        {rows.map((f) => {
          const kind = flowKind(f)
          const Icon = KIND_ICON[kind]
          const root = f.root_agent_name ?? copy.panel.unknownAgent
          const d = durationOf(f)
          return (
            <TableRow key={f.flow_id}>
              <TableCell className="max-w-0 min-w-56 max-sm:min-w-0">
                <span className="flex min-w-0 items-center gap-2">
                  <Icon className="size-4 shrink-0 text-muted-foreground" aria-hidden />
                  <span className="sr-only">{copy.list.kindIcon[kind]}: </span>
                  <Link
                    to="/flows/$flowId"
                    params={{ flowId: f.flow_id }}
                    state={linkState}
                    className="truncate font-medium hover:underline"
                  >
                    {f.title || copy.callTo(root)}
                  </Link>
                </span>
                {/* Phones (F24): status and duration on a second line, the other columns hidden. */}
                <span className="mt-1 flex items-center gap-2 pl-6 text-xs text-muted-foreground sm:hidden">
                  <Badge variant={own(STATUS_BADGE, f.status) ?? 'muted'}>
                    {own(copy.list.statuses, f.status) ?? f.status}
                  </Badge>
                  {d === null ? null : <span className="tabular-nums">{fmtDuration(d)}</span>}
                  <span>{relTime(f.created_at, now)}</span>
                </span>
              </TableCell>
              <TableCell className="text-muted-foreground max-sm:hidden">
                {kind === 'orchestrated' ? copy.panel.orchestrator : root}
              </TableCell>
              <TableCell className="max-sm:hidden">
                <Badge variant={own(STATUS_BADGE, f.status) ?? 'muted'}>
                  {own(copy.list.statuses, f.status) ?? f.status}
                </Badge>
              </TableCell>
              <TableCell className="text-right tabular-nums max-sm:hidden">
                {d === null ? '—' : fmtDuration(d)}
              </TableCell>
              <TableCell className="text-right whitespace-nowrap text-muted-foreground max-sm:hidden">
                {relTime(f.created_at, now)}
              </TableCell>
            </TableRow>
          )
        })}
      </TableBody>
    </Table>
  )
}

function Frame({ children }: { children: React.ReactNode }) {
  return <div className="mx-auto flex w-full max-w-page flex-col gap-4">{children}</div>
}
