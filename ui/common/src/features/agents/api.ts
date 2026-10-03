/**
 * Agent queries and mutations (plan §5, §6). Facts that shape them:
 * - The list is a bare array paged by offset over `created_at DESC`, so pages are deduped by
 *   id (an insert between page fetches shifts offsets).
 * - Detail status isn't reconciled with the runtime; the list is. `useAgentStatus` reads the
 *   one list page that holds the agent (`owner=<owner_id>`) instead of polling every page.
 * - Lifecycle routes take the UUID (their name lookup isn't owner-scoped).
 */
import {
  infiniteQueryOptions,
  queryOptions,
  useInfiniteQuery,
  useMutation,
  useQuery,
  useQueryClient,
  type QueryClient,
} from '@tanstack/react-query'
import { useEffect, useMemo } from 'react'
import type { z } from 'zod'
import { create } from 'zustand'
import { onCacheReset } from '@/lib/queryClient'
import { apiData, apiFetch, ApiError, withQuery } from '@/lib/api/client'
import type { LogLine, SessionListResponse, SessionSummary } from '@/features/observability/types'
import type { components } from '@/lib/api/schema.gen'
import {
  grantsBodySchema,
  grantWrite,
  normalizeGrants,
  type AccessGrants,
  type GrantsBody,
} from './grants'
import { normalizeDetail, type AgentView } from './normalize'
import { displayStatus, isHarness, type DisplayStatus } from './status'
import {
  AGENTS_MAX_PAGES,
  AGENTS_PAGE,
  DEPLOYMENT_STALE_MS,
  DETAIL_STALE_MS,
  DIRECTORY_STALE_MS,
  ERROR_LOG_LIMIT,
  ERROR_LOG_WINDOW_MS,
  ERROR_LOGS_STALE_MS,
  LIVE_CARD_STALE_MS,
  LOG_LIMIT,
  POLL_MS,
  SESSIONS_FALLBACK_MS,
  SESSIONS_SCAN,
  USER_SEARCH_MAX,
  USER_SEARCH_MIN,
  USERS_LIMIT,
  WATCH_CAP_MS,
} from './tuning'
import { agentDetailSchema, agentStatsSchema, usage24hSchema } from './types'
import type {
  Agent,
  AgentDetailResponse,
  AgentGrant,
  AgentResources,
  AgentStats,
  AgentVersion,
  DeletedAgent,
  DeploymentRow,
  RollbackResponse,
  SecretName,
  Unavailable,
  UserGrant,
  UserRow,
  UserSearchResult,
} from './types'
import { expireWatch, isWatching, startWatch, stepWatch, type Watch, type WatchKind } from './watch'

const OBS = '/api/observability'

/** The agent directory's key, for pages outside this module that refresh it (deploy's Build page). */
export const AGENTS_DIRECTORY_KEY = ['agents'] as const

export const agentKeys = {
  // Shared with Sessions' directory (same key, same data).
  directory: ['agents'] as const,
  catalog: ['agents', 'catalog'] as const,
  owned: (owner: string) => ['agents', 'owned', owner] as const,
  statusPage: (owner: string) => ['agents', 'status-page', owner] as const,
  detail: (id: string) => ['agents', 'detail', id] as const,
  deployment: (id: string) => ['agents', 'deployment', id] as const,
  versions: (id: string) => ['agents', 'versions', id] as const,
  grants: (id: string) => ['agents', 'grants', id] as const,
  userGrants: (id: string) => ['agents', 'grants-users', id] as const,
  agentGrants: (id: string) => ['agents', 'grants-agents', id] as const,
  secrets: (id: string) => ['agents', 'secrets', id] as const,
  stats: (id: string) => ['agents', 'stats', id] as const,
  resources: (id: string) => ['agents', 'resources', id] as const,
  errorLogs: (id: string) => ['agents', 'error-logs', id] as const,
  logs: (id: string, level: string) => ['agents', 'logs', id, level] as const,
  liveCard: (id: string) => ['agents', 'live-card', id] as const,
  sessions: (name: string, createdAt: string) => ['agents', 'sessions', name, createdAt] as const,
  usage24h: ['agents', 'usage-24h'] as const,
  users: ['agents', 'users'] as const,
  userSearch: (q: string) => ['agents', 'user-search', q] as const,
  settings: ['agents', 'settings'] as const,
}

// ─── lists ──────────────────────────────────────────────────────────────────

const listPath = (offset: number, owner?: string) =>
  withQuery('/api/agents', { limit: AGENTS_PAGE, offset, owner })

async function fetchPage(offset: number, signal: AbortSignal, owner?: string): Promise<Agent[]> {
  const page = await apiFetch<unknown>(listPath(offset, owner), { signal })
  if (!Array.isArray(page))
    throw new ApiError(
      200,
      page,
      listPath(offset, owner),
      'GET /api/agents → unexpected response (not an array)',
    )
  return page as Agent[]
}

/** Drop rows already seen (by id), keeping first-seen order. */
export function dedupeById<T extends { id: string }>(rows: T[]): T[] {
  const seen = new Set<string>()
  return rows.filter((r) => (seen.has(r.id) ? false : (seen.add(r.id), true)))
}

/** Every page of GET /api/agents (limit clamps to 100), optionally one owner's. */
async function fetchAllAgents(signal: AbortSignal, owner?: string): Promise<Agent[]> {
  const all: Agent[] = []
  for (let offset = 0; offset < AGENTS_PAGE * AGENTS_MAX_PAGES; offset += AGENTS_PAGE) {
    const page = await fetchPage(offset, signal, owner)
    all.push(...page)
    if (page.length < AGENTS_PAGE) break
  }
  return dedupeById(all)
}

interface DirectoryIndex {
  byName: Map<string, Agent>
  byId: Map<string, Agent>
  byNameAll: Map<string, Agent[]>
}
const EMPTY_INDEX: DirectoryIndex = { byName: new Map(), byId: new Map(), byNameAll: new Map() }
/** Built once per fetched list, shared by every caller (a table can hold hundreds of AgentLinks). */
const indexCache = new WeakMap<Agent[], DirectoryIndex>()
function indexFor(rows: Agent[] | undefined): DirectoryIndex {
  if (!rows) return EMPTY_INDEX
  let idx = indexCache.get(rows)
  if (!idx) {
    idx = { byName: new Map(), byId: new Map(), byNameAll: new Map() }
    for (const a of rows) {
      idx.byName.set(a.name, a)
      idx.byId.set(a.id, a)
      const same = idx.byNameAll.get(a.name)
      if (same) same.push(a)
      else idx.byNameAll.set(a.name, [a])
    }
    indexCache.set(rows, idx)
  }
  return idx
}

/** Every agent the caller can see, by id, raw name and all agents sharing a name. */
export const directoryQuery = queryOptions({
  queryKey: agentKeys.directory,
  queryFn: ({ signal }) => fetchAllAgents(signal),
  staleTime: DIRECTORY_STALE_MS,
  meta: { path: '/api/agents' },
})

export function useAgentsDirectory(enabled = true) {
  const q = useQuery({ ...directoryQuery, enabled })
  return { ...q, ...indexFor(q.data) }
}

export const catalogQuery = infiniteQueryOptions({
  queryKey: agentKeys.catalog,
  queryFn: ({ pageParam, signal }) => fetchPage(pageParam, signal),
  initialPageParam: 0,
  getNextPageParam: (last, pages) =>
    last.length === AGENTS_PAGE && pages.length < AGENTS_MAX_PAGES
      ? pages.length * AGENTS_PAGE
      : undefined,
  staleTime: DIRECTORY_STALE_MS,
  meta: { path: '/api/agents' },
})

/**
 * The catalog: page 1 renders at once, later pages follow ("Loading more…"); a failed later
 * page keeps what loaded. The auto-advance effect depends on pages.length (learning:
 * batched renders can skip the intermediate fetching state).
 */
export function useCatalogAgents() {
  const q = useInfiniteQuery(catalogQuery)
  const pages = q.data?.pages.length ?? 0
  const { hasNextPage, isFetchingNextPage, isFetchNextPageError, fetchNextPage } = q
  useEffect(() => {
    if (hasNextPage && !isFetchingNextPage && !isFetchNextPageError) void fetchNextPage()
  }, [pages, hasNextPage, isFetchingNextPage, isFetchNextPageError, fetchNextPage])
  const agents = useMemo(() => dedupeById((q.data?.pages ?? []).flat()), [q.data])
  return { ...q, agents }
}

/** A row Deploying for less than the watch cap; one stuck longer (a lost build job) stops the polling. */
export const hasFreshDeploying = (rows: Agent[] | undefined, now: number) =>
  !!rows?.some(
    (a) =>
      displayStatus(a.status, isHarness(a)) === 'deploying' &&
      now - Date.parse(a.updated_at) < WATCH_CAP_MS,
  )

/** Your agents: one owner's rows, polled every 5 s while one is Deploying or a row is being watched. */
export const ownedQuery = (owner: string) =>
  queryOptions({
    queryKey: agentKeys.owned(owner),
    queryFn: ({ signal }) => fetchAllAgents(signal, owner),
    meta: { path: '/api/agents?owner=' },
  })

export function useOwnedAgents(owner: string | undefined, watching = false) {
  return useQuery({
    ...ownedQuery(owner ?? ''),
    enabled: !!owner,
    // Pure: reads only the query's own data and the clock (learning: refetchInterval runs on every render).
    refetchInterval: (query) =>
      watching || hasFreshDeploying(query.state.data, Date.now()) ? POLL_MS : false,
  })
}

// ─── one agent ──────────────────────────────────────────────────────────────

export function agentDetailQuery(id: string) {
  return queryOptions({
    queryKey: agentKeys.detail(id),
    queryFn: ({ signal }) =>
      apiData<AgentDetailResponse>(`/api/agents/${id}`, { signal, schema: agentDetailSchema }),
    staleTime: DETAIL_STALE_MS,
    meta: { path: `/api/agents/${id}` },
  })
}

/** `poll`: while a watch runs and the agent isn't on its owner's first list page, the detail is the only status source. */
export function useAgentDetail(id: string, enabled = true, poll = false) {
  return useQuery({
    ...agentDetailQuery(id),
    enabled,
    select: normalizeDetail,
    refetchInterval: poll ? POLL_MS : false,
  })
}

/**
 * The detail page's display status: the owner-scoped list page (reconciled server-side)
 * when the agent is on it, else the detail's own `status`. Polls only while watching.
 */
export function useAgentStatus(
  agent: AgentView | undefined,
  watching: boolean,
): {
  display: DisplayStatus | undefined
  raw: string | undefined
  row: Agent | undefined
  failed: boolean
  updatedAt: number
  errorAt: number
} {
  const owner = agent?.ownerId ?? ''
  const q = useQuery({
    queryKey: agentKeys.statusPage(owner),
    queryFn: ({ signal }) => fetchPage(0, signal, owner),
    enabled: !!agent && !agent.isHarness,
    staleTime: DETAIL_STALE_MS,
    refetchInterval: watching ? POLL_MS : false,
    meta: { path: '/api/agents?owner=' },
  })
  if (!agent)
    return {
      display: undefined,
      raw: undefined,
      row: undefined,
      failed: false,
      updatedAt: 0,
      errorAt: 0,
    }
  const row = q.data?.find((a) => a.id === agent.id)
  const raw = row?.status ?? agent.status
  return {
    display: displayStatus(raw, agent.isHarness),
    raw,
    row,
    failed: q.isError,
    updatedAt: q.dataUpdatedAt,
    errorAt: q.errorUpdatedAt,
  }
}

/** `/deployment`: a row, `{available:false}`, or `null` when there's no live row (404). Shared with the Overview's fleet health. */
export function deploymentQuery(id: string) {
  return queryOptions({
    queryKey: agentKeys.deployment(id),
    queryFn: async ({ signal }): Promise<DeploymentRow | Unavailable | null> => {
      try {
        const body = await apiFetch<DeploymentRow | Unavailable>(`/api/agents/${id}/deployment`, {
          signal,
        })
        return body ?? null
      } catch (err) {
        if (err instanceof ApiError && err.status === 404) return null
        throw err
      }
    },
    staleTime: DEPLOYMENT_STALE_MS,
  })
}

export function useDeployment(id: string, enabled: boolean, watching = false) {
  return useQuery({ ...deploymentQuery(id), enabled, refetchInterval: watching ? POLL_MS : false })
}

export const isUnavailable = (v: unknown): v is Unavailable =>
  !!v && typeof v === 'object' && (v as { available?: unknown }).available === false

export function useVersions(id: string, enabled: boolean) {
  return useQuery({
    queryKey: agentKeys.versions(id),
    queryFn: ({ signal }) => apiData<AgentVersion[]>(`/api/agents/${id}/versions`, { signal }),
    enabled,
    select: (vs) => [...vs].sort((a, b) => b.created_at.localeCompare(a.created_at)),
  })
}

/** Parse guard: the grants routes are OSS-mounted; EE serves its own shapes at these paths. */
function arrayOf<T>(check: (x: unknown) => boolean, path: string) {
  return (body: unknown): T[] => {
    if (!Array.isArray(body) || !body.every(check))
      throw new ApiError(200, body, path, `${path} → unexpected response shape`)
    return body as T[]
  }
}
const isObj = (x: unknown): x is Record<string, unknown> => !!x && typeof x === 'object'

/** Both editions' `/grants` (grants.ts): the summary, tagged with the edition that answered. */
const grantsQuery = (id: string) =>
  queryOptions({
    queryKey: agentKeys.grants(id),
    queryFn: async ({ signal }): Promise<AccessGrants> => {
      const path = `/api/agents/${id}/grants`
      const body = await apiFetch<GrantsBody>(path, { signal, schema: grantsBodySchema })
      const summary = normalizeGrants(body, id)
      if (summary.edition === 'ee') {
        // EE can be public with no 'public' grant row (grants.rs revoke_public); agents.is_public is the truth.
        const vis = await apiFetch<unknown>(`/api/agents/${id}/visibility`, { signal })
        if (
          vis &&
          typeof vis === 'object' &&
          typeof (vis as { is_public?: unknown }).is_public === 'boolean'
        )
          return { ...summary, is_public: (vis as { is_public: boolean }).is_public }
      }
      return summary
    },
  })

export function useGrants(id: string, enabled: boolean) {
  return useQuery({ ...grantsQuery(id), enabled })
}

export function useUserGrants(id: string, enabled: boolean) {
  const qc = useQueryClient()
  const path = `/api/agents/${id}/grants/users`
  // `username` is null for a grant to a deleted user (LEFT JOIN, agents/grants.rs:226-245): still removable.
  const parse = arrayOf<UserGrant>(
    (x) =>
      isObj(x) &&
      typeof x.user_id === 'string' &&
      (typeof x.username === 'string' || x.username == null),
    path,
  )
  return useQuery({
    queryKey: agentKeys.userGrants(id),
    queryFn: async ({ signal }) => {
      // Once the edition is known to be OSS, fetch at once instead of waiting on /grants (refetches after a write).
      if (qc.getQueryData<AccessGrants>(agentKeys.grants(id))?.edition === 'oss')
        return parse(await apiFetch<unknown>(path, { signal }))
      // fetchQuery, not ensureQueryData: after a write invalidates /grants, the derived lists must see the new rows.
      const grants = await qc.fetchQuery({ ...grantsQuery(id), staleTime: 5_000 })
      if (grants.edition === 'oss') return parse(await apiFetch<unknown>(path, { signal }))
      // EE has no /grants/users read: the grant rows carry ids, and /api/agents/{id}/users names everyone with access.
      if (!grants.user_grants.length) return []
      const users = await apiFetch<unknown>(`/api/agents/${id}/users`, { signal })
      const names = new Map(
        (Array.isArray(users) ? users : [])
          .filter(isObj)
          .map((u) => [String(u.id), typeof u.username === 'string' ? u.username : null]),
      )
      return grants.user_grants.map((uid) => ({ user_id: uid, username: names.get(uid) ?? null }))
    },
    enabled,
  })
}

export function useAgentGrants(id: string, enabled: boolean) {
  const qc = useQueryClient()
  const path = `/api/agents/${id}/grants/agents`
  const parse = arrayOf<AgentGrant>((x) => isObj(x) && typeof x.target_agent_id === 'string', path)
  return useQuery({
    queryKey: agentKeys.agentGrants(id),
    queryFn: async ({ signal }) => {
      if (qc.getQueryData<AccessGrants>(agentKeys.grants(id))?.edition === 'oss')
        return parse(await apiFetch<unknown>(path, { signal }))
      // fetchQuery, not ensureQueryData: after a write invalidates /grants, the derived lists must see the new rows.
      const grants = await qc.fetchQuery({ ...grantsQuery(id), staleTime: 5_000 })
      if (grants.edition === 'oss') return parse(await apiFetch<unknown>(path, { signal }))
      // EE has no /grants/agents read; the Access tab names targets from the agents directory.
      return grants.agent_acl.map((tid) => ({ target_agent_id: tid, target_name: null }))
    },
    enabled,
  })
}

export function useSecrets(id: string, enabled: boolean) {
  const path = `/api/agents/${id}/secrets`
  const parse = arrayOf<SecretName>((x) => isObj(x) && typeof x.name === 'string', path)
  return useQuery({
    queryKey: agentKeys.secrets(id),
    queryFn: async ({ signal }) => parse(await apiFetch<unknown>(path, { signal })),
    enabled,
  })
}

export function useAgentStats(id: string, enabled: boolean) {
  return useQuery({
    queryKey: agentKeys.stats(id),
    queryFn: ({ signal }) =>
      apiData<z.infer<typeof agentStatsSchema>>(`${OBS}/agent/${id}/stats`, {
        signal,
        schema: agentStatsSchema,
      }),
    enabled,
    select: (d): AgentStats => d.project ?? d,
  })
}

export function useAgentResources(id: string, enabled: boolean) {
  return useQuery({
    queryKey: agentKeys.resources(id),
    queryFn: ({ signal }) => apiData<AgentResources>(`${OBS}/agent/${id}/resources`, { signal }),
    enabled,
  })
}

/**
 * The Activity dot and the OSS crash card: newest ERROR lines. `since` must be RFC 3339 (a
 * relative value is silently dropped, observability/routes.rs:238) and the level filter runs
 * after the per-source limit, so this means "errors in the latest lines", not "last 24 h".
 */
export function useErrorLogs(id: string, enabled: boolean, now: () => number = Date.now) {
  return useQuery({
    queryKey: agentKeys.errorLogs(id),
    queryFn: ({ signal }) =>
      apiFetch<LogLine[]>(
        withQuery(`${OBS}/agents/${id}/logs`, {
          level: 'ERROR',
          limit: ERROR_LOG_LIMIT,
          since: new Date(now() - ERROR_LOG_WINDOW_MS).toISOString(),
        }),
        { signal },
      ),
    enabled,
    staleTime: ERROR_LOGS_STALE_MS,
  })
}

export function useAgentLogs(id: string, level: string, enabled: boolean) {
  return useQuery({
    queryKey: agentKeys.logs(id, level),
    queryFn: ({ signal }) =>
      apiFetch<LogLine[]>(
        withQuery(`${OBS}/agents/${id}/logs`, { limit: LOG_LIMIT, level: level || undefined }),
        { signal },
      ),
    enabled,
  })
}

/** Proxied to the agent itself: only when the collapsible opens, and cached. */
export function useLiveCard(id: string, enabled: boolean) {
  return useQuery({
    queryKey: agentKeys.liveCard(id),
    queryFn: ({ signal }) =>
      apiFetch<unknown>(`/api/agents/${id}/.well-known/agent-card.json`, { signal }),
    enabled,
    staleTime: LIVE_CARD_STALE_MS,
  })
}

/**
 * Recent sessions: the first page of session/list (no agent filter server-side), matched by
 * the raw agent name and created after the agent was (a deleted namesake's sessions drop).
 */
export function useAgentSessions(name: string, createdAt: string, enabled: boolean) {
  return useQuery({
    queryKey: agentKeys.sessions(name, createdAt),
    queryFn: ({ signal }) =>
      apiFetch<SessionListResponse>(
        withQuery(`${OBS}/session/list`, {
          start_time: createdAt || new Date(Date.now() - SESSIONS_FALLBACK_MS).toISOString(),
          limit: SESSIONS_SCAN,
          offset: 0,
        }),
        { signal },
      ),
    enabled: enabled && !!name,
    select: (r): SessionSummary[] => {
      const since = createdAt ? Date.parse(createdAt) : 0
      return (r?.data?.sessions ?? []).filter(
        (s) => s.agent_id === name && (!s.start_time || Date.parse(s.start_time) >= since),
      )
    },
  })
}

type FinopsAgentRow = components['schemas']['AgentFinopsRow']

/** 24 h turns and est. cost per agent id (finops dashboard rows carry display names; join by id). */
export function useUsage24h(enabled: boolean) {
  return useQuery({
    queryKey: agentKeys.usage24h,
    queryFn: ({ signal }) =>
      apiData<{ agents: FinopsAgentRow[] }>(
        withQuery(`${OBS}/finops/dashboard`, { range: '24h', view: 'agent' }),
        { signal, schema: usage24hSchema },
      ),
    enabled,
    staleTime: DETAIL_STALE_MS,
    select: (d) =>
      new Map(d.agents.map((r) => [r.agent_id, { turns: r.operations, cost: r.total_cost }])),
  })
}

/** Superusers only (users router is behind require_superuser). */
export function useUsers(enabled: boolean) {
  return useQuery({
    queryKey: agentKeys.users,
    queryFn: ({ signal }) =>
      apiFetch<{ data: UserRow[] }>(withQuery('/api/users', { limit: USERS_LIMIT }), { signal }),
    enabled,
    staleTime: DIRECTORY_STALE_MS,
    select: (r) => new Map((r?.data ?? []).map((u) => [u.id, u.display_name || u.username])),
  })
}

export function useUserSearch(q: string) {
  const term = q.trim()
  return useQuery({
    queryKey: agentKeys.userSearch(term),
    queryFn: ({ signal }) =>
      apiData<UserSearchResult[]>(withQuery('/api/search/users', { q: term }), { signal }),
    enabled: term.length >= USER_SEARCH_MIN,
    select: (rows) => rows.slice(0, USER_SEARCH_MAX),
  })
}

export function useCatalogTabs() {
  return useQuery({
    queryKey: agentKeys.settings,
    queryFn: ({ signal }) =>
      apiFetch<{ catalog_tabs?: string | null } | { data?: { catalog_tabs?: string | null } }>(
        '/api/settings',
        { signal },
      ),
    staleTime: DIRECTORY_STALE_MS,
    select: (r): string[] => {
      const raw =
        (r && 'data' in r
          ? r.data?.catalog_tabs
          : (r as { catalog_tabs?: string | null })?.catalog_tabs) ?? ''
      return raw
        .split(',')
        .map((t) => t.trim())
        .filter(Boolean)
    },
  })
}

// ─── watch (truth checks) ───────────────────────────────────────────────────

/**
 * Per-agent lifecycle watches: client state that outlives a route (tab switches, remounts), so a
 * zustand store, not the query cache. Snapshots are replaced, never mutated. It ends with the
 * session: `onCacheReset` clears it on every cache clear or reset (sign-out, sign-in, a 401 expiry,
 * another account signing in), and deleting an agent drops its watch.
 */
const useWatchStore = create<Readonly<Record<string, Watch>>>(() => ({}))

const getWatch = (id: string): Watch | null => useWatchStore.getState()[id] ?? null

function setWatch(id: string, w: Watch | null) {
  useWatchStore.setState((s) => {
    const { [id]: _, ...rest } = s
    return w ? { ...rest, [id]: w } : rest
  }, true)
}

/** Drops every watch: every cache clear or reset runs it (onCacheReset). */
const clearWatches = () => useWatchStore.setState({}, true)
onCacheReset(clearWatches)

export function useWatch(id: string) {
  const watch = useWatchStore((s) => s[id] ?? null)
  return {
    watch,
    watching: isWatching(watch),
    begin: (kind: WatchKind, buildId?: string) =>
      setWatch(id, startWatch(kind, Date.now(), buildId)),
    clear: () => setWatch(id, null),
  }
}

/** Whether any of these agents has a watch running (drives a list's polling). */
export function useAnyWatching(ids: readonly string[]): boolean {
  return useWatchStore((s) => ids.some((id) => isWatching(s[id])))
}

/** Store the next watch; when it just finished, refetch what the outcome changed (version, deployment). */
function applyWatch(qc: QueryClient, id: string, w: Watch, next: Watch) {
  if (next === w) return
  setWatch(id, next)
  if (next.outcome) {
    for (const k of [agentKeys.detail(id), agentKeys.versions(id), agentKeys.deployment(id)])
      void qc.invalidateQueries({ queryKey: k })
    // The directory's status feeds the Overview's fleet health: refresh it with the real outcome (/ship adversarial).
    void qc.invalidateQueries({ queryKey: agentKeys.directory, exact: true })
  }
}

/**
 * Fold each fresh status into the active watch. `errorAt` is the status query's
 * `errorUpdatedAt`: a failed poll carries no reading, but still ends the watch at the cap.
 */
export function useWatchStep(
  id: string,
  display: DisplayStatus | undefined,
  updatedAt: number,
  errorAt = 0,
) {
  const qc = useQueryClient()
  useEffect(() => {
    if (!display) return
    const w = getWatch(id)
    if (isWatching(w)) applyWatch(qc, id, w, stepWatch(w, display, Date.now()))
  }, [qc, id, display, updatedAt])
  useEffect(() => {
    if (!errorAt) return
    const w = getWatch(id)
    if (isWatching(w)) applyWatch(qc, id, w, expireWatch(w, Date.now()))
  }, [qc, id, errorAt])
}

/**
 * A list's version of useWatchStep: steps every watched row from the page, so a row that
 * leaves the visible tab (or unmounts) keeps being checked until its watch ends.
 */
export function useWatchesStep(
  rows: readonly { id: string; display: DisplayStatus }[],
  updatedAt: number,
  errorAt: number,
) {
  const qc = useQueryClient()
  useEffect(() => {
    for (const r of rows) {
      const w = getWatch(r.id)
      if (isWatching(w)) applyWatch(qc, r.id, w, stepWatch(w, r.display, Date.now()))
    }
    // rows is rebuilt from the same data as updatedAt; stepping on updatedAt alone avoids double steps.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [qc, updatedAt])
  useEffect(() => {
    if (!errorAt) return
    for (const r of rows) {
      const w = getWatch(r.id)
      if (isWatching(w)) applyWatch(qc, r.id, w, expireWatch(w, Date.now()))
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [qc, errorAt])
}

// ─── mutations ──────────────────────────────────────────────────────────────

const json = (body: unknown): RequestInit => ({
  method: 'POST',
  headers: { 'Content-Type': 'application/json' },
  body: JSON.stringify(body),
})

/**
 * Only what a lifecycle action, roll back or edit changes: the status lists and this agent's
 * detail, deployment, versions and the directory (fleet health reads its status). Usage, logs and sessions stay put.
 * cancelRefetch (default) drops an in-flight poll, so a stale answer can't overwrite the new state.
 */
function invalidateAgent(qc: QueryClient, id: string) {
  for (const k of [
    ['agents', 'owned'],
    ['agents', 'status-page'],
    agentKeys.detail(id),
    agentKeys.deployment(id),
    agentKeys.versions(id),
  ]) {
    void qc.invalidateQueries({ queryKey: k })
  }
  // The directory carries status for the Overview's fleet health: a restart must not leave it saying "crashed".
  void qc.invalidateQueries({ queryKey: agentKeys.directory, exact: true })
}

/** Lists that show names (a rename or delete changes them). Inactive ones just go stale. */
function invalidateNameLists(qc: QueryClient) {
  void qc.invalidateQueries({ queryKey: agentKeys.directory, exact: true })
  void qc.invalidateQueries({ queryKey: agentKeys.catalog })
}

export function useLifecycle(id: string) {
  const qc = useQueryClient()
  return useMutation({
    mutationFn: (action: 'restart' | 'stop' | 'start') =>
      apiFetch<unknown>(`/api/containers/${id}/${action}`, { method: 'POST' }),
    onSettled: () => invalidateAgent(qc, id),
  })
}

export function useRollback(id: string) {
  const qc = useQueryClient()
  return useMutation({
    mutationFn: (v: { target_version: string; reason?: string }) =>
      apiFetch<RollbackResponse>(`/api/agents/${id}/rollback`, json(v)),
    onSettled: () => invalidateAgent(qc, id),
  })
}

export function useDeleteAgent(id: string) {
  const qc = useQueryClient()
  return useMutation({
    mutationFn: () => apiFetch<DeletedAgent>(`/api/agents/${id}`, { method: 'DELETE' }),
    onSuccess: () => {
      // Drop everything about the deleted agent, so nothing refetches a 404, and its watch.
      qc.removeQueries({ queryKey: ['agents'], predicate: (q) => q.queryKey[2] === id })
      setWatch(id, null)
      for (const k of [
        ['agents', 'owned'],
        ['agents', 'status-page'],
      ])
        void qc.invalidateQueries({ queryKey: k })
      invalidateNameLists(qc)
    },
  })
}

/** `PUT /api/agents/{id}` (models.rs `UpdateAgentRequest`): the fields the Settings tab writes. */
export interface AgentUpdate {
  display_name?: string
  description?: string
  metadata?: Record<string, unknown>
  compress_enabled?: boolean
  minimal_code_enabled?: boolean
}

export function useUpdateAgent(id: string) {
  const qc = useQueryClient()
  return useMutation({
    // Every field is COALESCEd server-side, so a body carries only what it changes (catalog/routes.rs update).
    mutationFn: (v: AgentUpdate) =>
      apiFetch<Agent>(`/api/agents/${id}`, { ...json(v), method: 'PUT' }),
    onSettled: () => {
      invalidateAgent(qc, id)
      invalidateNameLists(qc)
      // Pending until the detail is re-read, so a Settings switch never flashes back to its old state.
      return qc.refetchQueries({ queryKey: agentKeys.detail(id) }, { cancelRefetch: false })
    },
  })
}

export function useSecretMutations(id: string) {
  const qc = useQueryClient()
  // Returned, so a write stays pending until the list is re-read (the Self-review switch reads it).
  const done = () => qc.invalidateQueries({ queryKey: agentKeys.secrets(id) })
  return {
    // gcTime 0 + the caller's reset(): the value must not stay in the mutation cache as `variables`.
    set: useMutation({
      mutationFn: (v: { name: string; value: string }) =>
        apiFetch<null>(`/api/agents/${id}/secrets`, json(v)),
      onSettled: done,
      gcTime: 0,
    }),
    remove: useMutation({
      mutationFn: (name: string) =>
        apiFetch<null>(`/api/agents/${id}/secrets/${encodeURIComponent(name)}`, {
          method: 'DELETE',
        }),
      onSettled: done,
    }),
  }
}

export function useGrantMutations(id: string) {
  const qc = useQueryClient()
  const done = () => {
    for (const k of [agentKeys.grants(id), agentKeys.userGrants(id), agentKeys.agentGrants(id)])
      void qc.invalidateQueries({ queryKey: k })
  }
  // The Access tab has loaded /grants before any write, so the edition is known (grants.ts).
  const edition = () => qc.getQueryData<AccessGrants>(agentKeys.grants(id))?.edition
  const grantPost = (w: { path: string; body?: unknown }) =>
    apiFetch<null>(w.path, w.body === undefined ? { method: 'POST' } : json(w.body))
  return {
    setPublic: useMutation({
      mutationFn: (on: boolean) =>
        apiFetch<null>(`/api/agents/${id}/grants/public`, { method: on ? 'POST' : 'DELETE' }),
      onSettled: done,
    }),
    addUser: useMutation({
      mutationFn: (userId: string) => grantPost(grantWrite(edition(), id, 'users', userId)),
      onSettled: done,
    }),
    removeUser: useMutation({
      mutationFn: (userId: string) =>
        apiFetch<null>(`/api/agents/${id}/grants/users/${userId}`, { method: 'DELETE' }),
      onSettled: done,
    }),
    addAgent: useMutation({
      mutationFn: (agentId: string) => grantPost(grantWrite(edition(), id, 'agents', agentId)),
      onSettled: done,
    }),
    removeAgent: useMutation({
      mutationFn: (agentId: string) =>
        apiFetch<null>(`/api/agents/${id}/grants/agents/${agentId}`, { method: 'DELETE' }),
      onSettled: done,
    }),
  }
}
