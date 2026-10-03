/**
 * The Overview's shared data (plans/feat-overview.md §5, eng review R1): `useFleetHealth()` is the one source of the
 * fleet ratings, called by the Overview and by the Agents catalog while `?health=` is set, so both apply the same
 * rules to the same directory rows. It reuses existing query keys, so TokenOps and the Agents pages open warm.
 */
import { isNotVisible, useUsage } from '@/features/harnesses/api'
import { useLiveIndividual } from '@/features/harnesses/liveIndividual'
import { mostlyUnpriced } from '@/features/harnesses/rollup'
import { sessionRows, useChatSessions } from '@/features/chat/api'
import { useWaiting } from '@/features/chat/waiting'
import { STATUS_CHECK_ROWS } from '@/features/observability/tuning'
import { tempoSafeStart, useFleetSessions, useSessionStatuses } from '@/features/sessions/api'
import type { Me } from '@/lib/api/auth'
import { useQueries } from '@tanstack/react-query'
import { useCallback, useMemo } from 'react'
import { useAgentsDirectory } from '@/features/agents/api'
import type { Agent } from '@/features/agents/types'
import { displayStatus, isHarness } from '@/features/agents/status'
import { useBudgetStatus, useBudgets } from '@/features/router/api'
import { budgetForecast, usedPercent, type Forecast } from '@/features/router/budgets'
import type { BudgetAction, BudgetState } from '@/features/router/types'
import { findSpike, type Spike } from '@/features/narrative/tokenops'
import {
  timeseriesQuery,
  useCalendar,
  useDashboard,
  useSavings,
  useTimeseries,
} from '@/features/tokenops/api'
import { buildAttribution, type AttributionRow } from '@/features/tokenops/attribution'
import { summarizeMonth, type MonthSummary } from '@/features/tokenops/forecast'
import { zeroFillTimeline, type TimelinePoint } from '@/features/tokenops/series'
import type { AgentFinopsRow, FinopsSpendTimeseries } from '@/features/tokenops/types'
import { summarizeOptimisation } from '@/features/tokenops/optimisation'
import {
  monthKey,
  resolveWindow,
  utcMonthStart,
  type ResolvedWindow,
} from '@/features/tokenops/window'
import { RANGE_DAYS, type Range } from './search'
import { isEndpointAbsent } from '@/lib/api/detect'
import { copy } from './copy'
import { mergeNeeds, type Needs, type NeedsInput, type SourceId } from './needs'
import { stackSpend, type Stack } from './stack'
import { DAY, TOP_DRIVERS } from './tuning'
import {
  RATED_STATUSES,
  rateAgent,
  spikeDrivers,
  summarizeFleet,
  uniqueByName,
  type FleetHealthSummary,
  type UsageWindow,
} from './health'

/** Agents the ratings cover: every visible agent except coding harnesses (rated on the Harnesses card). */
const RATED = new Set<string>(RATED_STATUSES)

const usage = (rows: readonly AgentFinopsRow[] | undefined) => {
  const m = new Map<string, UsageWindow>()
  for (const r of rows ?? [])
    m.set(r.agent_id, {
      cost: r.total_cost,
      operations: r.operations,
      p95: r.avg_latency_p95_ms ?? null,
    })
  return m
}
const ZERO: UsageWindow = { cost: 0, operations: 0, p95: null }

export interface FleetHealth {
  /** The directory hasn't answered yet: nothing can be rated. */
  isPending: boolean
  /** The directory failed: the card shows its error. */
  error: unknown
  summary: FleetHealthSummary | null
  /** Some rating inputs are still loading (dashboards, spend series, budgets): Needs you waits (design review 5A). */
  ratingsPending: boolean
  /** A cost input failed (7-day or previous-week or 30-day dashboard, spend series, budgets): every agent's cost
   *  rating is Unknown, never Healthy, and the card says so (/ship adversarial). */
  costFailed: boolean
  /** Visible agents other than harnesses; zero means first run (design review 7A). */
  agentCount: number
  /** Of those, how many are deployed (running, deploying or crashed/failed): only these can be rated. */
  deployedCount: number
  /** The directory's indexes, so Needs you labels rows without a second directory observer. */
  byId: ReadonlyMap<string, Agent>
  /** A raw agent name → its display name, only when exactly one visible agent has that name (else the raw name). */
  displayName(raw: string): string
  /** Each agent's p95 latency over the last 7 days (the window the ratings use); absent when it had no calls. */
  p95: ReadonlyMap<string, number>
  retry(): void
  retryCost(): void
}

/**
 * Ratings for every visible agent. `enabled: false` (the Agents catalog without `?health=`) fetches and computes
 * nothing. There are no per-agent `/deployment` reads: the recent-crash rule was dropped (/ship review).
 */
export function useFleetHealth(now: Date, enabled = true): FleetHealth {
  const dir = useAgentsDirectory(enabled)
  const win7 = useMemo(() => resolveWindow({ preset: '7d' }, now), [now])
  const win30 = useMemo(() => resolveWindow({ preset: '30d' }, now), [now])
  const agents = useMemo(
    () => (enabled ? (dir.data ?? []).filter((a) => !isHarness(a)) : []),
    [enabled, dir.data],
  )
  // First run (no agents but harnesses) fetches nothing else (design review 7A).
  const on = enabled && agents.length > 0
  const cur = useDashboard(win7, {}, 'agent', 'current', on)
  const prev = useDashboard(win7, {}, 'agent', 'previous', on)
  const d30 = useDashboard(win30, {}, 'agent', 'current', on)
  const ts = useTimeseries(win30, {}, on)
  // Budgets hidden: no server support for /api/budgets yet (R-L10). Restore the commented lines when it lands.
  // const budgets = useBudgets(on)
  // const budgetsAbsent = isEndpointAbsent(budgets.error)
  const budgets = useBudgets(false)
  const budgetsAbsent = true
  // Only once the list answered: on a server without budgets (bare 404) no status request goes out.
  const budgetStatus = useBudgetStatus(on && budgets.isSuccess)
  const deployedCount = useMemo(
    () => agents.filter((a) => RATED.has(displayStatus(a.status, false))).length,
    [agents],
  )
  const { byId, byNameAll } = dir
  const displayName = useCallback(
    (raw: string) => {
      const id = uniqueByName(byNameAll)(raw)
      return (id ? byId.get(id)?.display_name : undefined) || raw
    },
    [byId, byNameAll],
  )
  // Any failed cost input leaves ratings incomplete: rate cost as unknown rather than silently healthy.
  const costFailed =
    on &&
    (cur.isError ||
      prev.isError ||
      d30.isError ||
      ts.isError ||
      (budgets.isError && !budgetsAbsent) ||
      budgetStatus.isError)

  const summary = useMemo(() => {
    if (!enabled || !dir.data) return null
    const t = now.getTime()
    const current = cur.data && !costFailed ? usage(cur.data.agents) : null
    const previous = prev.data && !prev.isError ? usage(prev.data.agents) : null
    const perOp30 = new Map(
      (d30.data?.agents ?? []).map((r) => [r.agent_id, r.avg_cost_per_operation ?? null]),
    )
    const days =
      ts.data?.bucket === 'day'
        ? zeroFillTimeline(ts.data.points, 'day', win30.start, win30.end).map((p) => ({
            date: p.iso.slice(0, 10),
            spend: p.spend,
            topAgent: p.topAgent,
          }))
        : []
    const spikes = spikeDrivers(days, uniqueByName(dir.byNameAll), t)
    const budgetByAgent = new Map<string, { stopped: boolean; exceeded: boolean }>()
    const statusById = new Map((budgetStatus.data?.data ?? []).map((st) => [st.budget_id, st]))
    for (const b of budgets.data ?? []) {
      const st = statusById.get(b.id)
      if (b.agent_id && st)
        budgetByAgent.set(b.agent_id, { stopped: st.stopped, exceeded: st.state === 'exceeded' })
    }
    return summarizeFleet(
      agents.map((a) =>
        rateAgent(
          {
            id: a.id,
            name: a.display_name || a.name,
            display: displayStatus(a.status, false),
            raw: a.status,
            createdAt: a.created_at,
            updatedAt: a.updated_at,
            // A loaded window without the agent's row is a real zero; a missing or failed window is unknown.
            current: current ? (current.get(a.id) ?? ZERO) : null,
            previous: previous ? (previous.get(a.id) ?? ZERO) : null,
            costPerOp30d: perOp30.get(a.id) ?? null,
            budget: budgetByAgent.get(a.id),
            spike: spikes.get(a.id),
          },
          t,
        ),
      ),
    )
  }, [
    enabled,
    dir.data,
    dir.byNameAll,
    agents,
    now,
    cur.data,
    costFailed,
    prev.data,
    prev.isError,
    d30.data,
    ts.data,
    win30,
    budgets.data,
    budgetStatus.data,
  ])

  const p95 = useMemo(
    () =>
      new Map(
        (cur.data?.agents ?? []).flatMap((r) =>
          r.avg_latency_p95_ms == null ? [] : [[r.agent_id, r.avg_latency_p95_ms] as const],
        ),
      ),
    [cur.data],
  )
  const budgetsSettled =
    budgetsAbsent || budgets.isError || budgetStatus.isSuccess || budgetStatus.isError
  const ratingsPending =
    on && (cur.isPending || prev.isPending || d30.isPending || ts.isPending || !budgetsSettled)
  return {
    isPending: dir.isPending || (on && cur.isPending),
    error: dir.error,
    summary,
    ratingsPending,
    costFailed,
    agentCount: agents.length,
    deployedCount,
    byId: dir.byId,
    displayName,
    p95,
    retry: () => void dir.refetch(),
    retryCost: () => {
      for (const q of [cur, prev, d30, ts, budgets, budgetStatus]) if (q.isError) void q.refetch()
    },
  }
}

/** 7d and 30d are the server's own ranges; 90d has none, so it sends explicit UTC days ending today. */
export function rangeWindow(range: Range, now: Date): ResolvedWindow {
  if (range !== '90d') return resolveWindow({ preset: range }, now)
  const to = now.toISOString().slice(0, 10)
  const from = new Date(now.getTime() - 89 * 86_400_000).toISOString().slice(0, 10)
  return resolveWindow({ preset: 'custom', from, to }, now)
}

/** The previous window of the same length, as a window of its own (its own query key). */
export function previousWindow(win: ResolvedWindow): ResolvedWindow {
  return {
    ...win,
    start: win.prevStart,
    end: win.prevEnd,
    params: win.prevParams,
    key: `${win.key}|previous`,
  }
}

export interface Spend {
  /** Month-to-date and forecast from the fleet-wide calendar (eng review R2): said as fleet spend. */
  summary: MonthSummary | null
  isPending: boolean
  error: unknown
  /** The range's length in days (for per-day averages and "last N days"). */
  rangeDays: number
  /** Fleet spend and runs (operations) over the range and the window before it, from the fleet-wide timeseries;
   *  `previous` is null while it loads or when it failed. */
  totals: {
    spend: number
    runs: number
    previous: { spend: number; runs: number } | null
  } | null
  /** The range's timeseries failed: no totals, no chart. */
  totalsError: unknown
  /** The range's UTC days, zero-filled (fleet-wide). */
  days: TimelinePoint[]
  /** Those days split by the top drivers plus Other; null while their series load, or when one failed (the chart
   *  then draws the fleet total alone). */
  stack: Stack | null
  /** The drivers or their series are still loading: the chart waits rather than redraw as a stack. */
  stackPending: boolean
  /** The 30-day peak when it is a spike (TokenOps' rule); may fall before the strip. */
  spike: Spike | null
  /** The top cost drivers over the range, ACL-scoped (only agents you can access). */
  drivers: AttributionRow[]
  /** Every other accessible agent, summed; null when there are none. `deltaPct` is null without a previous cost. */
  other: {
    count: number
    cost: number
    sharePct: number
    deltaPct: number | null
    unavailable: boolean
  } | null
  driversError: unknown
  /** Some calls in the range had no price: totals are a floor. */
  unpriced: boolean
  retry(): void
}

export function useSpend(now: Date, range: Range, enabled = true): Spend {
  const thisMonth = utcMonthStart(now)
  const lastMonth = utcMonthStart(now, -1)
  const win = useMemo(() => rangeWindow(range, now), [range, now])
  const winPrev = useMemo(() => previousWindow(win), [win])
  const calThis = useCalendar(monthKey(thisMonth), {}, enabled)
  const calLast = useCalendar(monthKey(lastMonth), {}, enabled)
  const cur = useDashboard(win, {}, 'agent', 'current', enabled)
  const prev = useDashboard(win, {}, 'agent', 'previous', enabled)
  const ts = useTimeseries(win, {}, enabled)
  const tsPrev = useTimeseries(winPrev, {}, enabled)
  // Just after a UTC month rollover the kept-previous calendar is last month's: loading, never summarised as this month.
  const stale = calThis.isPlaceholderData || calLast.isPlaceholderData
  const summary = useMemo(
    () =>
      calThis.data && calLast.data && !stale
        ? summarizeMonth(calThis.data.days, calLast.data.days, now)
        : null,
    [calThis.data, calLast.data, stale, now],
  )
  const days = useMemo(
    () =>
      ts.data?.bucket === 'day' ? zeroFillTimeline(ts.data.points, 'day', win.start, win.end) : [],
    [ts.data, win],
  )
  const totals = useMemo(() => {
    if (ts.data?.bucket !== 'day' || ts.isPlaceholderData) return null
    const sum = (pts: readonly { spend_usd: number; operations: number }[]) => ({
      spend: pts.reduce((n, p) => n + p.spend_usd, 0),
      runs: pts.reduce((n, p) => n + p.operations, 0),
    })
    const previous = tsPrev.data && !tsPrev.isPlaceholderData ? sum(tsPrev.data.points) : null
    return { ...sum(ts.data.points), previous }
  }, [ts.data, ts.isPlaceholderData, tsPrev.data, tsPrev.isPlaceholderData])
  const { drivers, other } = useMemo(() => {
    if (!cur.data) return { drivers: [], other: null }
    const prevRows = prev.isError ? undefined : prev.data?.attributions
    const all = buildAttribution(cur.data.attributions, prevRows).sort((a, b) => b.cost - a.cost)
    const top = all.slice(0, TOP_DRIVERS)
    const rest = all.slice(TOP_DRIVERS)
    if (!rest.length) return { drivers: top, other: null }
    const topIds = new Set(top.map((r) => r.id))
    const cost = rest.reduce((n, r) => n + r.cost, 0)
    // The previous window's cost of everyone outside the top: an agent new to the top was "other" before.
    const before =
      prevRows?.view === 'agent'
        ? prevRows.rows.reduce((n, r) => n + (topIds.has(r.agent_id) ? 0 : r.total_cost), 0)
        : null
    return {
      drivers: top,
      other: {
        count: rest.length,
        cost,
        sharePct: rest.reduce((n, r) => n + r.sharePct, 0),
        deltaPct: before ? ((cost - before) / before) * 100 : null,
        unavailable: before === null,
      },
    }
  }, [cur.data, prev.data, prev.isError])
  // Each driver's own days: the same timeseries filtered by agent, under TokenOps' keys (its filtered view opens warm).
  const withSpend = useMemo(() => drivers.filter((d) => d.cost > 0), [drivers])
  const parts = useQueries({
    queries: withSpend.map((d) => ({ ...timeseriesQuery(win, { agent: d.id }), enabled })),
    combine: combineSeries,
  })
  const stack = useMemo(() => {
    if (!withSpend.length || !days.length || parts.data.some((d) => d?.bucket !== 'day'))
      return null
    return stackSpend(
      days,
      withSpend.map((d, i) => ({
        id: d.id,
        name: d.name,
        days: zeroFillTimeline(parts.data[i]?.points ?? [], 'day', win.start, win.end),
      })),
    )
    // parts.data is structurally shared by combine, so it changes only when a series does.
  }, [withSpend, days, parts.data, win])
  return {
    summary,
    isPending: calThis.isPending || calLast.isPending || stale,
    error: calThis.error ?? calLast.error,
    rangeDays: RANGE_DAYS[range],
    totals,
    totalsError: ts.error,
    days,
    stack,
    stackPending: cur.isPending || parts.pending,
    spike: findSpike(days.map((p) => ({ date: p.iso.slice(0, 10), spend: p.spend }))),
    drivers,
    other,
    driversError: cur.error,
    unpriced: (cur.data?.summary.unpriced_calls ?? 0) > 0,
    retry: () => {
      void calThis.refetch()
      void calLast.refetch()
      void cur.refetch()
      void prev.refetch()
      void ts.refetch()
      void tsPrev.refetch()
      parts.refetch()
    },
  }
}

/** Module-level, so TanStack memoises it: `data` stays the same array until a series changes. */
function combineSeries(
  rs: {
    data: FinopsSpendTimeseries | undefined
    isError: boolean
    isPending: boolean
    refetch: () => unknown
  }[],
) {
  return {
    data: rs.map((r) => (r.isError ? undefined : r.data)),
    pending: rs.some((r) => r.isPending),
    refetch: () => rs.forEach((r) => r.isError && void r.refetch()),
  }
}

export interface NeedsYou {
  needs: Needs
  /** The oldest answer among the sources that answered (design review 8A's "Checked …"), or 0. */
  lastChecked: number
  retry: Record<SourceId, () => void>
  /** The newest sessions of the last 7 days and their checked status, shared with Recent sessions (eng review R7). */
  sessions: ReturnType<typeof useRecentSessions>
}

/** The newest STATUS_CHECK_ROWS sessions of the last 7 days, status-checked with the Sessions page's keys (eng review R7). */
function useRecentSessions(now: Date, enabled: boolean) {
  const win = useMemo(() => resolveWindow({ preset: '7d' }, now), [now])
  const start = useMemo(() => tempoSafeStart(win.start, now), [win.start, now])
  const list = useFleetSessions(win.key, start, win.end, enabled)
  const newest = useMemo(() => list.rows.slice(0, STATUS_CHECK_ROWS), [list.rows])
  const status = useSessionStatuses(newest, now.getTime(), false)
  return { list, newest, status }
}

export function useNeedsYou(now: Date, me: Me, fleet: FleetHealth, enabled = true): NeedsYou {
  const waiting = useWaiting(me.sub, me.is_superuser)
  const chats = useChatSessions()
  // The fleet's directory, not a second observer: one enabled late would refetch a failed list and flicker it pending.
  const dir = { byId: fleet.byId }
  // Budgets hidden: no server support for /api/budgets yet (R-L10). Restore the commented lines when it lands.
  // const budgets = useBudgets(enabled)
  // const budgetsAbsent = isEndpointAbsent(budgets.error)
  const budgets = useBudgets(false)
  const budgetsAbsent = true
  const status = useBudgetStatus(enabled && budgets.isSuccess)
  const sessions = useRecentSessions(now, enabled)
  // Stable inputs for the memo below: query result objects are new on every render.
  const { match, failedCold, isPending: waitingPending } = waiting
  const statusKey = sessions.newest.map((r) => sessions.status.get(r.session_id) ?? '').join('|')

  const input: NeedsInput = useMemo(() => {
    const titles = new Map(sessionRows(chats.data).map((r) => [r.session_id, r.title]))
    const requests: NeedsInput['requests'] = failedCold
      ? { state: 'failed' }
      : waitingPending
        ? { state: 'loading' }
        : {
            state: 'ok',
            value: {
              chats: match.chats.flatMap((c) => {
                const first = c.requests[0]
                if (!first) return []
                return {
                  sessionId: c.sessionId,
                  firstId: first.id,
                  kind: first.kind,
                  count: c.requests.length,
                  chatTitle: titles.get(c.sessionId) || copy.needs.untitledChat,
                  at: first.created_at,
                }
              }),
              outside: match.outside,
            },
          }
    // Answered only once every rating input settled, or a spike or stopped budget could land after "Nothing needs
    // you" (design review 5A; /ship review).
    // Failed only with no data: a failed background refresh keeps the cached ratings, as Fleet health does.
    const agents: NeedsInput['agents'] =
      fleet.error && !fleet.summary
        ? { state: 'failed' }
        : !fleet.summary || fleet.ratingsPending
          ? { state: 'loading' }
          : {
              state: 'ok',
              value: fleet.summary.action.map((a) => ({
                id: a.id,
                name: a.name,
                reason: a.reasons[0]?.text ?? '',
                budget: a.reasons[0]?.code === 'budgetStopped',
              })),
              // Rows are real, but with a cost input down the source can't vouch that nothing else needs action.
              incomplete: fleet.costFailed,
            }
    const budgetRows: NeedsInput['budgets'] = budgetsAbsent
      ? { state: 'absent' }
      : budgets.isError || status.isError
        ? { state: 'failed' }
        : budgets.isPending || status.isPending
          ? { state: 'loading' }
          : {
              state: 'ok',
              value: (budgets.data ?? []).flatMap((b) => {
                const st = status.data?.data.find((x) => x.budget_id === b.id)
                // A stopped agent budget is skipped only when its agent row already says so (a stopped, harness or unlisted
                // agent isn't rated, so the budget row must stay; /ship adversarial).
                const shownAsAgent =
                  !!b.agent_id &&
                  !!fleet.summary?.byId
                    .get(b.agent_id)
                    ?.reasons.some((r) => r.code === 'budgetStopped')
                if (!st || st.state === 'ok' || (st.stopped && shownAsAgent)) return []
                const agent = b.agent_id ? dir.byId.get(b.agent_id) : undefined
                return [
                  {
                    id: b.id,
                    label: b.agent_id
                      ? agent?.display_name || agent?.name || copy.needs.anAgent
                      : copy.needs.yourBudget,
                    crossed: st.crossed,
                    state: st.state === 'warning' ? 'warning' : 'exceeded',
                  } as const,
                ]
              }),
            }
    const checking = sessions.newest.some((r) => sessions.status.get(r.session_id) === 'checking')
    const failedRows = sessions.newest.filter((r) => sessions.status.get(r.session_id) === 'failed')
    // Every check came back unknown (e.g. no trace store: session details answer 503): nothing was actually checked,
    // so the source failed; it must never count as "no failures" (design 5A, QA ISSUE-005).
    const settled = sessions.newest.filter((r) =>
      ['ok', 'failed'].includes(sessions.status.get(r.session_id) ?? ''),
    ).length
    // Most of the sample must be checked for "no failures" to count (user decision, /ship adversarial finding 7);
    // failures actually found always show.
    const noneChecked =
      !checking &&
      !failedRows.length &&
      sessions.newest.length > 0 &&
      settled * 2 < sessions.newest.length
    const sessionState: NeedsInput['sessions'] =
      sessions.list.isError || noneChecked
        ? { state: 'failed' }
        : sessions.list.isPending || checking
          ? { state: 'loading' }
          : {
              state: 'ok',
              value: {
                failed: failedRows.length,
                // "N of M checked sessions failed": the sessions actually checked (user decision, /ship adversarial).
                checked: settled,
                agents: [
                  ...new Set(
                    failedRows.map((r) =>
                      r.agent_id ? fleet.displayName(r.agent_id) : copy.needs.unknownAgent,
                    ),
                  ),
                ],
              },
            }
    return { requests, agents, budgets: budgetRows, sessions: sessionState }
    // statusKey stands in for sessions.status (a new Map every render).
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [
    match,
    failedCold,
    waitingPending,
    chats.data,
    fleet.summary,
    fleet.error,
    fleet.ratingsPending,
    fleet.costFailed,
    fleet.displayName,
    budgetsAbsent,
    budgets.data,
    budgets.isError,
    budgets.isPending,
    status.data,
    status.isError,
    status.isPending,
    dir.byId,
    sessions.newest,
    sessions.list.isError,
    sessions.list.isPending,
    statusKey,
  ])

  const needs = useMemo(() => mergeNeeds(input), [input])
  const stamps = [
    waiting.lastChecked,
    budgetsAbsent ? 0 : status.dataUpdatedAt,
    sessions.list.dataUpdatedAt,
  ].filter((t) => t > 0)
  return {
    needs,
    lastChecked: stamps.length ? Math.min(...stamps) : 0,
    retry: {
      requests: waiting.retry,
      // With a cost input down the agents source is incomplete: its Retry must refetch those too.
      agents: () => {
        fleet.retry()
        if (fleet.costFailed) fleet.retryCost()
      },
      budgets: () => {
        void budgets.refetch()
        void status.refetch()
      },
      sessions: () => void sessions.list.refetch(),
    },
    sessions,
  }
}

export interface HarnessLine {
  id: string
  cost: number | null
  /** More than half the turns unpriced: the cost reads "unpriced" (the Harnesses page rule). */
  unpriced: boolean
}

export interface HarnessSummary {
  isPending: boolean
  error: unknown
  /** The org endpoint is absent (a bare 404): these are the viewer's own numbers (plans/feat-harness-org-view.md §4). */
  ownOnly: boolean
  /** A coded 404: nothing visible at the landing scope. */
  notVisible: boolean
  connected: number
  activeDevs: number | null
  harnesses: HarnessLine[]
  retry(): void
}

/** Coding harnesses over the range: the landing request (no scope, the server picks), else the viewer's own usage. */
export function useHarnessSummary(now: Date, me: Me, range: Range): HarnessSummary {
  const win = useMemo(() => rangeWindow(range, now), [range, now])
  const usage = useUsage(me.sub, { ...win.params }, win.key, true)
  const absent = isEndpointAbsent(usage.error)
  const live = useLiveIndividual(me.sub, win, false, absent)
  if (absent) {
    const h = live.data?.harnesses.filter((x) => x.registered) ?? []
    return {
      isPending: live.loading || (!live.data && !live.error),
      error: live.error,
      ownOnly: true,
      notVisible: false,
      connected: h.length,
      activeDevs: null,
      harnesses: h
        .map((x) => ({ id: x.harness, cost: x.cost_usd, unpriced: false }))
        .sort((a, b) => (b.cost ?? 0) - (a.cost ?? 0)),
      retry: live.refetch,
    }
  }
  // The landing request has no group_by=user, so its first page holds the whole answer (no cursor).
  const d = usage.data?.pages[0]
  const by = (d?.by_harness ?? []).filter((x) => x.registered_devs > 0 || x.turns > 0)
  return {
    isPending: usage.isPending,
    error: isNotVisible(usage.error) ? null : usage.error,
    ownOnly: false,
    notVisible: isNotVisible(usage.error),
    connected: by.length,
    activeDevs: d?.totals.active_devs ?? null,
    harnesses: by
      .map((x) => ({ id: x.harness, cost: x.cost_usd, unpriced: mostlyUnpriced(x) }))
      .sort((a, b) => (b.cost ?? 0) - (a.cost ?? 0)),
    retry: () => void usage.refetch(),
  }
}

export interface BudgetLine {
  id: string
  /** The agent's display name; null for the viewer's own monthly budget. */
  name: string | null
  limit: number
  used: number
  /** Bar fill 0-100, capped at the limit. */
  pct: number
  state: BudgetState
  stopped: boolean
  thresholds: number[]
  action: BudgetAction
  forecast: Forecast
  /** Whole days until the period resets, from when the status was read. */
  daysLeft: number
}

export interface BudgetCard {
  /** No `/api/budgets` on this server (a bare 404): the card isn't shown. */
  absent: boolean
  isPending: boolean
  error: unknown
  /** The viewer's own monthly budget, if set. */
  own: BudgetLine | null
  agents: BudgetLine[]
  retry(): void
}

/** The Budgets card: the same list and status reads as Needs you and Fleet health (one fetch each). */
export function useBudgetCard(fleet: FleetHealth, enabled: boolean): BudgetCard {
  const budgets = useBudgets(enabled)
  const status = useBudgetStatus(enabled && budgets.isSuccess)
  const absent = isEndpointAbsent(budgets.error)
  const lines = useMemo(() => {
    // The forecast's clock is when the status was read, never the page's frozen `now` (plans/feat-llm-router.md §5.1).
    const read = new Date(status.dataUpdatedAt)
    const byId = new Map((status.data?.data ?? []).map((s) => [s.budget_id, s]))
    return (budgets.data ?? []).flatMap((b): BudgetLine[] => {
      const st = byId.get(b.id)
      if (!st) return []
      const agent = b.agent_id ? fleet.byId.get(b.agent_id) : undefined
      return [
        {
          id: b.id,
          name:
            b.scope === 'owner' ? null : agent?.display_name || agent?.name || copy.needs.anAgent,
          limit: b.limit_usd,
          used: st.used_usd,
          pct: usedPercent(st.used_usd, b.limit_usd),
          state: st.state,
          stopped: st.stopped,
          thresholds: b.thresholds,
          action: b.action,
          forecast: budgetForecast(st, b.limit_usd, read),
          daysLeft: Math.max(0, Math.ceil((Date.parse(st.resets_at) - read.getTime()) / DAY)),
        },
      ]
    })
  }, [budgets.data, status.data, status.dataUpdatedAt, fleet.byId])
  return {
    absent,
    isPending: !absent && (budgets.isPending || (budgets.isSuccess && status.isPending)),
    error: absent ? null : (budgets.error ?? status.error),
    own: lines.find((l) => l.name === null) ?? null,
    // Most used first: the one nearest its limit leads.
    agents: lines.filter((l) => l.name !== null).sort((a, b) => b.pct - a.pct),
    retry: () => {
      void budgets.refetch()
      void status.refetch()
    },
  }
}

/**
 * Token-optimisation savings for the header's range.
 *
 * Reuses TokenOps' own query and window so the two pages can never disagree about a figure, and so
 * the 90-day range works — the endpoint's `range` shorthand only understands 24h/7d/30d, while a
 * resolved window carries explicit timestamps.
 */
export function useOverviewSavings(now: Date, range: Range, enabled = true) {
  const win = useMemo(() => rangeWindow(range, now), [range, now])
  const q = useSavings(win, {}, 'total', enabled)
  const view = useMemo(() => (q.data ? summarizeOptimisation(q.data) : undefined), [q.data])
  return { view, isPending: q.isPending, isError: q.isError }
}
