/**
 * Live fallback (plan §5): when the usage endpoint is absent (OSS, or an older EE server),
 * the viewer's OWN Individual level is rebuilt from existing endpoints:
 *   - GET /finops/dashboard?my_agent=true, current and previous window (live agents only);
 *   - GET /api/agents?owner=<me>, pre-filtered by tags/metadata, then each candidate is
 *     CONFIRMED via GET /api/agents/{id} coding_agent_integration_id (metadata is spoofable);
 *   - GET /api/chat/sessions?agent_id=<id>&limit=LIVE_SESSION_LIMIT per confirmed harness agent
 *     (own-only), merged by updated_at like the server orders them: an unfiltered list lets
 *     ordinary chats push harness sessions out.
 * Top models and the calendar strip have no source and are hidden, never zero-filled.
 */
import { useQueries, useQuery } from '@tanstack/react-query'
import {
  finopsDashboardRowsSchema,
  type AgentFinopsRow,
  type FinopsDashboardData,
} from '@/features/tokenops/types'
import type { ResolvedWindow } from '@/features/tokenops/window'
import { ApiError, apiData, apiFetch, withQuery } from '@/lib/api/client'
import { harnessKeys } from './api'
import { ACTIVE_MIN_TURNS, AGENT_PAGE_CAP, AGENT_PAGE_SIZE, LIVE_SESSION_LIMIT } from './constants'
import { orderHarnesses } from './rollup'
import type { HarnessId } from './types'

export interface CatalogAgent {
  id: string
  name: string
  owner_id?: string
  tags?: string[]
  metadata?: Record<string, unknown>
}

export interface ChatSessionRow {
  session_id: string
  agent_id: string | null
  created_at: string
  updated_at: string
  is_coding_agent: boolean
  message_count: number | null
  total_tokens: number | null
}

interface LiveHarness {
  harness: HarnessId
  registered: boolean
  active: boolean
  cost_usd: number
  tokens: number
  turns: number
  /** Turns change vs the previous window; null when unknown or previous was 0. */
  delta_pct: number | null
}

export interface LiveIndividual {
  harnesses: LiveHarness[]
  totals: {
    registered: number
    active: number
    idle: number
    cost_usd: number
    tokens: number
    turns: number
  }
  sessions: {
    session_id: string
    harness: HarnessId
    started_at: string
    messages: number | null
    tokens: number | null
  }[]
  prevUnavailable: boolean
}

/** Candidates the CLI registered (tags/metadata). Only a detail-call confirmation makes them harness agents. */
export function harnessCandidates(agents: CatalogAgent[]): CatalogAgent[] {
  return agents.filter(
    (a) => a.tags?.includes('coding-agent') || a.metadata?.source === 'nasiko-cli-integration',
  )
}

export function buildLiveIndividual(input: {
  /** agent id → confirmed coding_agent_integration_id (null = not a harness agent). */
  confirmed: Map<string, string | null>
  current: Pick<FinopsDashboardData, 'agents' | 'summary'>
  previous: Pick<FinopsDashboardData, 'agents'> | null
  prevFailed: boolean
  sessions: ChatSessionRow[]
}): LiveIndividual {
  const harnessOf = (agentId: string | null | undefined) =>
    agentId ? (input.confirmed.get(agentId) ?? null) : null
  const sum = (rows: AgentFinopsRow[], h: string, f: (r: AgentFinopsRow) => number) =>
    rows.filter((r) => harnessOf(r.agent_id) === h).reduce((n, r) => n + f(r), 0)
  const registered = new Set([...input.confirmed.values()].filter((h): h is string => !!h))
  // Dashboard rows for agents that aren't confirmed harness agents are ignored (no "Other" bucket, N3).
  const seen = new Set(
    input.current.agents.map((r) => harnessOf(r.agent_id)).filter((h): h is string => !!h),
  )
  const harnesses = orderHarnesses([...registered, ...seen]).map((h) => {
    const turns = sum(input.current.agents, h, (r) => r.operations)
    const prevTurns = input.previous ? sum(input.previous.agents, h, (r) => r.operations) : null
    return {
      harness: h,
      registered: registered.has(h),
      active: turns >= ACTIVE_MIN_TURNS,
      cost_usd: sum(input.current.agents, h, (r) => r.total_cost),
      tokens: sum(input.current.agents, h, (r) => r.total_tokens),
      turns,
      delta_pct: prevTurns ? Math.round(((turns - prevTurns) / prevTurns) * 1000) / 10 : null,
    }
  })
  const active = harnesses.filter((h) => h.active).length
  return {
    harnesses,
    totals: {
      registered: harnesses.filter((h) => h.registered).length,
      active,
      idle: harnesses.filter((h) => h.registered && !h.active).length,
      cost_usd: harnesses.reduce((n, h) => n + h.cost_usd, 0),
      tokens: harnesses.reduce((n, h) => n + h.tokens, 0),
      turns: harnesses.reduce((n, h) => n + h.turns, 0),
    },
    // Shown by last activity, the order the server pages them in.
    sessions: input.sessions.flatMap((s) => {
      const harness = s.is_coding_agent ? harnessOf(s.agent_id) : undefined
      return harness
        ? [
            {
              session_id: s.session_id,
              harness,
              started_at: s.updated_at,
              messages: s.message_count,
              tokens: s.total_tokens,
            },
          ]
        : []
    }),
    prevUnavailable: input.prevFailed,
  }
}

const FINOPS = '/api/observability/finops'

async function ownedAgents(owner: string, signal: AbortSignal): Promise<CatalogAgent[]> {
  // catalog/routes.rs clamps limit to 100: page until a short page.
  const out: CatalogAgent[] = []
  for (let offset = 0; offset < AGENT_PAGE_CAP; offset += AGENT_PAGE_SIZE) {
    const page = await apiFetch<CatalogAgent[]>(
      withQuery('/api/agents', { owner, limit: AGENT_PAGE_SIZE, offset }),
      { signal },
    )
    out.push(...page)
    if (page.length < AGENT_PAGE_SIZE) break
  }
  return out
}

export function useLiveIndividual(
  userId: string | undefined,
  win: ResolvedWindow,
  compare: boolean,
  enabled: boolean,
) {
  const on = enabled && !!userId
  const dash = (params: object, key: string) => ({
    queryKey: harnessKeys.live(userId, 'dashboard', key),
    queryFn: ({ signal }: { signal: AbortSignal }) =>
      apiData<Pick<FinopsDashboardData, 'agents' | 'summary'>>(
        withQuery(`${FINOPS}/dashboard`, { ...params, my_agent: true }),
        { signal, schema: finopsDashboardRowsSchema },
      ),
    enabled: on,
  })
  const current = useQuery(dash(win.params, win.key))
  // The previous window only feeds the Δ, which shows with Compare on.
  const previous = useQuery({
    ...dash(win.prevParams, `prev|${win.prevStart.toISOString()}|${win.prevEnd.toISOString()}`),
    enabled: on && compare,
  })
  const agents = useQuery({
    queryKey: harnessKeys.live(userId, 'agents'),
    queryFn: ({ signal }) => {
      if (!userId)
        throw new Error('owned agents queried without a user id (`enabled` should gate it)')
      return ownedAgents(userId, signal)
    },
    enabled: on,
  })
  const candidates = harnessCandidates(agents.data ?? [])
  const details = useQueries({
    queries: candidates.map((a) => ({
      queryKey: harnessKeys.live(userId, 'agent', a.id),
      // An agent deleted between the list and this call answers 404: it is simply not a
      // harness any more, not an error that a Retry could never clear.
      queryFn: ({ signal }: { signal: AbortSignal }) =>
        apiData<{ coding_agent_integration_id?: string | null }>(`/api/agents/${a.id}`, {
          signal,
        }).catch((err: unknown) => {
          if (err instanceof ApiError && err.status === 404)
            return { coding_agent_integration_id: null }
          throw err
        }),
      enabled: on,
    })),
  })
  const confirmed = new Map<string, string | null>()
  candidates.forEach((a, i) =>
    confirmed.set(a.id, details[i]?.data?.coding_agent_integration_id ?? null),
  )
  const harnessAgentIds = [...confirmed].filter(([, h]) => !!h).map(([id]) => id)
  const sessions = useQueries({
    queries: harnessAgentIds.map((id) => ({
      queryKey: harnessKeys.live(userId, 'chat-sessions', id),
      queryFn: ({ signal }: { signal: AbortSignal }) =>
        apiFetch<{ data: ChatSessionRow[] }>(
          withQuery('/api/chat/sessions', { agent_id: id, limit: LIVE_SESSION_LIMIT }),
          { signal },
        ).then((b) => b.data ?? []),
      enabled: on,
    })),
  })
  const recent = sessions
    .flatMap((q) => q.data ?? [])
    .sort((a, b) => Date.parse(b.updated_at) - Date.parse(a.updated_at))
    .slice(0, LIVE_SESSION_LIMIT)

  // With Compare on, wait for the previous window too: otherwise every Δ reads "new" first.
  const loading =
    current.isPending ||
    agents.isPending ||
    details.some((d) => d.isPending) ||
    sessions.some((q) => q.isPending) ||
    (compare && previous.isPending)
  // A failed confirmation must not read as "not a harness", and a failed session list must not
  // read as "no sessions": both are errors. A failed previous window only loses the Δ.
  const required = [current, agents, ...details, ...sessions]
  const error = required.find((q) => q.isError)?.error ?? null
  const data =
    !loading && !error && current.data
      ? buildLiveIndividual({
          confirmed,
          current: current.data,
          previous: previous.data ?? null,
          prevFailed: previous.isError,
          sessions: recent,
        })
      : undefined
  return {
    data,
    loading: on && loading,
    error,
    refetch: () => required.filter((q) => q.isError).forEach((q) => void q.refetch()),
  }
}
