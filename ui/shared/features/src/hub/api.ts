/**
 * The agent hub's reads (plans/feat-agent-centric.md eng "Gate", "Load"), in one module that owns the gate and every
 * key that depends on it. Keys live under `['agents', 'hub', …]`, never TokenOps' own, so a warm TokenOps cache can't
 * render through the gate.
 *
 * The gate is a frontend gate: the finops timeseries is fleet-wide on the server (not ACL-scoped), so the hub reads
 * the ACL-scoped dashboard (`view=agent`, `agent_id`) first and asks for the timeseries and savings only after a fresh
 * success for this exact agent and window. Cached or placeholder data never authorizes; a failed revalidation, a
 * window change while pending, or a late answer for another key all leave the dependent reads disabled and unrendered.
 * While the gate revalidates (a focus refetch, a retry, an invalidation of `['agents', …]`) the dependent reads are
 * disabled, and any dependent data fetched after the gate's last success is withheld until the gate answers again
 * (`GateResult.authorizes`): a refetch racing a revoked grant never draws (review: Codex P1).
 * The server-side gap is a written ask (AH-P0 in docs/designs/openruntime-agent-hub-recommendations.md).
 */
import { useSlots } from '@nasiko/app-core/edition/edition-context'
import { useQuery, useQueryClient } from '@tanstack/react-query'
import {
  isUnavailable,
  useAgentsDirectory,
  useDeployment,
  useVersions,
} from '@nasiko/features/agents/api'
import { displayStatus } from '@nasiko/features/agents/status'
import {
  historyPages,
  useChatHistory,
  useChatSessions,
  usePendingRequests,
} from '@nasiko/features/chat/api'
import { pendingRequestRows, useWaiting } from '@nasiko/features/chat/waiting'
import { useFlowsList } from '@nasiko/features/flows/api'
import type { ChatSessionRow, CursorPage } from '@nasiko/features/chat/types'
import type { SessionListResponse } from '@nasiko/features/observability/types'
import type { Status } from '@nasiko/features/observability/sessions'
import { useFleetHealth } from '@nasiko/features/overview/api'
import { rateAgent } from '@nasiko/features/overview/health'
import { useDashboard } from '@nasiko/features/tokenops/api'
import { useSessionStatuses } from '@nasiko/features/sessions/api'
import { useAgentBuilds } from '@nasiko/features/deploy/api'
import { contextSavingsSchema } from '@nasiko/features/optimization/types'
import { finopsDashboardSchema } from '@nasiko/features/tokenops/types'
import type {
  FinopsDashboardData,
  FinopsSpendTimeseries,
  SavingsBasis,
  SavingsData,
} from '@nasiko/features/tokenops/types'
import { resolveWindow, type ResolvedWindow } from '@nasiko/features/tokenops/window'
import { ApiError, apiData, apiFetch, withQuery } from '@nasiko/data-access/client'
import { isPreview } from '@nasiko/app-core/env'
import { markersFor, type MarkerSet } from './events'
import { hubNeeds, sessionSource, type HubNeeds, type HubNeedsInput, type HubSource } from './needs'
import { agentRows, sampleStart } from './sessions'
import { EVENTS_READ, SESSION_CHECKS, SESSION_SAMPLE } from './tuning'
import { buildLedger } from './ledger'
import { agentEventsSchema, type AgentEvents } from './types'

const FINOPS = '/api/observability/finops'

const hubKeys = {
  all: (id: string) => ['agents', 'hub', id] as const,
  dashboard: (id: string, w: string) => ['agents', 'hub', id, 'dashboard', w] as const,
  timeseries: (id: string, w: string) => ['agents', 'hub', id, 'timeseries', w] as const,
  savings: (id: string, w: string) => ['agents', 'hub', id, 'savings', w] as const,
  savedSeries: (id: string, w: string) => ['agents', 'hub', id, 'saved-series', w] as const,
  events: (id: string, w: string) => ['agents', 'hub', id, 'events', w] as const,
  sessions: (id: string, start: string) => ['agents', 'hub', id, 'sessions', start] as const,
}

type Win = Pick<
  ResolvedWindow,
  'key' | 'params' | 'prevParams' | 'prevStart' | 'prevEnd' | 'start' | 'end'
>

function dashboardPath(id: string, win: Win) {
  return withQuery(`${FINOPS}/dashboard`, { ...win.params, agent_id: id, view: 'agent' })
}

/** The gate's read. */
function hubDashboardQuery(id: string, win: Win) {
  const path = dashboardPath(id, win)
  return {
    queryKey: hubKeys.dashboard(id, win.key),
    queryFn: ({ signal }: { signal: AbortSignal }) =>
      apiData<FinopsDashboardData>(path, { signal, schema: finopsDashboardSchema }),
    meta: { path },
  }
}

export interface HubMoney {
  current: number
  previous: number
  changePct: number | null
  requests: number
  prevRequests: number
  requestsChangePct: number | null
  /** Some calls had no price: the spend is a floor. */
  unpriced: boolean
}

export type GateState =
  | { kind: 'pending' }
  | { kind: 'ok'; money: HubMoney }
  /** The dashboard's 404 "agent not found": the viewer can't see this agent's finops. */
  | { kind: 'not-visible' }
  /** A 400: the server doesn't know the agent (unknown or soft-deleted). */
  | { kind: 'deleted' }
  | { kind: 'error'; error: unknown }

export interface GateResult {
  state: GateState
  retry: () => void
  /** Dependent reads may run: authorized and not revalidating. */
  canFetch: boolean
  /** Dependent data updated at `at` may render: authorized, and not newer than an unfinished revalidation. */
  authorizes: (at: number) => boolean
}

export function useHubGate(id: string, win: Win): GateResult {
  const qc = useQueryClient()
  const q = useQuery({
    ...hubDashboardQuery(id, win),
    // Revalidate on every visit: only a response fetched since this page asked for this key authorizes
    // (`isFetchedAfterMount`, reset when the key changes), so an earlier visit's cache never does.
    refetchOnMount: 'always',
    staleTime: 0,
  })
  // Re-runs the gate (it is active, so it refetches at once), then the dependent reads as they enable again.
  const retry = () => void qc.invalidateQueries({ queryKey: hubKeys.all(id) })
  let state: GateState
  if (q.isError) {
    const err = q.error
    state =
      err instanceof ApiError && err.status === 404
        ? { kind: 'not-visible' }
        : err instanceof ApiError && err.status === 400
          ? { kind: 'deleted' }
          : { kind: 'error', error: err }
  } else if (q.isSuccess && q.isFetchedAfterMount) {
    const k = q.data.kpis
    state = {
      kind: 'ok',
      money: {
        current: k.total_spend.current,
        previous: k.total_spend.previous,
        changePct: k.total_spend.change_pct,
        requests: k.total_operations.current,
        prevRequests: k.total_operations.previous,
        requestsChangePct: k.total_operations.change_pct,
        unpriced: q.data.summary.unpriced_calls > 0,
      },
    }
  } else state = { kind: 'pending' }
  const ok = state.kind === 'ok'
  const revalidating = q.isFetching
  const confirmedAt = q.dataUpdatedAt
  return {
    state,
    retry,
    canFetch: ok && !revalidating,
    // Strictly older: data stamped in the same millisecond as the gate's success can't be shown to predate it.
    authorizes: (at) => ok && (!revalidating || at < confirmedAt),
  }
}

/** The timeseries for this agent; `which: 'previous'` is the compare ghost (7d/30d and other daily windows only). */
export function useHubTimeseries(
  id: string,
  win: Win,
  which: 'current' | 'previous',
  enabled: boolean,
) {
  const params = which === 'current' ? win.params : win.prevParams
  const key =
    which === 'current'
      ? win.key
      : `prev|${win.prevStart.toISOString()}|${win.prevEnd.toISOString()}`
  const path = withQuery(`${FINOPS}/spend-timeseries`, { ...params, agent_id: id })
  return useQuery({
    queryKey: hubKeys.timeseries(id, key),
    queryFn: ({ signal }) => apiData<FinopsSpendTimeseries>(path, { signal }),
    enabled,
    meta: { path },
  })
}

export interface SavedTotal {
  saved: number
  basis: SavingsBasis
}

/**
 * This agent's row of `/finops/savings` (`scope=agent`, `agent_id`) for the window: the window total and its basis. No
 * row means "not reported" (null), never 0. `agent_id` is ACL-checked by the server (handler.rs `get_finops_savings`,
 * b2a966332) and keeps the agent inside the default 25-row `by_agent` cap; the row is still matched by id, so a server
 * that ignores the filter answers the same way.
 */
export function useHubSavings(id: string, win: Win, enabled: boolean) {
  const path = withQuery(`${FINOPS}/savings`, { ...win.params, scope: 'agent', agent_id: id })
  return useQuery({
    queryKey: hubKeys.savings(id, win.key),
    queryFn: ({ signal }) => apiData<SavingsData>(path, { signal }),
    select: (s): SavedTotal | null => {
      const row = s.by_agent.find((a) => a.agent_id === id)
      return row ? { saved: row.saved_tokens, basis: row.basis } : null
    },
    enabled,
    meta: { path },
  })
}

/**
 * The per-agent savings series (proposed CX-V3a), a preview: read only while the `optimization` mocks answer
 * (`isPreview`), whatever the server says. Live mode has no daily savings, so the lane header shows the total alone.
 */
export function useHubSavedSeries(id: string, win: Win, bucket: 'hour' | 'day', enabled: boolean) {
  const path = withQuery(`${FINOPS}/context-savings`, {
    ...win.params,
    agent_id: id,
    series: bucket === 'hour' ? 'hourly' : 'daily',
  })
  return useQuery({
    queryKey: hubKeys.savedSeries(id, win.key),
    queryFn: ({ signal }) => apiData(path, { signal, schema: contextSavingsSchema }),
    select: (s) => s.series?.points ?? [],
    enabled: enabled && isPreview('optimization'),
    meta: { path },
  })
}

/**
 * The proposed event log (AH-3), a preview: read only while the `events` mocks answer. Under `/api/observability`, never
 * `/api/agents/{id}/…`: that prefix is the A2A proxy (lib.rs `/agents/{id}/{*rest}`), where the agent itself would
 * answer. Any 404 (no such route, or no such agent) means no events to draw.
 */
function useHubEvents(id: string, win: Win) {
  const path = withQuery(`/api/observability/agent/${encodeURIComponent(id)}/events`, {
    start_time: win.start.toISOString(),
    end_time: win.end.toISOString(),
    limit: EVENTS_READ,
  })
  return useQuery({
    queryKey: hubKeys.events(id, win.key),
    queryFn: async ({ signal }): Promise<AgentEvents | null> => {
      try {
        return await apiData(path, { signal, schema: agentEventsSchema })
      } catch (err) {
        if (err instanceof ApiError && err.status === 404) return null
        throw err
      }
    },
    enabled: isPreview('events'),
    meta: { path: '/api/observability/agent/{id}/events' },
  })
}

export type MarkersState =
  | { kind: 'pending' }
  | { kind: 'ready'; set: MarkerSet; eventsTruncated: boolean; preview: boolean }
  /** The deploy reads failed; the event log's markers (if any) still draw. */
  | { kind: 'failed'; which: 'deploys' | 'changes'; retry: () => void; set: MarkerSet | null }

/**
 * Markers from the builds (newest 20), versions, the current deployment's `crashed_at` (EE writes it) and, as a
 * preview, the event log. Independent of the gate: none of them carry money, and each has its own ACL.
 */
export function useHubMarkers(id: string, win: Win, harness: boolean): MarkersState {
  const builds = useAgentBuilds(id, !harness)
  const versions = useVersions(id, !harness)
  const deployment = useDeployment(id, !harness)
  const events = useHubEvents(id, win)
  const preview = isPreview('events')
  const deploysPending =
    !harness && (builds.isPending || versions.isPending || deployment.isPending)
  // A failed deployment read hides the crash marker: that is a failed source too, never "no crash" (review).
  const deploysFailed = !harness && (builds.isError || versions.isError || deployment.isError)
  if (deploysPending || (preview && events.isPending)) return { kind: 'pending' }
  const buildRecords = harness ? null : builds.data?.available ? builds.data.value : null
  const dep = deployment.data
  const crashedAt = dep && !isUnavailable(dep) ? (dep.crashed_at ?? null) : null
  const set = markersFor({
    builds: buildRecords,
    versions: harness ? null : (versions.data ?? null),
    crashedAt,
    events: events.data?.events ?? null,
    win,
    harness,
  })
  if (deploysFailed)
    return {
      kind: 'failed',
      which: 'deploys',
      // Only the event log's markers: with the deploy reads failed, nothing is said about builds or versions.
      set: events.data
        ? markersFor({
            builds: null,
            versions: null,
            crashedAt: null,
            events: events.data.events,
            win,
            harness: true,
          })
        : null,
      retry: () => {
        if (builds.isError) void builds.refetch()
        if (versions.isError) void versions.refetch()
        if (deployment.isError) void deployment.refetch()
      },
    }
  if (preview && events.isError)
    return { kind: 'failed', which: 'changes', set, retry: () => void events.refetch() }
  return {
    kind: 'ready',
    set,
    eventsTruncated: !!events.data?.truncated,
    preview: preview && !!events.data,
  }
}

// ─── PR1c: this agent's sessions, flows and Needs you ─────────────────────────────────────────────

/**
 * The hub's session sample (`sessions.ts`): one `session/list` read of the newest SESSION_SAMPLE rows from
 * max(created, 7 days back), this agent's rows kept. Coverage travels with it, so the lists say what they checked.
 */
export function useHubSessions(
  agent: { id: string; name: string; createdAt: string },
  now: Date,
  enabled = true,
) {
  const start = sampleStart(agent.createdAt, now)
  const path = withQuery('/api/observability/session/list', {
    start_time: start.toISOString(),
    limit: SESSION_SAMPLE,
    offset: 0,
  })
  return useQuery({
    queryKey: hubKeys.sessions(agent.id, start.toISOString()),
    queryFn: ({ signal }) => apiFetch<SessionListResponse>(path, { signal }),
    enabled: enabled && !!agent.name,
    select: (r) => {
      const all = r?.data?.sessions ?? []
      return { rows: agentRows(all, agent.name, start), scanned: all.length }
    },
    meta: { path: '/api/observability/session/list' },
  })
}

/**
 * Flows started by this agent: the Flows page's list read back to the window start, filtered by `root_agent_id` (a
 * flow's steps carry no agent id, FL-9, so "flows that call this agent" needs FL-12).
 */
export function useHubFlows(id: string, win: Win) {
  const list = useFlowsList({ sinceMs: win.start.getTime() })
  const rows = list.data?.rows ?? []
  const end = win.end.getTime()
  return {
    ...list,
    // The list reads back from the newest flow: a past window drops flows created after its end (review: Codex).
    mine: rows.filter((f) => f.root_agent_id === id && Date.parse(f.created_at) < end),
    read: rows.length,
    complete: !!list.data?.complete,
    /** A later page failed: what was read shows, with the failure and Retry (`readFlowsBack` keeps partial rows). */
    partial: !!list.data?.error,
  }
}

export interface HubNeedsResult {
  needs: HubNeeds
  retry: Record<HubSource, () => void>
  sessions: ReturnType<typeof useHubSessions>
  statuses: ReadonlyMap<string, string>
}

type Rated = HubNeedsInput['status']

/**
 * This agent's rating (eng "Health input contract", per viewer). A superuser keeps the Overview's own inputs
 * (`useFleetHealth`: fleet dashboards and the fleet spend series for spikes). Anyone else is rated from this agent's
 * ACL-scoped dashboards only, with no spike dimension: the fleet spend series isn't ACL-scoped on the server (AH-P0), so
 * a member's visit must never read it (review: Codex P1). The hub's gate decides first: an agent whose spend the viewer
 * can't see, or that the server doesn't know, isn't rated here.
 */
function useAgentRating(
  agent: {
    id: string
    name: string
    status: string
    createdAt: string
    updatedAt: string
    isHarness: boolean
  },
  superuser: boolean,
  gate: GateState['kind'],
  now: Date,
): { state: Rated; retry: () => void } {
  const on = !agent.isHarness && gate === 'ok'
  const fleet = useFleetHealth(now, on && superuser)
  const win7 = resolveWindow({ preset: '7d' }, now)
  const win30 = resolveWindow({ preset: '30d' }, now)
  const f = { agent: agent.id }
  const member = on && !superuser
  const cur = useDashboard(win7, f, 'agent', 'current', member)
  const prev = useDashboard(win7, f, 'agent', 'previous', member)
  const d30 = useDashboard(win30, f, 'agent', 'current', member)
  if (agent.isHarness || gate === 'not-visible' || gate === 'deleted' || gate === 'error')
    return { state: { state: 'absent' }, retry: () => {} }
  if (gate === 'pending') return { state: { state: 'loading' }, retry: () => {} }
  if (superuser) {
    const rated = fleet.summary?.byId.get(agent.id)
    const state: Rated =
      fleet.error && !fleet.summary
        ? { state: 'failed' }
        : !fleet.summary || fleet.ratingsPending
          ? { state: 'loading' }
          : rated
            ? {
                state: 'ok',
                value: {
                  rating: rated.rating,
                  reason: rated.reasons[0]?.text ?? null,
                  incomplete: fleet.costFailed,
                },
              }
            : { state: 'absent' }
    return {
      state,
      retry: () => {
        fleet.retry()
        if (fleet.costFailed) fleet.retryCost()
      },
    }
  }
  const retry = () => {
    for (const q of [cur, prev, d30]) if (q.isError) void q.refetch()
  }
  // A dashboard 404 is the viewer not seeing this agent's spend: nothing to rate (the gate says so too).
  if ([cur, prev, d30].some((q) => q.error instanceof ApiError && q.error.status === 404))
    return { state: { state: 'absent' }, retry }
  if (cur.isError) return { state: { state: 'failed' }, retry }
  if (cur.isPending || prev.isPending || d30.isPending)
    return { state: { state: 'loading' }, retry }
  const usage = (d: FinopsDashboardData | undefined) => {
    const r = d?.agents.find((a) => a.agent_id === agent.id)
    return r
      ? { cost: r.total_cost, operations: r.operations, p95: r.avg_latency_p95_ms ?? null }
      : ZERO_USAGE
  }
  const rated = rateAgent(
    {
      id: agent.id,
      name: agent.name,
      display: displayStatus(agent.status, false),
      raw: agent.status,
      createdAt: agent.createdAt,
      updatedAt: agent.updatedAt,
      current: usage(cur.data),
      previous: prev.isError ? null : usage(prev.data),
      costPerOp30d:
        d30.data?.agents.find((a) => a.agent_id === agent.id)?.avg_cost_per_operation ?? null,
    },
    now.getTime(),
  )
  return {
    state: {
      state: 'ok',
      value: {
        rating: rated.rating,
        reason: rated.reasons[0]?.text ?? null,
        incomplete: prev.isError || d30.isError,
      },
    },
    retry,
  }
}

const ZERO_USAGE = { cost: 0, operations: 0, p95: null }

/**
 * Needs you for one agent (`needs.ts`): requests, the agent's rating and its sessions' failures. `now` is Needs you's
 * own frozen clock, never the timeline's window, so a window change refetches nothing here (review: performance).
 */
export function useHubNeeds(
  agent: {
    id: string
    name: string
    status: string
    createdAt: string
    updatedAt: string
    isHarness: boolean
    ownerId: string
  },
  me: { sub: string; is_superuser: boolean } | undefined,
  gate: GateState['kind'],
  now: Date,
): HubNeedsResult {
  const qc = useQueryClient()
  const superuser = !!me?.is_superuser
  const waiting = useWaiting(me?.sub ?? '', superuser)
  // The same query `useWaiting` runs (one key): its rows carry the agent, which the match drops.
  const pending = usePendingRequests(me?.sub ?? '', !!me)
  const chats = useChatSessions()
  const rating = useAgentRating(agent, superuser, gate, now)
  const dir = useAgentsDirectory()
  const sessions = useHubSessions(agent, now)
  const checkedRows = (sessions.data?.rows ?? []).slice(0, SESSION_CHECKS)
  const statuses = useSessionStatuses(checkedRows, now.getTime(), false)
  // Budgets are Enterprise only: an OSS build reads nothing and the source is absent.
  const budget = useSlots().budgets.useAgentBudgetNeed(agent, me)

  // This agent's pending requests; a superuser's count only once a chat proves them theirs (chat's Waiting rule), and
  // only this agent's unprovable ones hold the verdict back: another agent's say nothing about this one (ISSUE-007).
  const forAgent = pendingRequestRows(superuser, waiting.match, pending.data?.items).filter(
    (r) => r.agentId === agent.id,
  )
  const all = forAgent.filter((r) => r.proven)
  const unproven = forAgent.filter((r) => !r.proven).length
  // Matching needs the chat list (it proves a superuser's requests): pending or failed, the source can't answer yet.
  const requests: HubNeedsInput['requests'] =
    waiting.failedCold || waiting.stale || (superuser && chats.isError)
      ? { state: 'failed' }
      : !me || waiting.isPending || chats.isPending || pending.isPending
        ? { state: 'loading' }
        : {
            state: 'ok',
            value: {
              count: all.length,
              firstChat: all.find((r) => r.chat)?.chat ?? null,
              firstRequest: all.find((r) => r.chat)?.id ?? null,
              unproven,
            },
          }

  const sessionState = sessionSource({
    listError: sessions.isError,
    listPending: sessions.isPending,
    found: sessions.data?.rows.length ?? 0,
    scanned: sessions.data?.scanned ?? 0,
    verdicts: checkedRows.map((r) => statuses.get(r.session_id) as Status | undefined),
    ambiguous: (dir.byNameAll.get(agent.name)?.length ?? 0) > 1,
  })

  return {
    needs: hubNeeds({
      requests,
      status: rating.state,
      sessions: sessionState,
      budget: budget.state,
    }),
    retry: {
      requests: () => {
        waiting.retry()
        if (chats.isError) void chats.refetch()
      },
      status: rating.retry,
      budget: budget.retry,
      // The list and the checks that failed: detail and trace reads never retry on their own (review).
      sessions: () => {
        void sessions.refetch()
        for (const r of checkedRows)
          void qc.invalidateQueries({ queryKey: ['sessions', 'detail', r.session_id] })
        void qc.invalidateQueries({
          predicate: (q) => q.queryKey[0] === 'trace' && q.state.status === 'error',
        })
      },
    },
    sessions,
    statuses,
  }
}

// ─── the harness session ledger (PR1b) ─────────────────────────────────────

/**
 * The session's recorded messages, newest 100 first (chat's own history query, so an open chat and its ledger share one
 * cache). The messages route answers only the chat's owner (no superuser bypass, `chat/routes.rs` `list_messages`): a
 * 404 means "not yours or not a chat", and the ledger shows metadata only. Older pages load on request.
 */
export function useLedgerHistory(sessionId: string) {
  const q = useChatHistory(sessionId)
  const pages = historyPages(q.data)
  // From the pages held, not the query's status: a failed background refresh keeps them, and must neither make the
  // ledger a "subtotal" nor hide that it failed (review: Codex, adversarial).
  const complete = !!q.data && !q.hasNextPage
  // The React Compiler memoises this on the query's data (the module is compiled).
  const ledger = q.data ? buildLedger(pages, { complete, preview: isPreview('chat') }) : null
  // A 404 before anything loaded: not a chat this viewer owns (or no chat at all). Once pages are held, a later 404
  // is a refresh failure, and the ledger it already showed is withheld, never kept exportable (review: Codex).
  const denied = q.error instanceof ApiError && q.error.status === 404
  return {
    query: q,
    ledger: denied ? null : ledger,
    complete,
    notOwner: denied,
    refreshFailed: !denied && q.isRefetchError,
  }
}

/**
 * A harness owner's own recorded sessions with this harness (`/api/chat/sessions?agent_id=`, own-only on the server):
 * exact, where the fleet-wide session sample would miss any but the newest. Only the owner can open a session's
 * ledger (messages are owner-only), so the hub lists these for the owner and keeps the sample for anyone else.
 */
export function useHarnessChats(agentId: string, enabled: boolean) {
  return useQuery({
    queryKey: ['agents', 'hub', agentId, 'chats'],
    queryFn: async ({ signal }) => {
      const body = await apiFetch<Partial<CursorPage<ChatSessionRow>>>(
        withQuery('/api/chat/sessions', { agent_id: agentId, limit: SESSION_SAMPLE }),
        { signal },
      )
      return Array.isArray(body?.data) ? body.data : []
    },
    enabled,
    meta: { path: '/api/chat/sessions' },
  })
}
