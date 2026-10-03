/**
 * Sessions: the fleet pulse (fleet mode) and "that day's sessions" (day mode, ?day=).
 *
 *   fleet mode  window from the shared context, time sort, Load more, Live (replay/poll)
 *   day mode    scan from the day start, cost sort, Live disabled, divergence note
 *
 * Lanes (failing, slow, costly) filter the list; failing only covers status-checked rows
 * (the first 25 in the current order). Everything the page shows comes from the URL.
 */
import { useQuery } from '@tanstack/react-query'
import { m } from 'motion/react'
import { ListTree, Pause, Play, SearchX, X } from 'lucide-react'
import { useCallback, useMemo, useState, type ReactNode } from 'react'
import { PageHeader } from '@/components/shared/page-header'
import { PageLoader } from '@/components/shared/page-loader'
import { EmptyState } from '@/components/shared/state-card'
import { Alert, AlertDescription } from '@/components/ui/alert'
import { Button } from '@/components/ui/button'
import { Toggle } from '@/components/ui/toggle'
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from '@/components/ui/select'
import { compareOn, pickShared } from '@/app/shell/context'
import { copy } from '@/features/observability/copy'
import {
  computeLanes,
  isUnknownAgent,
  noTraceData,
  sessionCost,
  sortSessions,
  type Status,
} from '@/features/observability/sessions'
import { ErrorState } from '@/features/observability/StateCard'
import {
  morphId,
  SCAN_MAX_PAGES,
  SCAN_PAGES,
  STATUS_CHECK_ROWS,
  TEMPO_MAX_SEARCH_MS,
} from '@/features/observability/tuning'
import type { SessionSummary } from '@/features/observability/types'
import { useDay } from '@/features/tokenops/api'
import { TimeControl } from '@/components/shared/time-control'
import { fmtMoney, fmtShortDay } from '@/lib/format'
import { resolveWindow } from '@/features/tokenops/window'
import { meQuery } from '@/lib/api/auth'
import { env } from '@/lib/env'
import { cn } from '@/lib/utils'
import { useAgentsDirectory } from '@/features/agents/api'
import { frozenNow, tempoSafeStart, useDayScan, useFleetSessions, useSessionStatuses } from './api'
import { useLiveFeed } from './live'
import { SessionRow } from './SessionRow'
import type { SessionsSearch } from './search'

export type SetSessionsSearch = (
  patch: Partial<SessionsSearch>,
  opts?: { replace?: boolean },
) => void

const ALL = '__all'

export function SessionsPage({
  search,
  setSearch,
}: {
  search: SessionsSearch
  setSearch: SetSessionsSearch
}) {
  // "Now" is frozen per window (no ticking keys) and survives the trace round trip;
  // the live pulse brings new rows in.
  const now = useMemo(
    () => frozenNow(`${search.preset}|${search.from}|${search.to}`),
    [search.preset, search.from, search.to],
  )
  const win = useMemo(
    () => resolveWindow({ preset: search.preset, from: search.from, to: search.to }, now),
    [search.preset, search.from, search.to, now],
  )
  const day = search.day
  const dayMode = !!day
  // "Scan more" applies to the day it was clicked on; another day starts at the default cap.
  const [extendedDay, setExtendedDay] = useState<string | undefined>()
  const maxPages = day && extendedDay === day ? SCAN_MAX_PAGES : SCAN_PAGES
  const [expanded, setExpanded] = useState<string | null>(null)
  const [paused, setPaused] = useState(search.live === 'paused')

  const me = useQuery(meQuery)
  // session/list searches Tempo from start_time to the server's now: past Tempo's limit, every
  // lookup fails. 7d is sent just inside it (tempoSafeStart); longer windows go past it.
  const fleetStart = useMemo(() => tempoSafeStart(win.start, now), [win.start, now])
  const fleet = useFleetSessions(win.key, fleetStart, win.end, !dayMode)
  // The start is what was sent (day start in day mode).
  const listStart = dayMode ? new Date(`${day}T00:00:00Z`) : fleetStart
  const longSearch = now.getTime() - listStart.getTime() >= TEMPO_MAX_SEARCH_MS
  const scan = useDayScan(day, maxPages)
  const agents = useAgentsDirectory()
  const finopsDay = useDay(day, {}, dayMode)

  const base: SessionSummary[] = useMemo(
    () => (dayMode ? (scan.scan?.rows ?? []) : fleet.rows),
    [dayMode, scan.scan, fleet.rows],
  )
  const today = now.toISOString().slice(0, 10)
  // A window that ended before now is closed: nothing new can arrive in it.
  const pastWindow = !dayMode && win.end.getTime() < now.getTime()
  const live = useLiveFeed({
    base,
    mode: env.mode,
    running: !dayMode && !paused && !pastWindow,
    today,
    windowStart: fleetStart,
    windowEndMs: pastWindow ? win.end.getTime() : null,
  })
  const rowsAll = dayMode ? base : live.rows

  // Agent filter: a raw name here; a UUID carried from TokenOps maps through /api/agents.
  const agentName = search.agent ? (agents.byId.get(search.agent)?.name ?? search.agent) : undefined
  const knownAgent =
    !agentName || agents.byName.has(agentName) || rowsAll.some((r) => r.agent_id === agentName)
  const display = (s: SessionSummary) =>
    isUnknownAgent(s)
      ? copy.unknownAgent
      : (agents.byName.get(s.agent_id)?.display_name ?? s.agent_id)

  const sort = search.sort ?? (dayMode ? 'cost' : 'time')
  const filtered = useMemo(
    () => sortSessions(agentName ? rowsAll.filter((r) => r.agent_id === agentName) : rowsAll, sort),
    [rowsAll, agentName, sort],
  )
  const checkRows = useMemo(() => filtered.slice(0, STATUS_CHECK_ROWS), [filtered])
  const checked = useSessionStatuses(checkRows, now.getTime(), !dayMode && !paused && !pastWindow)
  const status = (s: SessionSummary): Status => checked.get(s.session_id) ?? 'unchecked'
  const lanes = useMemo(() => computeLanes(filtered, checked), [filtered, checked])
  const shown = filtered.filter((s) => {
    if (search.lane === 'failing' && !lanes.failing.has(s.session_id)) return false
    if (search.lane === 'slow' && !lanes.slow.has(s.session_id)) return false
    if (search.lane === 'costly' && !lanes.costly.has(s.session_id)) return false
    if (search.status && status(s) !== search.status) return false
    return true
  })

  // Day mode: an empty first page is not "no sessions" while later pages are still coming.
  const scanning =
    dayMode && !!scan.scan && !scan.scan.complete && !scan.scan.capped && !scan.isError
  const scanFailed = dayMode && scan.isError && scan.pages.length > 0
  const pending = dayMode
    ? scan.isPending || (scanning && base.length === 0)
    : fleet.isPending || fleet.seeking
  // Empty only when nothing more can be paged in: a capped scan or seek keeps its controls.
  const empty =
    rowsAll.length === 0 &&
    !scanFailed &&
    !(dayMode && scan.scan?.capped) &&
    !(!dayMode && fleet.seekCapped)
  const error = dayMode ? (scan.pages.length ? null : scan.error) : fleet.error
  const sessionSum = shown.reduce((acc, s) => acc + (sessionCost(s) ?? 0), 0)
  const dayTotal = finopsDay.data
    ? finopsDay.data.hours.reduce((acc, h) => acc + h.spend_usd, 0)
    : null
  const mixed =
    env.mode === 'live' &&
    env.partialMocks.includes('observability') !== env.partialMocks.includes('spend-calendar')
  // The trace keeps the list's context, so its breadcrumb returns to the same view. The router
  // shares `search` structurally, so an equal URL is the same object and keeps the same links.
  const linkSearch = useMemo(
    () => ({
      ...pickShared(search),
      day: search.day,
      sort: search.sort,
      lane: search.lane,
      status: search.status,
      live: search.live,
    }),
    [search],
  )
  const toggleRow = useCallback((id: string) => setExpanded((e) => (e === id ? null : id)), [])

  return (
    <div className="flex flex-col gap-4">
      <div className="sticky top-0 z-20 -mx-4 flex flex-col gap-2 border-b border-border bg-background/95 px-4 py-3 backdrop-blur">
        <PageHeader
          title={
            dayMode ? (
              <m.span layoutId={morphId(`day-header-${day}`)} className="inline-block">
                {fmtShortDay(day)} ·{' '}
                {scan.scan
                  ? `${filtered.length} session${filtered.length === 1 ? '' : 's'}`
                  : 'sessions'}
                {scan.scan
                  ? ` · ${fmtMoney(filtered.reduce((a, s) => a + (sessionCost(s) ?? 0), 0))}`
                  : ''}
              </m.span>
            ) : (
              'Sessions'
            )
          }
          actions={
            <>
              {dayMode ? (
                <Button
                  variant="outline"
                  size="sm"
                  className="h-7 gap-1 text-xs"
                  onClick={() => setSearch({ day: undefined, sort: undefined, lane: undefined })}
                >
                  <X className="size-3" aria-hidden /> {copy.clearDay}
                </Button>
              ) : (
                <TimeControl
                  preset={search.preset}
                  from={search.from}
                  to={search.to}
                  today={today}
                  onChange={(n) => setSearch({ preset: n.preset, from: n.from, to: n.to })}
                />
              )}
              {dayMode ? (
                <span className="text-xs text-muted-foreground">
                  {copy.viewingDay(fmtShortDay(day))}
                </span>
              ) : pastWindow ? (
                <span className="text-xs text-muted-foreground">{copy.pastWindow}</span>
              ) : (
                <Button
                  variant={paused ? 'default' : 'outline'}
                  size="sm"
                  onClick={() => setPaused((p) => !p)}
                  aria-pressed={!paused}
                  aria-label={paused ? copy.resumeLive : copy.pauseLive}
                >
                  {paused ? (
                    <Play className="size-3.5" aria-hidden />
                  ) : (
                    <Pause className="size-3.5" aria-hidden />
                  )}
                  {paused ? 'Live paused' : 'Live'}
                </Button>
              )}
            </>
          }
        />
        <div className="flex flex-wrap items-center gap-2 text-sm">
          <AgentFilter
            value={agentName}
            options={[...new Set(base.map((r) => r.agent_id).filter(Boolean))]
              .sort()
              .map((n) => ({ value: n, label: agents.byName.get(n)?.display_name ?? n }))}
            onChange={(v) => setSearch({ agent: v })}
          />
          {dayMode ? (
            <Select
              value={sort}
              onValueChange={(v) => setSearch({ sort: v as 'cost' | 'time' }, { replace: true })}
            >
              <SelectTrigger size="sm" className="h-8 w-auto text-xs" aria-label="Sort sessions">
                <span className="text-muted-foreground">Sorted by</span> <SelectValue />
              </SelectTrigger>
              <SelectContent>
                <SelectItem value="cost">cost</SelectItem>
                <SelectItem value="time">time</SelectItem>
              </SelectContent>
            </Select>
          ) : null}
          {!compareOn(search) ? (
            <span className="text-xs text-muted-foreground">Compare off</span>
          ) : null}
        </div>
      </div>

      {search.provider || search.model ? (
        <Notice>
          {search.provider ? copy.unappliedFilter('Provider', search.provider) : null}{' '}
          {search.model ? copy.unappliedFilter('Model', search.model) : null}
        </Notice>
      ) : null}
      {agentName && !knownAgent && !pending ? (
        <Notice tone="warning">
          {copy.unknownAgentFilter(agentName)}{' '}
          <Button
            variant="link"
            size="sm"
            className="h-auto p-0"
            onClick={() => setSearch({ agent: undefined })}
          >
            Clear agent filter
          </Button>
        </Notice>
      ) : null}
      {mixed ? (
        <Notice>
          {copy.mixedSources(env.partialMocks.includes('observability') ? 'Sessions' : 'TokenOps')}
        </Notice>
      ) : null}
      {dayMode &&
      dayTotal !== null &&
      scan.scan?.complete &&
      !agentName &&
      !mixed &&
      Math.abs(dayTotal - sessionSum) > 0.005 &&
      !search.lane &&
      !search.status ? (
        <Notice>
          {copy.divergence(
            fmtMoney(sessionSum),
            fmtMoney(dayTotal),
            me.data ? !me.data.is_superuser : false,
          )}
        </Notice>
      ) : null}

      {error ? (
        <ErrorState
          error={error}
          onRetry={() => void (dayMode ? scan.refetch() : fleet.refetch())}
        />
      ) : pending ? (
        <>
          {scanning ? (
            <p role="status" className="text-sm text-muted-foreground">
              {copy.scanning(scan.pages.length + 1, maxPages)}
            </p>
          ) : null}
          <PageLoader label="Loading sessions" />
        </>
      ) : empty ? (
        <EmptyState
          icon={ListTree}
          title={copy.emptyTitle}
          action={
            !dayMode && search.preset !== '30d' ? (
              <Button
                size="sm"
                variant="outline"
                onClick={() => setSearch({ preset: '30d', from: undefined, to: undefined })}
              >
                Show the last 30 days
              </Button>
            ) : undefined
          }
        >
          {env.mode === 'live' ? copy.sparseLive : copy.emptyBody}
        </EmptyState>
      ) : (
        <>
          {/* Rows come from the DB even when their trace lookups failed: list them (each opens its
              own page, which looks the trace up directly) and say why the details are missing. */}
          {base.length > 0 && noTraceData(base) ? (
            longSearch ? (
              <Notice tone="warning">
                {copy.noTraceDataLongWindow} {copy.noTraceDataLongWindowFix}
              </Notice>
            ) : (
              <Notice tone="warning">
                {copy.noTraceData} {copy.noTraceDataFix}
              </Notice>
            )
          ) : null}
          <Lanes
            lanes={lanes}
            lane={search.lane}
            checkRows={checkRows.length}
            day={day}
            onLane={(l) =>
              setSearch({ lane: search.lane === l ? undefined : l }, { replace: true })
            }
          />

          {scanning ? (
            <p role="status" className="text-sm text-muted-foreground">
              {copy.scanning(scan.pages.length + 1, maxPages)}
            </p>
          ) : null}
          {scanFailed ? (
            <Notice tone="warning">
              {copy.scanPartial(scan.scan?.scanned ?? 0, scan.pages.length + 1, maxPages)}{' '}
              <Button
                variant="link"
                size="sm"
                className="h-auto p-0"
                onClick={() => void scan.fetchNextPage()}
              >
                Retry
              </Button>
            </Notice>
          ) : null}
          {dayMode && scan.scan?.capped ? (
            <Notice>
              {scan.scan.missedDay
                ? copy.scanMissedDay(fmtShortDay(day))
                : copy.scanCapped(fmtShortDay(day), scan.scan.scanned)}{' '}
              {maxPages < SCAN_MAX_PAGES ? (
                <Button
                  variant="link"
                  size="sm"
                  className="h-auto p-0"
                  onClick={() => setExtendedDay(day)}
                >
                  {copy.scanMore}
                </Button>
              ) : null}
            </Notice>
          ) : null}

          {!dayMode && live.pollFailed ? (
            <Notice tone="warning">{copy.livePollFailed}</Notice>
          ) : null}
          {!dayMode && fleet.seekCapped ? (
            <Notice>{copy.fleetSeekCapped(fleet.data?.pages.length ?? 0)}</Notice>
          ) : null}
          {!dayMode && live.queued ? (
            <div className="sticky top-28 z-10 flex justify-center">
              <Button size="sm" className="rounded-full shadow-sm" onClick={live.flush}>
                {copy.newSessions(live.queued)} ↑
              </Button>
            </div>
          ) : null}
          <span className="sr-only" aria-live="polite">
            {!dayMode && live.fresh.size ? copy.newSessions(live.fresh.size) : ''}
          </span>

          {shown.length ? (
            <div className="@container rounded-lg border border-border">
              <div className="hidden grid-cols-[5.5rem_10rem_minmax(0,1fr)_5rem_4.5rem_8.5rem_2.75rem] gap-x-3 border-b border-border px-2 py-2 text-xs text-muted-foreground @[672px]:grid">
                <span>{dayMode ? 'Time (UTC)' : 'Started (UTC)'}</span>
                <span>Agent</span>
                <span>First input</span>
                <span className="text-right">Cost</span>
                <span className="text-right">Duration</span>
                <span>Status</span>
                <span />
              </div>
              <ul aria-label="Sessions" {...live.listProps}>
                {shown.map((s) => (
                  <SessionRow
                    key={s.session_id}
                    s={s}
                    agentLabel={display(s)}
                    rawName={s.agent_id}
                    status={status(s)}
                    slow={lanes.ranked && lanes.slow.has(s.session_id)}
                    costly={lanes.ranked && lanes.costly.has(s.session_id)}
                    withDay={!dayMode}
                    expanded={expanded === s.session_id}
                    onToggle={toggleRow}
                    linkSearch={linkSearch}
                    fresh={live.fresh.has(s.session_id)}
                  />
                ))}
              </ul>
            </div>
          ) : (
            <EmptyState
              icon={SearchX}
              title={copy.noMatchTitle}
              action={
                <Button
                  size="sm"
                  variant="outline"
                  onClick={() =>
                    setSearch({ agent: undefined, lane: undefined, status: undefined })
                  }
                >
                  {copy.clearFilters}
                </Button>
              }
            >
              {copy.noMatchBody}
            </EmptyState>
          )}
          <div className="flex flex-wrap items-center justify-between gap-2 text-xs text-muted-foreground">
            <span>{copy.statusCaption(lanes.checked, filtered.length)}</span>
            {!dayMode ? (
              fleet.hasNextPage ? (
                <Button
                  size="sm"
                  variant="outline"
                  disabled={fleet.isFetchingNextPage}
                  onClick={() => void fleet.fetchNextPage()}
                >
                  {fleet.isFetchingNextPage ? 'Loading…' : copy.loadMore}
                </Button>
              ) : fleet.isFetchNextPageError ? (
                <Button size="sm" variant="outline" onClick={() => void fleet.fetchNextPage()}>
                  Retry loading more
                </Button>
              ) : (
                <span>{copy.endOfRange}</span>
              )
            ) : null}
          </div>
        </>
      )}
    </div>
  )
}

function Lanes({
  lanes,
  lane,
  checkRows,
  day,
  onLane,
}: {
  lanes: ReturnType<typeof computeLanes>
  lane: SessionsSearch['lane']
  checkRows: number
  day?: string
  onLane: (l: 'failing' | 'slow' | 'costly') => void
}) {
  if (!lanes.ranked) return <p className="text-sm text-muted-foreground">{copy.notEnoughToRank}</p>
  const checking = lanes.settled < checkRows
  const none = !checking && !lanes.failing.size && !lanes.slow.size && !lanes.costly.size
  const chip = (id: 'failing' | 'slow' | 'costly', label: string, tone: string, title?: string) => (
    <Toggle
      variant="outline"
      pressed={lane === id}
      onClick={() => onLane(id)}
      title={title}
      className="rounded-full px-3 font-normal data-[state=on]:border-foreground data-[state=on]:bg-foreground data-[state=on]:text-background"
    >
      <span className={cn('size-2 rounded-full', tone)} aria-hidden />
      {label}
    </Toggle>
  )
  return (
    <div className="flex flex-wrap items-center gap-2" role="group" aria-label="Needs attention">
      <span className="text-sm font-medium">Needs attention</span>
      {none ? (
        <span className="text-sm text-muted-foreground">{copy.nothingNeedsAttention}</span>
      ) : null}
      {chip(
        'failing',
        `${copy.failingChip(lanes.failing.size, lanes.checked)}${checking ? ' …' : ''}`,
        'bg-destructive',
        copy.failingTooltip,
      )}
      {chip('slow', copy.slowChip(lanes.slow.size), 'bg-warning')}
      {chip('costly', copy.costlyChip(lanes.costly.size), 'bg-chart-4-edge')}
      <span className="text-xs text-muted-foreground">
        {copy.laneScope(day ? fmtShortDay(day) : undefined)}
      </span>
    </div>
  )
}

function AgentFilter({
  value,
  options,
  onChange,
}: {
  value?: string
  options: { value: string; label: string }[]
  onChange: (v: string | undefined) => void
}) {
  const known = !value || options.some((o) => o.value === value)
  return (
    <div className="flex items-center">
      <Select value={value ?? ALL} onValueChange={(v) => onChange(v === ALL ? undefined : v)}>
        <SelectTrigger size="sm" className="h-8 max-w-56 text-xs" aria-label="Filter by agent">
          <span className="text-muted-foreground">Agent:</span> <SelectValue />
        </SelectTrigger>
        <SelectContent>
          <SelectItem value={ALL}>All</SelectItem>
          {!known && value ? <SelectItem value={value}>{value}</SelectItem> : null}
          {options.map((o) => (
            <SelectItem key={o.value} value={o.value}>
              {o.label}
            </SelectItem>
          ))}
        </SelectContent>
      </Select>
      {value ? (
        <Button
          variant="ghost"
          size="icon"
          className="size-10 md:size-7"
          onClick={() => onChange(undefined)}
          aria-label="Remove agent filter"
        >
          <X className="size-3.5" aria-hidden />
        </Button>
      ) : null}
    </div>
  )
}

function Notice({ tone = 'info', children }: { tone?: 'info' | 'warning'; children: ReactNode }) {
  return (
    <Alert
      role={tone === 'warning' ? 'alert' : 'status'}
      className={cn(
        'rounded-md p-3',
        tone === 'warning' ? 'border-warning/40 bg-warning/5' : 'bg-muted/40',
      )}
    >
      <AlertDescription className="text-foreground">
        <p>{children}</p>
      </AlertDescription>
    </Alert>
  )
}
