/**
 * TokenOps, question-led (plans/feat-tokenops-page.md), as one open page: nothing hides behind an expander.
 *
 *   Hero bento: narrative + spend curve │ six KPI tiles │ more metrics │ savings strip
 *   ── sticky section nav (Spend · Drivers · Optimisation) ──
 *   Spend: F2 timeline (Spend · Latency) ─ click bucket ─▶ ?day= ─▶ DayPanel ─ agent ─▶ ?agent= │ View traces ─▶ ?traces=true
 *          F1 This month (calendar + forecast) ─ click day ─▶ ?day=
 *   Drivers: concentration + token mix │ F3 attribution | F5 cost × p95   F3 row "Traces" ─▶ ?agent=<id>&traces=true
 *   Optimisation: /finops/savings bento + chat history (CX-V3)
 *   F4 traces drawer (sheet)
 *
 * The shell's top bar carries the time presets and the drill path (TokenOps › day › agent › Traces); the page header
 * keeps the title, freshness and filters. All state lives in the URL. Drill-downs push history (Back unwinds them);
 * sort and search replace it (A5). `?open=` (links from other pages) scrolls to its section on arrival.
 */
import { useQuery, useQueryClient } from '@tanstack/react-query'
import { Link, useRouter } from '@tanstack/react-router'
import { AlertTriangle, DollarSign, Info, X } from 'lucide-react'
import { AnimatePresence, m } from 'motion/react'
import { useEffect, useMemo, useRef, useState } from 'react'
import { Alert, AlertDescription } from '@nasiko/ui/primitives/alert'
import { Button } from '@nasiko/ui/primitives/button'
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from '@nasiko/ui/primitives/select'
import { meQuery } from '@nasiko/data-access/auth'
import { ApiError } from '@nasiko/data-access/client'
import { env } from '@nasiko/app-core/env'
import { fmtLocalTime, fmtPct, fmtShortDay } from '@nasiko/ui/lib/format'
import { useMediaQuery } from '@nasiko/ui/lib/useMediaQuery'
import { transitions } from '@nasiko/ui/lib/motion'
import { useFrozenNow, useReturnTick } from '@nasiko/app-core/useReturnTick'
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
import { compareOn, withoutWindow } from '@nasiko/app-core/shell/context'
import { TopBar } from '@nasiko/app-core/shell/TopBar'
import { tokenopsNarrative } from '@nasiko/features/narrative/tokenops'
import { useSlots } from '@nasiko/app-core/edition/edition-context'
import { buildAttribution } from './attribution'
import { summarizeMonth } from './forecast'
import { openSet, type Disclosure as DisclosureId, type TokenopsSearch } from './search'
import { toTimeline, zeroFillTimeline } from './series'
import { DAY_MS, dayWindow, monthKey, resolveWindow, utcMonthStart } from './window'
import { AttributionTable } from './components/AttributionTable'
import { CostPerformance } from './components/CostPerformance'
import { DayPanel } from './components/DayPanel'
import { DriversOverview } from './components/DriversOverview'
import { ChatHistory } from './components/ChatHistory'
import { Reveal, RevealItem } from '@nasiko/ui/composites/reveal'
import { Segmented } from './components/Segmented'
import { SectionNav } from './components/SectionNav'
import { scrollToSection, sectionDomId } from './sections'
import { useSavingsTrend } from '@nasiko/features/optimization/api'
import { MonthHero } from './components/MonthHero'
import { copy as optimizationCopy } from '@nasiko/features/optimization/copy'
import { OptimisationPanel } from './components/OptimisationPanel'
import { SavingsHighlight } from './components/SavingsHighlight'
import { summarizeOptimisation } from './optimisation'
import { PageHeader } from '@nasiko/ui/composites/page-header'
import { ImportingOr, ImportNotice } from '@nasiko/features/harnesses/components/ImportNotice'
import { PanelError } from '@nasiko/ui/composites/panel'
import { PageLoader } from '@nasiko/ui/composites/page-loader'
import { EmptyState, StateCard } from '@nasiko/ui/composites/state-card'
import { SpendTimeline } from './components/SpendTimeline'
import { SummaryHero } from './components/SummaryHero'
import { CustomRange, InvalidRangeNote, TimePresets } from '@nasiko/ui/composites/time-control'
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
  // EE slot: null for OSS, or for a viewer with no units to pick (a plain member) — the component
  // decides that itself from its own org-units read, same as the login page's SSO button does.
  const { tokenopsOrgUnitFilter: OrgUnitFilter } = useSlots()
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
    () => ({
      agent: search.agent,
      provider: search.provider,
      model: search.model,
      orgUnit: search.org_unit,
    }),
    [search.agent, search.provider, search.model, search.org_unit],
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
  // CX-V3 (proposed): history selection's savings, the /optimization lead's read under the same key.
  const history = useSavingsTrend(win, ready)
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

  // Arriving with `?open=` (the Overview's "See the breakdown", a trace's "Open in TokenOps") should land on that
  // section, not the top of the page. No dependency array on purpose: the sections sit behind the page loader, so on a
  // cold load the element does not exist yet and no single piece of state marks the moment it appears. Retry each
  // render until it is there, then latch, so a later scroll of the user's own is never yanked back.
  const arrivedAt = useRef(false)
  useEffect(() => {
    if (arrivedAt.current) return
    const target = arrivalSection(openSet(search.open))
    if (!target) {
      arrivedAt.current = true
      return
    }
    if (scrollToSection(target)) arrivedAt.current = true
  })

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
  // MTD's KPI Δ compares with the same-length window before the month, not "same days last month" (the month's comparison).
  const mtdDays = Math.max(1, Math.round((win.end.getTime() - win.start.getTime()) / DAY_MS))
  const kpiCompareLabel =
    win.preset === 'mtd' ? `the previous ${mtdDays === 1 ? 'day' : `${mtdDays} days`}` : undefined
  const historyData =
    history.data?.kind === 'ready'
      ? history.data.data
      : history.isError || history.data
        ? null
        : undefined
  const optimiseHint = optimisation
    ? `${optimisation.optimisedCount} of ${optimisation.totalAgents} agents · ${showOptPct(optimisation.savedPct)} tokens saved`
    : 'savings per layer and per agent'
  const sections = [
    { id: 'spend', label: 'Spend', hint: win.label },
    { id: 'drivers', label: 'Drivers' },
    { id: 'optimize', label: 'Optimization', hint: optimisation ? optimiseHint : undefined },
  ]

  const time = {
    preset: search.preset,
    invalid: search.preset === 'custom' && win.preset !== 'custom',
    from: search.from,
    to: search.to,
    today,
    onChange: (n: Pick<TokenopsSearch, 'preset' | 'from' | 'to'>) =>
      setSearch(
        { preset: n.preset, from: n.from, to: n.to },
        { replace: search.preset === 'custom' && n.preset === 'custom' },
      ),
  }

  const filterControls = (
    <>
      {OrgUnitFilter ? (
        <OrgUnitFilter value={search.org_unit} onChange={(v) => setSearch({ org_unit: v })} />
      ) : null}
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
    </>
  )

  if (firstRun) return <FirstRun />

  return (
    // The page's first load is the one page loader below the header (plan §8 Phase 9: one busy region).
    <div className="flex flex-col gap-4">
      {/* The top bar is the page's one sticky row: the presets and the drill path stay in view (A1). */}
      <TopBar
        {...drillTrail(search, agentLabel, setSearch)}
        actions={
          <>
            <TimePresets {...time} />
            {/* Custom dates sit beside the presets; a phone's bar has no room, so there they join the filters. */}
            {isMd ? <CustomRange {...time} noteElsewhere /> : null}
          </>
        }
        pinActions
      />
      {/* Left: what you are looking at and how fresh it is. Right: what narrows it, under the time presets. */}
      <PageHeader
        title="TokenOps"
        description={
          dash.dataUpdatedAt ? (
            <span title="When this data was fetched">
              Data as of {fmtLocalTime(dash.dataUpdatedAt, true)}
            </span>
          ) : null
        }
        actions={
          isMd ? (
            // A column, so the invalid-range note sits under the filters and both keep the right edge.
            <div data-tour="tokenops-filters" className="flex flex-col items-end gap-1">
              <div className="flex flex-wrap items-center justify-end gap-2">{filterControls}</div>
              {time.invalid ? <InvalidRangeNote /> : null}
            </div>
          ) : null
        }
        className="mb-2"
      />
      {/* Phones: the custom dates and the filters scroll sideways in one row instead of wrapping (A1b). */}
      {isMd ? null : (
        <div
          data-tour="tokenops-filters"
          className="-mx-4 mb-2 flex flex-nowrap items-center gap-2 overflow-x-auto px-4 [&>*]:shrink-0"
        >
          <CustomRange {...time} />
          {filterControls}
        </div>
      )}

      {/* Coding agents' spend is still arriving while the desktop imports them. */}
      <ImportNotice />
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
      {/* Fixed 2026-10-22: KPIs, the agent attribution table, the calendar, the timeline and the day
          breakdown all now scope to your own calls on agents you can access (oss/server/src/observability,
          ee/server/src/finops_scope.rs). Only the workflow attribution table is still fleet-wide — its
          runs are matched by agent *name* inside a JSON column, with no caller column to scope by yet. */}
      {!showLoader && me.data && !me.data.is_superuser && search.view === 'workflow' ? (
        <Notice tone="info">
          The workflow table is fleet-wide (a known server gap): it may show totals from runs and
          name agents you don't have access to. KPIs, the agent table, the calendar and the timeline
          reflect only your own calls to agents you can access.
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
            compareLabel={kpiCompareLabel}
            data={dash.data}
            points={summaryStale ? [] : timeline}
            compare={compare && !prevUnavailable}
            unpriced={unpriced > 0}
            month={summary}
            loading={dash.isPending || summaryStale}
            error={dash.error}
            onRetry={() => void dash.refetch()}
          >
            <SavingsHighlight data={optimisation} onSeeDetail={() => scrollToSection('optimize')} />
          </SummaryHero>

          <SectionNav sections={sections} />

          <Reveal data-tour="tokenops-sections" className="flex flex-col gap-10">
            <PageSection id="spend" title="Spend" hint={win.label}>
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

              <AnimatePresence initial={false}>
                {search.day ? (
                  <m.div
                    key="day"
                    initial={{ opacity: 0, y: -8 }}
                    animate={{ opacity: 1, y: 0 }}
                    exit={{ opacity: 0, y: -8 }}
                    transition={transitions.disclosure}
                  >
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
                  </m.div>
                ) : null}
              </AnimatePresence>

              {/* `layout`: when the day panel opens above, the month glides down instead of jumping. */}
              <m.div layout="position" transition={transitions.disclosure}>
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
              </m.div>
            </PageSection>

            <PageSection
              id="drivers"
              title="Drivers"
              hint={search.view === 'agent' ? 'agents, tokens, cost vs performance' : 'workflows'}
            >
              <DriversOverview
                rows={attrRows}
                tokens={dash.data?.token_usage}
                view={search.view}
                loading={dash.isPending}
              />
              {search.view === 'agent' && !isMd ? (
                <Segmented
                  label="Panel"
                  value={mobilePanel}
                  options={MOBILE_PANELS}
                  onChange={setMobilePanel}
                  className="w-fit"
                />
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
            </PageSection>

            <PageSection id="optimize" title="Optimization" hint={optimiseHint}>
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
                <>
                  <OptimisationPanel
                    data={optimisation}
                    history={
                      <ChatHistory
                        data={historyData}
                        loading={history.isPending}
                        filtered={!noFilters}
                        search={search}
                      />
                    }
                  />
                  {/* P3 (plans/feat-optimization-page.md): the trend, the agents and the fixes live on Optimization,
                      opened on this window (R7A: the two pages share the window keys). */}
                  <Link
                    to="/optimization"
                    search={{ preset: search.preset, from: search.from, to: search.to }}
                    className="inline-block w-fit text-xs underline underline-offset-4 hover:text-primary-text pointer-coarse:py-3"
                  >
                    {optimizationCopy.savings.more}
                  </Link>
                </>
              ) : (
                <div className="h-48 animate-pulse rounded-md bg-muted" aria-busy />
              )}
            </PageSection>
          </Reveal>

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

const MOBILE_PANELS = [
  { value: 'attribution', label: 'Who drives cost' },
  { value: 'performance', label: 'Cost vs performance' },
] as const

/** Which section an `?open=` link means; the retired ids map onto the section that now holds them. */
function arrivalSection(open: Set<DisclosureId>): string | undefined {
  if (open.has('optimize')) return 'optimize'
  if (open.has('drivers') || open.has('perf')) return 'drivers'
  if (open.has('month') || open.has('spend')) return 'spend'
  return undefined
}

/** One part of the page: a quiet header the section nav jumps to, then its panels. */
function PageSection({
  id,
  title,
  hint,
  children,
}: {
  id: string
  title: string
  hint?: string
  children: React.ReactNode
}) {
  return (
    <RevealItem>
      {/* scroll-mt clears the sticky section nav (the root's scroll-padding already clears the top bar). */}
      <section
        id={sectionDomId(id)}
        aria-labelledby={`${sectionDomId(id)}-title`}
        className="flex scroll-mt-14 flex-col gap-4"
      >
        <header className="flex items-baseline gap-2">
          <h2 id={`${sectionDomId(id)}-title`} className="text-base font-semibold tracking-tight">
            {title}
          </h2>
          {hint ? <span className="text-sm text-muted-foreground">{hint}</span> : null}
        </header>
        {children}
      </section>
    </RevealItem>
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

/**
 * The drill path as the top bar's breadcrumb: TokenOps › day › agent › Traces. Each level above the current one is a
 * button that clears what sits below it; TokenOps clears the whole drill but keeps the window and the other filters.
 * With nothing drilled the shell's own breadcrumb stays.
 */
function drillTrail(
  search: TokenopsSearch,
  agentLabel: string | undefined,
  setSearch: SetSearch,
): { root?: React.ReactNode; trail?: React.ReactNode[] } {
  const { day, traces } = search
  if (!day && !agentLabel && !traces) return {}
  const levels: { label: string; clear: Partial<TokenopsSearch> }[] = [
    { label: 'TokenOps', clear: { day: undefined, agent: undefined, traces: undefined } },
  ]
  if (day) levels.push({ label: fmtShortDay(day), clear: { agent: undefined, traces: undefined } })
  if (agentLabel) levels.push({ label: agentLabel, clear: { traces: undefined } })
  if (traces) levels.push({ label: 'Traces', clear: {} })
  const [root, ...trail] = levels.map((l, i) =>
    i === levels.length - 1 ? (
      l.label
    ) : (
      <Button
        variant="link"
        className="h-auto p-0 text-sm font-normal text-muted-foreground hover:text-foreground"
        onClick={() => setSearch(l.clear)}
      >
        {l.label}
      </Button>
    ),
  )
  return { root, trail }
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
      {/* While coding agents sync, their spend is on its way: the loading state, never "nothing recorded yet". */}
      <ImportingOr>
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
                  <code className="font-mono text-foreground">LOKI_URL</code> on nasiko-server and
                  run the OTel collector.
                </li>
                <li>Deploy an agent and send it a few requests.</li>
                <li>
                  Spend appears within a couple of minutes (the trace materializer runs every 120
                  s).
                </li>
              </ol>
              <p className="border-t pt-3 text-xs text-pretty">
                Developing locally? Run <code className="font-mono">npm run seed:live</code> against
                your OSS stack, or use mock mode.
              </p>
            </div>
          }
        >
          TokenOps reads spend from agent traces. Nothing has arrived in the last two months, which
          usually means the telemetry pipeline isn't connected yet.
        </EmptyState>
      </ImportingOr>
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
