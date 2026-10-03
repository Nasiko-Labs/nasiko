/**
 * TokenOps, question-led (plans/feat-tokenops-page.md).
 *
 *   F1 hero "This month" (always MTD) ─ click day ─▶ ?day=
 *   ── Window divider ──
 *   KPI strip (4 + More)
 *   F2 timeline ─ click bucket ─▶ ?day=  ─▶ DayPanel ─ agent ─▶ ?agent=  │ View traces ─▶ ?traces=true
 *   F3 attribution  |  F5 cost × p95          F3 row "Traces" ─▶ ?agent=<id>&traces=true
 *   F4 traces drawer (sheet)
 *
 * All state lives in the URL. Drill-downs push history (Back unwinds them); sort,
 * search and the KPI expander replace it (A5).
 */
import { useQuery, useQueryClient } from '@tanstack/react-query'
import { Link, useRouter } from '@tanstack/react-router'
import { AlertTriangle, DollarSign, Info, X } from 'lucide-react'
import { Fragment, useEffect, useMemo, useRef, useState } from 'react'
import { Alert, AlertDescription } from '@/components/ui/alert'
import {
  Breadcrumb,
  BreadcrumbItem,
  BreadcrumbLink,
  BreadcrumbList,
  BreadcrumbPage,
  BreadcrumbSeparator,
} from '@/components/ui/breadcrumb'
import { Button } from '@/components/ui/button'
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from '@/components/ui/select'
import { ToggleGroup, ToggleGroupItem } from '@/components/ui/toggle-group'
import { meQuery } from '@/lib/api/auth'
import { ApiError } from '@/lib/api/client'
import { env } from '@/lib/env'
import { fmtLocalTime, fmtPct, fmtShortDay } from '@/lib/format'
import { prefersReducedMotion, useMediaQuery } from '@/lib/useMediaQuery'
import { useFrozenNow, useReturnTick } from '@/lib/useReturnTick'
import {
  useDashboard,
  useCalendar,
  useDay,
  useProviders,
  useSavings,
  useTimeseries,
  useTopTraces,
  type Filters,
} from './api'
import { compareOn, withoutWindow } from '@/app/shell/context'
import { tokenopsNarrative } from '@/features/narrative/tokenops'
import { buildAttribution } from './attribution'
import { summarizeMonth } from './forecast'
import {
  DISCLOSURES,
  openSet,
  toggleOpen,
  type Disclosure as DisclosureId,
  type TokenopsSearch,
} from './search'
import { toTimeline, zeroFillTimeline } from './series'
import { DAY_MS, dayWindow, monthKey, resolveWindow, utcMonthStart } from './window'
import { AttributionTable } from './components/AttributionTable'
import { CostPerformance } from './components/CostPerformance'
import { DayPanel } from './components/DayPanel'
import { Disclosure } from '@/components/shared/disclosure'
import { KpiStrip } from './components/KpiStrip'
import { MonthHero } from './components/MonthHero'
import { OptimisationPanel } from './components/OptimisationPanel'
import { SavingsHighlight } from './components/SavingsHighlight'
import { summarizeOptimisation } from './optimisation'
import { PageHeader } from '@/components/shared/page-header'
import { PanelError } from '@/components/shared/panel'
import { PageLoader } from '@/components/shared/page-loader'
import { EmptyState, StateCard } from '@/components/shared/state-card'
import { SpendTimeline } from './components/SpendTimeline'
import { SummaryHero } from './components/SummaryHero'
import { TimeControl } from '@/components/shared/time-control'
import { TracesDrawer } from './components/TracesDrawer'

export type SetSearch = (patch: Partial<TokenopsSearch>, opts?: { replace?: boolean }) => void

const ALL = '__all'

/** `null` reduction means there was no baseline to compare against, not a zero saving. */
const showOptPct = (v: number | null) => (v == null ? '—' : fmtPct(v))

export function TokenopsPage({
  search,
  setSearch,
}: {
  search: TokenopsSearch
  setSearch: SetSearch
}) {
  // "Now" is fixed, not ticked: query keys stay stable and nothing polls (plan: no polling).
  // It moves when the user changes the window, or comes back to the page after a while
  // (useReturnTick), so the window, the hero month and every query move together.
  // Queries don't refetch on focus (queryClient.ts): a refetch against a frozen window
  // would mix fresh server-side `range` data with stale client bounds.
  const returnTick = useReturnTick()
  const queryClient = useQueryClient()
  const router = useRouter()
  // Window-keyed queries get new keys when "now" moves and fetch on their own; month- and
  // date-keyed ones (calendar, day) keep theirs, so refresh just those on a return.
  useEffect(() => {
    if (returnTick > 0)
      void queryClient.invalidateQueries({
        predicate: (q) =>
          q.queryKey[0] === 'tokenops' && (q.queryKey[1] === 'calendar' || q.queryKey[1] === 'day'),
      })
  }, [returnTick, queryClient])
  const now = useFrozenNow(search.preset, search.from, search.to, returnTick)
  const me = useQuery(meQuery)
  const ready = me.isSuccess
  const win = useMemo(
    () => resolveWindow({ preset: search.preset, from: search.from, to: search.to }, now),
    [search.preset, search.from, search.to, now],
  )
  // Traces opened with a day selected are that UTC day's traces, matching the breadcrumb.
  const tracesWin = useMemo(
    () => (search.day ? dayWindow(search.day, now) : win),
    [search.day, now, win],
  )
  const filters: Filters = useMemo(
    () => ({ agent: search.agent, provider: search.provider, model: search.model }),
    [search.agent, search.provider, search.model],
  )

  const dash = useDashboard(win, filters, search.view, 'current', ready)
  const prevDash = useDashboard(win, filters, search.view, 'previous', ready)
  const timeseries = useTimeseries(win, filters, ready)
  const savings = useSavings(win, filters, 'agent', ready)
  const optimisation = useMemo(
    () => (savings.data ? summarizeOptimisation(savings.data) : undefined),
    [savings.data],
  )
  const thisMonth = utcMonthStart(now)
  const lastMonth = utcMonthStart(now, -1)
  const calThis = useCalendar(monthKey(thisMonth), filters, ready)
  const calLast = useCalendar(monthKey(lastMonth), filters, ready)
  const day = useDay(search.day, filters, ready)
  const providers = useProviders(ready)
  const traces = useTopTraces(tracesWin, filters, ready && !!search.traces)

  // Agent filter options: the unfiltered dashboard (same cache entry when no filter is set).
  const narrowed = !!(search.agent || search.provider || search.model || search.view !== 'agent')
  const agentList = useDashboard(win, {}, 'agent', 'current', ready && narrowed)
  const agentOptions = ((narrowed ? agentList.data : dash.data)?.agents ?? []).map((a) => ({
    id: a.agent_id,
    name: a.agent_name,
  }))

  // Derived data is memoised on stable keys so typing in the table search doesn't rebuild every chart.
  // Δ is only trusted when both windows are fresh answers for the current key: a
  // keepPreviousData placeholder belongs to the old filter/window (A14).
  const compare = compareOn(search)
  const prevUnavailable =
    !compare || prevDash.isError || prevDash.isPlaceholderData || dash.isPlaceholderData
  const attrRows = useMemo(
    () =>
      dash.data
        ? buildAttribution(
            dash.data.attributions,
            prevUnavailable ? undefined : prevDash.data?.attributions,
          )
        : undefined,
    [dash.data, prevDash.data, prevUnavailable],
  )
  const timeline = useMemo(() => {
    if (!timeseries.data) return []
    // Placeholder points belong to the previous window: show them as-is, never stretched over the new bounds.
    return timeseries.isPlaceholderData
      ? toTimeline(timeseries.data.points, timeseries.data.bucket)
      : zeroFillTimeline(timeseries.data.points, timeseries.data.bucket, win.start, win.end)
  }, [timeseries.data, timeseries.isPlaceholderData, win])
  // A placeholder calendar from another month (e.g. just after a UTC month rollover) is
  // treated as loading rather than summarised against this month. A same-month placeholder
  // (a filter change) stays visible while the new answer loads.
  const otherMonth = (q: typeof calThis, month: string) =>
    q.isPlaceholderData && (q.data?.days.some((d) => !d.date.startsWith(month)) ?? false)
  const calPending =
    calThis.isPending ||
    calLast.isPending ||
    otherMonth(calThis, monthKey(thisMonth)) ||
    otherMonth(calLast, monthKey(lastMonth))
  const summary = useMemo(
    () =>
      !calPending && calThis.data && calLast.data
        ? summarizeMonth(calThis.data.days, calLast.data.days, now)
        : null,
    [calPending, calThis.data, calLast.data, now],
  )
  // One page loader until the first paint's reads settle (data or error: a failed read renders its panel's error), in
  // place of the panels' skeletons. Latched: a later window or filter change keeps the panels' own loading states.
  const coldPending =
    !me.isSuccess ||
    dash.isPending ||
    timeseries.isPending ||
    calThis.isPending ||
    calLast.isPending
  const [painted, setPainted] = useState(false)
  if (!painted && !coldPending) setPainted(true)
  const showLoader = !painted && coldPending

  // Executive summary (plan: TokenOps summary first). Day buckets feed the spike clause.
  // Placeholder data (keepPreviousData) belongs to the previous filters: summarising it under
  // the new window or agent label would state the wrong total, so the hero waits instead.
  const summaryStale = dash.isPlaceholderData || timeseries.isPlaceholderData
  const dashData = summaryStale ? undefined : dash.data
  const bucket = timeseries.data?.bucket
  // Read outside the memo: the compiler takes any `.current` inside one for a ref.
  const { current: spendNow, previous: spendBefore } = dashData?.kpis.total_spend ?? {}
  const narrative = useMemo(() => {
    if (!dashData) return null
    const days =
      bucket === 'day' ? timeline.map((p) => ({ date: p.iso.slice(0, 10), spend: p.spend })) : []
    return tokenopsNarrative({
      windowLabel: win.label,
      total: spendNow ?? 0,
      previous: prevUnavailable ? undefined : spendBefore,
      unpriced: dashData.summary.unpriced_calls > 0,
      rows:
        search.view === 'agent'
          ? (attrRows ?? []).map((r) => ({ name: r.name, sharePct: r.sharePct }))
          : [],
      agentLabel: search.agent
        ? (dashData.agents.find((a) => a.agent_id === search.agent)?.agent_name ?? search.agent)
        : undefined,
      days,
    })
  }, [
    dashData,
    spendNow,
    spendBefore,
    bucket,
    timeline,
    win.label,
    prevUnavailable,
    search.view,
    search.agent,
    attrRows,
  ])

  // Disclosures, one open at a time: Spend over time starts open, the rest collapsed; a drill-down
  // or table state in the URL opens its own. This month keeps Spend over time open beside it,
  // because its calendar opens the day panel there.
  // A disclosure forced open by URL state can still be collapsed: `closed` remembers that locally.
  const open = openSet(search.open)
  // The collapse is remembered against the URL state that forced it open, so a new drill-down
  // (another day, a new search) opens the disclosure again. Closing the day panel doesn't.
  const forcedBy = (id: DisclosureId) =>
    id === 'spend'
      ? (search.day ?? '')
      : id === 'drivers'
        ? `${search.q ?? ''}|${search.sort}|${search.view}|${open.has('perf')}`
        : id === 'metrics'
          ? String(search.more ?? '')
          : ''
  const [closed, setClosed] = useState<ReadonlyMap<DisclosureId, string>>(new Map())
  // Opening This month also opens Spend over time above it, which pushes This month down:
  // bring its header back to just under the sticky bar.
  const stickyBar = useRef<HTMLDivElement>(null)
  const scrollToMonth = useRef(false)
  useEffect(() => {
    const header = document.getElementById('disclosure-month-title')
    if (!scrollToMonth.current || !header || !openSet(search.open).has('month')) return
    scrollToMonth.current = false
    header.style.scrollMarginTop = `${(stickyBar.current?.offsetHeight ?? 0) + 8}px`
    header.scrollIntoView?.({
      block: 'start',
      behavior: prefersReducedMotion() ? 'auto' : 'smooth',
    })
  }, [search.open, showLoader])
  /**
   * Put the optimisation section under the sticky bar.
   *
   * One function for both ways in — the in-page button and the Overview's link — because they are
   * the same request and must land in the same place. They did not: the button scrolled without the
   * sticky-bar offset, so it stopped with the heading hidden behind the bar.
   */
  const scrollToOptimise = () => {
    const header = document.getElementById('disclosure-optimise-title')
    if (!header) return false
    header.style.scrollMarginTop = `${(stickyBar.current?.offsetHeight ?? 0) + 8}px`
    header.scrollIntoView?.({
      block: 'start',
      behavior: prefersReducedMotion() ? 'auto' : 'smooth',
    })
    return true
  }

  // Arriving from the Overview's "See the breakdown" should land on the section, not the top of a
  // page with four other panels above it.
  //
  // No dependency array on purpose. The disclosures sit behind the page loader, so on a cold load
  // the element does not exist yet, and there is no single piece of state whose change reliably
  // marks the moment it appears — `painted` is set during render, so it is already settled by the
  // time effects run. Retrying each render until the element is there, then latching on the ref,
  // is both simpler and correct; the cost is one `getElementById` per render, and only until it
  // succeeds. The ref also stops a later toggle of the same section from yanking the viewport.
  const scrolledToOpened = useRef(false)
  useEffect(() => {
    if (scrolledToOpened.current || !openSet(search.open).has('optimise')) return
    if (scrollToOptimise()) scrolledToOpened.current = true
  })
  const collapsed = (id: DisclosureId) =>
    closed.has(id) && (closed.get(id) === forcedBy(id) || (id === 'spend' && !search.day))
  const isOpen = (id: DisclosureId) =>
    !collapsed(id) &&
    (open.has(id) ||
      id === 'spend' ||
      // Open by default: savings are the one piece of good news on this page, and burying them
      // behind an expand made them invisible to anyone who did not already know to look.
      id === 'optimise' ||
      (id === 'drivers' &&
        (!!search.q || search.sort !== 'cost' || search.view !== 'agent' || open.has('perf'))) ||
      (id === 'metrics' && !!search.more))
  const toggle = (id: DisclosureId) => {
    if (isOpen(id)) {
      setClosed((prev) => new Map(prev).set(id, forcedBy(id)))
      if (open.has(id)) setSearch({ open: toggleOpen(search.open, id) }, { replace: true })
      return
    }
    const keep: DisclosureId[] = id === 'month' ? ['spend', 'month'] : [id]
    if (id === 'month') scrollToMonth.current = true
    setClosed((prev) => {
      const n = new Map(prev)
      for (const d of DISCLOSURES) {
        if (keep.includes(d)) n.delete(d)
        else if (isOpen(d)) n.set(d, forcedBy(d))
      }
      return n
    })
    setSearch(
      {
        open: keep.join(','),
        // This month opens on today's hour-by-hour breakdown unless a day is already picked.
        ...(id === 'month' && !search.day ? { day: now.toISOString().slice(0, 10) } : {}),
      },
      { replace: true },
    )
  }

  // Opening the day panel or the drawer pushes history. Closing goes Back only when the
  // current entry is still the one that open created (history index = pre-open + 1), so a
  // close leaves no duplicate entry but never undoes a filter, preset or other navigation
  // made while the panel was open. Anything else (deep links, later pushes) closes by replacing.
  const historyIndex = () => router.state.location.state.__TSR_index
  const openedAt = useRef<{ day?: number; traces?: number }>({})
  const [announceDay, setAnnounceDay] = useState(false)
  const openDay = (d: string) => {
    if (search.day) {
      setSearch({ day: d }, { replace: true })
    } else {
      openedAt.current.day = historyIndex()
      setAnnounceDay(true)
      setSearch({ day: d })
    }
  }
  const openTraces = (patch: Partial<TokenopsSearch>) => {
    openedAt.current.traces = historyIndex()
    setSearch({ ...patch, traces: true })
  }
  const close = (which: 'day' | 'traces') => {
    if (!search[which]) return // already closing (double click, repeated Escape)
    const pre = openedAt.current[which]
    openedAt.current[which] = undefined
    if (pre !== undefined && historyIndex() === pre + 1) router.history.back()
    else setSearch({ [which]: undefined }, { replace: true })
  }

  const [highlightId, setHighlightId] = useState<string | undefined>()
  const [mobilePanel, setMobilePanel] = useState<'attribution' | 'performance'>('attribution')
  // Below md (Tailwind v4: 48rem), F3 and F5 share one slot; the other panel is unmounted, not hidden (a hidden chart measures 0×0).
  const isMd = useMediaQuery('(min-width: 48rem)')
  const showAttribution = isMd || search.view !== 'agent' || mobilePanel === 'attribution'
  const showPerformance = search.view === 'agent' && (isMd || mobilePanel === 'performance')

  // Server down or /api/me failing for any reason other than 401 must not leave skeletons spinning (A25).
  const fatal = me.error ?? dash.error
  if (fatal instanceof ApiError && fatal.isServerUnreachable) return <ServerDown />
  // A 401 is handled by the QueryClient (redirect to /login); anything else gets a retry panel.
  if (me.isError && !(me.error instanceof ApiError && me.error.status === 401)) {
    return (
      <div className="mx-auto max-w-xl">
        <PanelError error={me.error} onRetry={() => void me.refetch()} what="your session" />
      </div>
    )
  }

  // Unknown agent (400) or an agent you can't access (404 "agent not found"): say why, offer to clear.
  const unknownAgent =
    dash.error instanceof ApiError &&
    (dash.error.status === 400 || dash.error.status === 404) &&
    search.agent
      ? (dash.error.serverMessage ?? `Agent ${search.agent} not found`)
      : null
  const unpriced = dash.data?.summary.unpriced_calls ?? 0
  const noFilters = !search.agent && !search.provider && !search.model
  // First run = no spend at all in the last two calendar months, whatever the agent count.
  const firstRun = noFilters && calThis.data?.days.length === 0 && calLast.data?.days.length === 0
  const today = now.toISOString().slice(0, 10)
  const agentLabel = search.agent
    ? (agentOptions.find((a) => a.id === search.agent)?.name ?? search.agent)
    : undefined
  const models = (providers.data ?? [])
    .filter((p) => !search.provider || p.provider === search.provider)
    .flatMap((p) => p.models.map((m) => m.model))
  const uniqueModels = [...new Set(models)].sort()
  const onLast30 =
    win.preset === '30d'
      ? undefined
      : () => setSearch({ preset: '30d', from: undefined, to: undefined })
  // Day-drilldown slices carry display names (the server resolves them); the filter needs
  // a raw name or UUID, so map through the dashboard's agent list. Only an unambiguous
  // match can filter: display names aren't unique, the list may still be loading, and it
  // only holds agents the caller can access (the day breakdown is fleet-wide).
  const agentIdForDisplayName = (name: string) => {
    const matches = agentOptions.filter((a) => a.name === name)
    return matches.length === 1 ? matches[0].id : undefined
  }
  // MTD's KPI Δ compares with the same-length window before the month, not "same days last month" (the hero's comparison).
  const mtdDays = Math.max(1, Math.round((win.end.getTime() - win.start.getTime()) / DAY_MS))
  const kpiCompareLabel =
    win.preset === 'mtd' ? `the previous ${mtdDays === 1 ? 'day' : `${mtdDays} days`}` : undefined

  if (firstRun) return <FirstRun />

  return (
    // The page's first load is the one page loader below the sticky bar (plan §8 Phase 9: one busy region).
    <div className="flex flex-col gap-4">
      {/* Sticky bar: title, time control, filters, breadcrumb, freshness (A1). */}
      <div
        ref={stickyBar}
        className="sticky top-0 z-20 -mx-4 flex flex-col gap-2 border-b border-border bg-background/95 px-4 py-3 backdrop-blur"
      >
        <PageHeader
          title="TokenOps"
          actions={
            <TimeControl
              preset={search.preset}
              invalid={search.preset === 'custom' && win.preset !== 'custom'}
              from={search.from}
              to={search.to}
              today={today}
              onChange={(n) =>
                setSearch(
                  { preset: n.preset, from: n.from, to: n.to },
                  { replace: search.preset === 'custom' && n.preset === 'custom' },
                )
              }
            />
          }
        />
        {/* Chips scroll horizontally on narrow screens instead of growing the sticky bar (A1b). */}
        <div className="-mx-4 flex flex-nowrap items-center gap-2 overflow-x-auto px-4 md:mx-0 md:flex-wrap md:overflow-visible md:px-0 [&>*]:shrink-0">
          <FilterSelect
            label="Agent"
            value={search.agent}
            options={agentOptions.map((a) => ({ value: a.id, label: a.name }))}
            onChange={(v) => setSearch({ agent: v, traces: undefined })}
            extraLabel={agentLabel}
          />
          <FilterSelect
            label="Provider"
            value={search.provider}
            options={(providers.data ?? []).map((p) => ({ value: p.provider, label: p.provider }))}
            onChange={(v) => setSearch({ provider: v, model: undefined })}
          />
          <FilterSelect
            label="Model"
            value={search.model}
            options={uniqueModels.map((m) => ({ value: m, label: m }))}
            onChange={(v) => setSearch({ model: v })}
          />
          <DrillCrumbs
            win={win.label}
            day={search.day}
            agent={agentLabel}
            traces={!!search.traces}
            setSearch={setSearch}
          />
          <span
            className="ml-auto text-xs text-muted-foreground"
            title="When this data was fetched"
          >
            {dash.dataUpdatedAt ? `Data as of ${fmtLocalTime(dash.dataUpdatedAt, true)}` : null}
          </span>
        </div>
      </div>

      {showLoader ? <PageLoader label="Loading TokenOps" /> : null}
      {showLoader ? null : unknownAgent ? (
        <Notice tone="warning">
          {unknownAgent}.{' '}
          <Button
            variant="link"
            size="sm"
            className="h-auto p-0 text-sm"
            onClick={() => setSearch({ agent: undefined, traces: undefined })}
          >
            Clear agent filter
          </Button>
        </Notice>
      ) : null}
      {!showLoader && me.data && !me.data.is_superuser ? (
        <Notice tone="info">
          KPIs and the attribution table only include agents you can access. The calendar, timeline
          and day breakdown are fleet-wide because those server endpoints don't check agent access
          yet (a known server gap), so their totals can differ and may name agents you can't open.
        </Notice>
      ) : null}

      {/* An unknown agent filter makes every query fail the same permanent way: the notice
          above says why and how to recover, so show nothing that offers a futile Retry. */}
      {showLoader || unknownAgent ? null : (
        <>
          <SummaryHero
            sentences={narrative?.sentences ?? []}
            spike={narrative?.spike ?? null}
            spikeLink={(children) =>
              narrative?.spike ? (
                <Link
                  to="/sessions"
                  search={{ ...withoutWindow(search), day: narrative.spike.date, sort: 'cost' }}
                  className="font-medium text-primary-text underline-offset-4 hover:underline"
                >
                  {children}
                </Link>
              ) : null
            }
            windowLabel={win.label}
            kpis={dash.data?.kpis}
            totalAgents={dash.data?.summary.total_agents}
            compare={compare && !prevUnavailable}
            unpriced={unpriced > 0}
            month={summary}
            loading={dash.isPending || summaryStale}
            error={dash.error}
            onRetry={() => void dash.refetch()}
          />

          <SavingsHighlight
            data={optimisation}
            onSeeDetail={() => {
              if (!isOpen('optimise')) toggle('optimise')
              scrollToOptimise()
            }}
          />

          <div className="flex flex-col">
            <Disclosure
              id="spend"
              title="Spend over time"
              hint={win.label}
              open={isOpen('spend')}
              onToggle={() => toggle('spend')}
            >
              <SpendTimeline
                points={timeline}
                bucket={timeseries.data?.bucket ?? 'day'}
                loading={timeseries.isPending}
                error={timeseries.error}
                onRetry={() => void timeseries.refetch()}
                onOpenDay={openDay}
                onLast30={onLast30}
                windowLabel={win.label.toLowerCase()}
              />

              {search.day ? (
                <DayPanel
                  date={search.day}
                  data={day.data}
                  loading={day.isPending}
                  error={day.error}
                  onRetry={() => void day.refetch()}
                  onClose={() => close('day')}
                  announce={announceDay}
                  onAnnounced={() => setAnnounceDay(false)}
                  canFilterAgent={(name) => agentIdForDisplayName(name) !== undefined}
                  onFilterAgent={(name) => {
                    const id = agentIdForDisplayName(name)
                    if (id) setSearch({ agent: id, view: 'agent' })
                  }}
                  onViewTraces={() => openTraces({})}
                  morph={narrative?.spike?.date !== search.day}
                  sessionsLink={
                    <Link
                      to="/sessions"
                      search={{ ...withoutWindow(search), day: search.day, sort: 'cost' }}
                      className="text-sm font-medium text-primary-text underline-offset-4 hover:underline"
                    >
                      See sessions <span aria-hidden>→</span>
                    </Link>
                  }
                />
              ) : null}
            </Disclosure>

            <Disclosure
              id="optimise"
              title="Token optimisation"
              hint={
                optimisation
                  ? `${optimisation.optimisedCount} of ${optimisation.totalAgents} agents · ${showOptPct(optimisation.savedPct)} tokens saved`
                  : 'savings per layer and per agent'
              }
              open={isOpen('optimise')}
              onToggle={() => toggle('optimise')}
            >
              {savings.isError ? (
                <StateCard
                  tone="warning"
                  title="Savings could not be loaded"
                  fix="Retry, or check the control plane logs for the finops savings query."
                  action={
                    <Button variant="outline" size="sm" onClick={() => void savings.refetch()}>
                      Retry
                    </Button>
                  }
                >
                  {savings.error instanceof Error ? savings.error.message : 'Request failed.'}
                </StateCard>
              ) : optimisation ? (
                <OptimisationPanel data={optimisation} />
              ) : (
                <div className="h-48 animate-pulse rounded-md bg-muted" aria-busy />
              )}
            </Disclosure>

            <Disclosure
              id="drivers"
              title="Who is driving cost"
              hint="agents, workflows, cost vs performance"
              open={isOpen('drivers')}
              onToggle={() => toggle('drivers')}
            >
              {search.view === 'agent' ? (
                <div className="md:hidden">
                  <ToggleGroup
                    type="single"
                    variant="outline"
                    size="sm"
                    value={mobilePanel}
                    onValueChange={(v) => v && setMobilePanel(v as 'attribution' | 'performance')}
                    aria-label="Panel"
                  >
                    <ToggleGroupItem value="attribution" className="px-3 text-xs">
                      Who drives cost
                    </ToggleGroupItem>
                    <ToggleGroupItem value="performance" className="px-3 text-xs">
                      Cost vs performance
                    </ToggleGroupItem>
                  </ToggleGroup>
                </div>
              ) : null}
              <div className="grid gap-4 xl:grid-cols-12">
                {showAttribution ? (
                  <div className="min-w-0 xl:col-span-8">
                    <AttributionTable
                      rows={attrRows}
                      view={search.view}
                      sort={
                        search.view === 'workflow' && search.sort === 'hours' ? 'cost' : search.sort
                      }
                      q={search.q}
                      loading={dash.isPending}
                      error={dash.error}
                      onRetry={() => void dash.refetch()}
                      prevUnavailable={prevUnavailable}
                      highlightId={highlightId}
                      agentFilter={search.agent}
                      onView={(v) => setSearch({ view: v, sort: 'cost' })}
                      onSort={(s) => setSearch({ sort: s }, { replace: true })}
                      onSearch={(q) => setSearch({ q: q || undefined }, { replace: true })}
                      // Row traces cover the row's whole window, so leave any day drill-down.
                      onViewTraces={(id) =>
                        openTraces({ agent: id, view: 'agent', day: undefined })
                      }
                      onLast30={onLast30}
                      unpriced={unpriced > 0}
                    />
                  </div>
                ) : null}
                {showPerformance ? (
                  <div className="min-w-0 xl:col-span-4">
                    <CostPerformance
                      rows={attrRows}
                      loading={dash.isPending}
                      error={dash.error}
                      onRetry={() => void dash.refetch()}
                      onSelect={(id) => {
                        setHighlightId(id)
                        setMobilePanel('attribution')
                      }}
                    />
                  </div>
                ) : null}
              </div>
            </Disclosure>

            <Disclosure
              id="month"
              title="This month"
              hint="calendar and forecast method"
              open={isOpen('month')}
              onToggle={() => toggle('month')}
            >
              <MonthHero
                summary={summary}
                unpricedCalls={
                  win.preset === 'mtd' && noFilters && dash.data && !dash.isPlaceholderData
                    ? unpriced
                    : null
                }
                days={calPending ? undefined : calThis.data?.days}
                monthStart={thisMonth}
                today={today}
                selectedDay={search.day}
                onSelectDay={openDay}
                loading={calPending}
                error={calThis.error ?? calLast.error}
                onRetry={() => {
                  void calThis.refetch()
                  void calLast.refetch()
                }}
              />
            </Disclosure>

            <Disclosure
              id="metrics"
              title="All metrics"
              hint={`operations, cost per operation, latency · ${win.label}`}
              open={isOpen('metrics')}
              onToggle={() => toggle('metrics')}
            >
              <KpiStrip
                data={dash.data}
                loading={dash.isPending}
                error={dash.error}
                onRetry={() => void dash.refetch()}
                more={!!search.more}
                onToggleMore={() =>
                  setSearch({ more: search.more ? undefined : true }, { replace: true })
                }
                windowLabel={win.label}
                compareLabel={kpiCompareLabel}
                hideDeltas={!compare}
              />
            </Disclosure>
          </div>

          <TracesDrawer
            open={!!search.traces}
            onOpenChange={(open) => {
              if (!open) close('traces')
            }}
            scopeLabel={[agentLabel ?? 'All agents', search.model, tracesWin.label]
              .filter(Boolean)
              .join(' · ')}
            data={traces.data}
            loading={traces.isPending}
            error={traces.error}
            onRetry={() => void traces.refetch()}
            legacyUiUrl={env.legacyUiUrl}
          />
        </>
      )}
    </div>
  )
}

function FilterSelect({
  label,
  value,
  options,
  onChange,
  extraLabel,
}: {
  label: string
  value?: string
  options: { value: string; label: string }[]
  onChange: (v: string | undefined) => void
  extraLabel?: string
}) {
  const known = !value || options.some((o) => o.value === value)
  return (
    <div className="flex items-center">
      <Select value={value ?? ALL} onValueChange={(v) => onChange(v === ALL ? undefined : v)}>
        <SelectTrigger
          size="sm"
          className="h-8 max-w-48 text-xs"
          aria-label={`Filter by ${label.toLowerCase()}`}
        >
          <span className="text-muted-foreground">{label}:</span> <SelectValue />
        </SelectTrigger>
        <SelectContent>
          <SelectItem value={ALL}>All</SelectItem>
          {!known && value ? <SelectItem value={value}>{extraLabel ?? value}</SelectItem> : null}
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
          aria-label={`Remove ${label.toLowerCase()} filter`}
        >
          <X className="size-3.5" aria-hidden />
        </Button>
      ) : null}
    </div>
  )
}

function DrillCrumbs({
  win,
  day,
  agent,
  traces,
  setSearch,
}: {
  win: string
  day?: string
  agent?: string
  traces: boolean
  setSearch: SetSearch
}) {
  if (!day && !agent && !traces) return null
  const crumbs: { level: string; label: string; clear: Partial<TokenopsSearch> }[] = [
    { level: 'window', label: win, clear: { day: undefined, agent: undefined, traces: undefined } },
  ]
  if (day)
    crumbs.push({
      level: 'day',
      label: fmtShortDay(day),
      clear: { agent: undefined, traces: undefined },
    })
  if (agent) crumbs.push({ level: 'agent', label: agent, clear: { traces: undefined } })
  if (traces) crumbs.push({ level: 'traces', label: 'Traces', clear: {} })
  return (
    <Breadcrumb aria-label="Drill-down">
      <BreadcrumbList className="gap-1 text-xs sm:gap-1">
        {crumbs.map((c, i) => (
          <Fragment key={c.level}>
            {i > 0 ? <BreadcrumbSeparator className="[&>svg]:size-3" /> : null}
            <BreadcrumbItem className="gap-1">
              {i === crumbs.length - 1 ? (
                <BreadcrumbPage aria-current="location" className="font-medium">
                  {c.label}
                </BreadcrumbPage>
              ) : (
                <BreadcrumbLink asChild>
                  <Button
                    variant="link"
                    size="sm"
                    className="h-auto p-0 text-xs font-normal text-muted-foreground underline-offset-2 hover:text-foreground"
                    onClick={() => setSearch(c.clear)}
                  >
                    {c.label}
                  </Button>
                </BreadcrumbLink>
              )}
            </BreadcrumbItem>
          </Fragment>
        ))}
      </BreadcrumbList>
    </Breadcrumb>
  )
}

function Notice({ tone, children }: { tone: 'info' | 'warning'; children: React.ReactNode }) {
  const Icon = tone === 'info' ? Info : AlertTriangle
  return (
    <Alert
      role={tone === 'warning' ? 'alert' : 'status'}
      className={
        tone === 'info'
          ? 'border-info/30 bg-info/5 p-3 [&>svg]:text-info'
          : 'border-warning/40 bg-warning/5 p-3 [&>svg]:text-warning'
      }
    >
      <Icon aria-hidden />
      <AlertDescription className="max-w-prose text-foreground">
        <div>{children}</div>
      </AlertDescription>
    </Alert>
  )
}

function FirstRun() {
  return (
    <div className="flex flex-col gap-4">
      <PageHeader title="TokenOps" />
      {/* The steps sit inside the card as its action, widened past Empty's 24rem content cap. */}
      <EmptyState
        icon={DollarSign}
        title="No AI spend recorded yet"
        className="*:data-[slot=empty-content]:max-w-xl"
        action={
          <div className="flex w-full flex-col gap-3 rounded-md border bg-muted/40 p-4 text-left text-sm text-muted-foreground">
            <ol className="flex list-decimal flex-col gap-1.5 pl-5">
              <li>
                Set <code className="font-mono text-foreground">TEMPO_URL</code> and{' '}
                <code className="font-mono text-foreground">LOKI_URL</code> on nasiko-server and run
                the OTel collector.
              </li>
              <li>Deploy an agent and send it a few requests.</li>
              <li>
                Spend appears within a couple of minutes (the trace materializer runs every 120 s).
              </li>
            </ol>
            <p className="border-t pt-3 text-xs text-pretty">
              Setup details: <code className="font-mono">oss/docs/BOOTSTRAP_AND_NETWORKING.md</code>{' '}
              in the nasiko repository (collector, Tempo and Loki wiring). Developing locally? Run{' '}
              <code className="font-mono">npm run seed:live</code> against your OSS stack, or use
              mock mode.
            </p>
          </div>
        }
      >
        TokenOps reads spend from agent traces. Nothing has arrived in the last two months, which
        usually means the telemetry pipeline isn't connected yet.
      </EmptyState>
    </div>
  )
}

function ServerDown() {
  return (
    <StateCard
      tone="error"
      className="mx-auto max-w-xl"
      title="Can't reach nasiko-server"
      fix={
        <span className="text-xs">
          Or switch to mock data: <code>VITE_NASIKO_API_MODE=mock npm run dev</code>.
        </span>
      }
      action={
        <Button className="w-fit" variant="outline" onClick={() => window.location.reload()}>
          Try again
        </Button>
      }
    >
      The dev proxy couldn't connect to the API. Start the OSS stack in <code>nasiko-cloud-rs</code>{' '}
      with <code>just run-stack</code>, or point <code>NASIKO_API_URL</code> at a running server and
      restart <code>npm run dev</code>.
    </StateCard>
  )
}
