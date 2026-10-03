/**
 * Builds and uploads data (plans/feat-deploy.md §5, §6, §8). All `/api/*` reads go through TanStack Query; the build
 * page's live status comes from the deploy stream (`src/lib/sse.ts`, fetch-based, so no EventSource auto-reconnect).
 */
import {
  keepPreviousData,
  queryOptions,
  useMutation,
  useQueries,
  useQuery,
  useQueryClient,
} from '@tanstack/react-query'
import { useEffect, useRef, useState } from 'react'
import { ApiError, apiData, apiFetch, withQuery } from '@/lib/api/client'
import { createLimiter } from '@/features/observability/limiter'
import type { components } from '@/lib/api/schema.gen'
import { readSse } from '@/lib/sse'
import {
  BUILDS_PAGE_SIZE,
  BUILDS_REFRESH_MS,
  GITHUB_CONFIGURED_STALE_MS,
  GITHUB_REPOS_STALE_MS,
  REASON_GC_MS,
  REASON_READS,
  STREAM_FALLBACK_POLL_MS,
} from './tuning'
import { IMPORT_PATH, rememberClone, uploadsEpoch } from './uploads'
import {
  buildRecordSchema,
  buildsPageSchema,
  cloneResultSchema,
  githubConfiguredSchema,
  githubReposSchema,
  githubTokenSchema,
  githubUserSchema,
  streamFrameSchema,
  unavailableSchema,
  type BuildRecord,
  type BuildStatus,
  type UploadStatus,
} from './types'
import type { BuildsFilter } from './search'
import { isActive } from './steps'
import { follow } from './follower'

export const buildKeys = {
  list: (p: { status?: BuildStatus; q?: string; page: number }) => ['builds', 'list', p] as const,
  active: (status: 'queued' | 'building') => ['builds', 'active', status] as const,
  detail: (id: string) => ['builds', 'detail', id] as const,
  forAgent: (agentId: string) => ['builds', 'agent', agentId] as const,
  upload: (id: string) => ['builds', 'upload', id] as const,
}

/** A read that may answer `{available:false}` (no deploy rights, EE) instead of data. */
export type Gated<T> = { available: true; value: T } | { available: false }

function gated<T>(body: unknown, parse: (b: unknown) => T): Gated<T> {
  if (unavailableSchema.safeParse(body).success) return { available: false }
  return { available: true, value: parse(body) }
}

const listUrl = (p: { status?: BuildStatus; q?: string; offset: number; limit: number }) =>
  withQuery('/api/builds', { limit: p.limit, offset: p.offset, status: p.status, q: p.q?.trim() })

export function buildsListQuery(p: { status?: BuildStatus; q?: string; page: number }) {
  return queryOptions({
    queryKey: buildKeys.list(p),
    queryFn: async ({ signal }) =>
      gated(
        await apiFetch<unknown>(
          listUrl({ ...p, offset: p.page * BUILDS_PAGE_SIZE, limit: BUILDS_PAGE_SIZE }),
          { signal },
        ),
        (b) => buildsPageSchema.parse(b).data,
      ),
    retry: false,
    meta: { path: '/api/builds' },
  })
}

function activeQuery(status: 'queued' | 'building', q: string | undefined) {
  return queryOptions({
    queryKey: [...buildKeys.active(status), q ?? ''] as const,
    queryFn: async ({ signal }) =>
      gated(
        await apiFetch<unknown>(listUrl({ status, q, offset: 0, limit: BUILDS_PAGE_SIZE }), {
          signal,
        }),
        (b) => buildsPageSchema.parse(b).data,
      ),
    retry: false,
    meta: { path: '/api/builds?status=' },
  })
}

/**
 * The Builds page (design review 2): in-progress builds pinned on top (the server filters one status at a time, so
 * queued and building are two small reads), then the chosen page newest first, without the pinned ids. Everything
 * refreshes every 5 s while a build is in progress and the tab is visible (TanStack pauses intervals when hidden).
 */
export function useBuildsPage(filter: BuildsFilter, q: string | undefined, page: number) {
  const showPinned = filter === 'all' || filter === 'active'
  const listStatus: BuildStatus | undefined =
    filter === 'failed' ? 'failed' : filter === 'success' ? 'success' : undefined
  const queued = useQuery({
    ...activeQuery('queued', q),
    enabled: showPinned,
    placeholderData: keepPreviousData,
    refetchInterval: (query) => (anyActive(query.state.data) ? BUILDS_REFRESH_MS : false),
  })
  const building = useQuery({
    ...activeQuery('building', q),
    enabled: showPinned,
    placeholderData: keepPreviousData,
    refetchInterval: (query) => (anyActive(query.state.data) ? BUILDS_REFRESH_MS : false),
  })
  // Only while the pinned reads are shown: on Failed/Succeeded they're disabled and their cache is frozen.
  const pinnedLive = showPinned && [queued.data, building.data].some(anyActive)
  const list = useQuery({
    ...buildsListQuery({ status: listStatus, q, page }),
    enabled: filter !== 'active',
    placeholderData: keepPreviousData,
    refetchInterval: pinnedLive ? BUILDS_REFRESH_MS : false,
  })
  const pinned = showPinned
    ? [...rowsOf(building.data), ...rowsOf(queued.data)].sort((a, b) =>
        b.created_at.localeCompare(a.created_at),
      )
    : []
  const pinnedIds = new Set(pinned.map((b) => b.id))
  // A pinned build that finished leaves the pinned reads; the list (which stops polling with them) must re-read once so
  // the build reappears there instead of vanishing until the next reload.
  const pinnedKey = [...pinnedIds].sort().join(',')
  // Only when the same view's pins shrink: a filter or search change fetches its own list already, and Active has none.
  const viewKey = `${filter}|${q ?? ''}`
  const lastPinned = useRef({ viewKey, pinnedKey })
  const refetchList = list.refetch
  useEffect(() => {
    const before = lastPinned.current
    lastPinned.current = { viewKey, pinnedKey }
    if (before.viewKey !== viewKey || filter === 'active') return
    const now = new Set(pinnedKey.split(','))
    if (before.pinnedKey.split(',').some((id) => id && !now.has(id))) void refetchList()
  }, [viewKey, pinnedKey, filter, refetchList])
  // `total` is the page length (D-6): a full page means there may be another.
  const rows = filter === 'active' ? [] : rowsOf(list.data).filter((b) => !pinnedIds.has(b.id))
  const noRights = [queued.data, building.data, list.data].some((d) => d && !d.available)
  const pages = [queued, building, list].filter((_, i) =>
    i < 2 ? showPinned : filter !== 'active',
  )
  return {
    pinned,
    rows,
    hasNext: filter !== 'active' && rowsOf(list.data).length === BUILDS_PAGE_SIZE,
    isPending: pages.some((x) => x.isPending),
    error: pages.find((x) => x.isError)?.error ?? null,
    noRights,
    /** "Now" for elapsed times: the last refresh (render stays pure; in-progress rows refresh every 5 s). */
    updatedAt: Math.max(0, ...pages.map((x) => x.dataUpdatedAt)),
    retry: () => pages.forEach((x) => void x.refetch()),
  }
}

const rowsOf = (d: Gated<BuildRecord[]> | undefined): BuildRecord[] => (d?.available ? d.value : [])
const anyActive = (d: Gated<BuildRecord[]> | undefined) => rowsOf(d).some((b) => isActive(b.status))

/** `GET /api/builds/{id}`: a 404 (empty body) is "not found or not yours"; the route itself exists on every server. */
export function useBuild(id: string, live: boolean) {
  return useQuery({
    queryKey: buildKeys.detail(id),
    queryFn: async ({ signal }) => {
      try {
        return gated(
          await apiFetch<unknown>(`/api/builds/${encodeURIComponent(id)}`, { signal }),
          (b) => buildRecordSchema.parse(b),
        )
      } catch (err) {
        // 404: not found or not yours. 400: axum rejected a non-UUID id (a mistyped link): not found either.
        if (err instanceof ApiError && (err.status === 404 || err.status === 400)) return null
        throw err
      }
    },
    retry: false,
    // Polled only while the page's stream is down, and only until the build finishes.
    // A null (404: deleted with its agent after a failed first upload) stops it too, like useUploadStatus.
    refetchInterval: (q) =>
      live &&
      q.state.data !== null &&
      !(q.state.data?.available && !isActive(q.state.data.value.status))
        ? STREAM_FALLBACK_POLL_MS
        : false,
    meta: { path: '/api/builds/{id}' },
  })
}

/** `GET /api/builds/agent/{id}` (last 20) for an agent's Builds tab. */
export function useAgentBuilds(agentId: string) {
  return useQuery({
    queryKey: buildKeys.forAgent(agentId),
    queryFn: async ({ signal }) =>
      gated(
        await apiFetch<unknown>(`/api/builds/agent/${encodeURIComponent(agentId)}`, { signal }),
        (b) => buildRecordSchema.array().parse(b),
      ),
    retry: false,
    // Like the Builds list (§6): refresh while one of them is queued or building and the tab is visible.
    refetchInterval: (q) =>
      q.state.data?.available && q.state.data.value.some((b) => isActive(b.status))
        ? BUILDS_REFRESH_MS
        : false,
    meta: { path: '/api/builds/agent/{id}' },
  })
}

/** `GET /api/agents/uploads/{id}`: bare row, or an empty 404 when it isn't an upload or isn't the caller's. */
async function fetchUpload(id: string, signal: AbortSignal): Promise<UploadStatus | null> {
  try {
    return await apiFetch<UploadStatus>(`/api/agents/uploads/${encodeURIComponent(id)}`, { signal })
  } catch (err) {
    if (err instanceof ApiError && err.status === 404) return null
    throw err
  }
}

export function uploadQuery(id: string) {
  return queryOptions({
    queryKey: buildKeys.upload(id),
    queryFn: ({ signal }) => fetchUpload(id, signal),
    retry: false,
    meta: { path: '/api/agents/uploads/{id}' },
  })
}

/** Polls while `poll` holds and the upload hasn't finished (completed or failed never change again). */
export function useUploadStatus(id: string, poll: boolean) {
  return useQuery({
    ...uploadQuery(id),
    // A 404 (null) means there is no such upload: nothing to wait for (the page says not found).
    refetchInterval: (q) =>
      poll && q.state.data !== null && !['completed', 'failed'].includes(q.state.data?.status ?? '')
        ? STREAM_FALLBACK_POLL_MS
        : false,
  })
}

const reasonLimiter = createLimiter(REASON_READS)

/**
 * One-line failure reasons for failed rows on the visible page (design review 2): a finished build's upload row never
 * changes, so each is read once and kept for the session.
 */
export function useFailureDetails(ids: readonly string[]) {
  const results = useQueries({
    queries: ids.map((id) => ({
      ...uploadQuery(id),
      queryFn: ({ signal }: { signal: AbortSignal }) =>
        reasonLimiter(signal, () => fetchUpload(id, signal)),
      staleTime: Infinity,
      gcTime: REASON_GC_MS,
    })),
  })
  return new Map(ids.map((id, i) => [id, results[i]?.data?.error_details ?? null] as const))
}

/** The recent uploads the background follower (eng review R8) and M2 read; enveloped `{data, status_code, message}`. */
export function recentUploadsQuery(limit: number) {
  return queryOptions({
    queryKey: ['builds', 'uploads', limit] as const,
    queryFn: ({ signal }) =>
      apiData<UploadStatus[]>(`/api/agents/uploads?limit=${limit}`, { signal }),
    retry: false,
    meta: { path: '/api/agents/uploads' },
  })
}

export type StreamState =
  | { kind: 'connecting' }
  | { kind: 'live'; status: BuildStatus | 'not_found' }
  | { kind: 'dropped'; status: BuildStatus | 'not_found' | null }
  | { kind: 'noRights' }
  | { kind: 'done'; status: BuildStatus | 'not_found' }

const TERMINAL = new Set(['success', 'failed', 'not_found'])

/**
 * The open Build page's live status (§5): `GET /api/agents/deploys/{id}/stream` sends `{status, build_id}` on change
 * and closes after success, failed or not_found. A dropped stream isn't reopened: the page falls back to polling the
 * upload row (the server's own pace is a 3 s DB poll, so nothing is lost). A 401 re-checks `me`, whose 401 takes the
 * app's one expiry path (queryClient.ts).
 */
export function useBuildStream(id: string, enabled: boolean): StreamState {
  // One build per mount: the Build page is keyed by build id (routes/_app/builds.$buildId.tsx), so a new id remounts.
  const [state, setState] = useState<StreamState>({ kind: 'connecting' })
  const qc = useQueryClient()
  const last = useRef<BuildStatus | 'not_found' | null>(null)
  useEffect(() => {
    if (!enabled) return
    const ctrl = new AbortController()
    // Set false in cleanup: a stream torn down by unmount or a new id never writes state again.
    let alive = true
    last.current = null
    const run = async () => {
      const res = await fetch(
        new URL(
          `/api/agents/deploys/${encodeURIComponent(id)}/stream`,
          globalThis.location?.origin ?? 'http://localhost',
        ),
        {
          credentials: 'same-origin',
          headers: { Accept: 'text/event-stream' },
          signal: ctrl.signal,
        },
      )
      if (res.status === 401) {
        void qc.invalidateQueries({ queryKey: ['me'] })
        throw new Error('unauthorized')
      }
      if (!res.ok) throw new Error(`stream ${res.status}`)
      // Below deployer role the route answers a JSON `{available:false}`, not a stream.
      if ((res.headers.get('content-type') ?? '').includes('application/json')) {
        const body: unknown = await res.json().catch(() => null)
        if (unavailableSchema.safeParse(body).success) return setState({ kind: 'noRights' })
        throw new Error('unexpected stream body')
      }
      await readSse(
        res,
        (events) => {
          for (const e of events) {
            let data: unknown
            try {
              data = JSON.parse(e.data)
            } catch {
              continue
            }
            const frame = streamFrameSchema.safeParse(data)
            if (!frame.success) continue
            last.current = frame.data.status
            setState(
              TERMINAL.has(frame.data.status)
                ? { kind: 'done', status: frame.data.status }
                : { kind: 'live', status: frame.data.status },
            )
            if (TERMINAL.has(frame.data.status)) {
              // The build moved on: re-read the record and the upload row (its error details name the fix).
              void qc.invalidateQueries({ queryKey: buildKeys.detail(id) })
              void qc.invalidateQueries({ queryKey: buildKeys.upload(id) })
              ctrl.abort()
            }
          }
        },
        { signal: ctrl.signal },
      )
      if (!last.current || !TERMINAL.has(last.current)) throw new Error('stream ended early')
    }
    run().catch(() => {
      if (!alive || (last.current && TERMINAL.has(last.current))) return
      setState((s) =>
        s.kind === 'done' || s.kind === 'noRights' ? s : { kind: 'dropped', status: last.current },
      )
    })
    return () => {
      alive = false
      ctrl.abort()
    }
  }, [id, enabled, qc])
  return state
}

// ── GitHub (plans/feat-deploy.md §4.2) ─────────────────────────────────────────────────────────

export const githubKeys = {
  configured: ['deploy', 'github', 'configured'] as const,
  user: ['deploy', 'github', 'user'] as const,
  repos: ['deploy', 'github', 'repos'] as const,
}

export function useGithubConfigured() {
  return useQuery({
    queryKey: githubKeys.configured,
    queryFn: async ({ signal }) =>
      githubConfiguredSchema.parse(await apiFetch<unknown>('/api/auth/github/status', { signal }))
        .configured,
    retry: false,
    staleTime: GITHUB_CONFIGURED_STALE_MS,
    meta: { path: '/api/auth/github/status' },
  })
}

export function useGithubUser(enabled: boolean) {
  return useQuery({
    queryKey: githubKeys.user,
    queryFn: async ({ signal }) =>
      githubUserSchema.parse(await apiFetch<unknown>('/api/github/user', { signal })),
    enabled,
    retry: false,
    meta: { path: '/api/github/user' },
  })
}

export function useGithubRepos(enabled: boolean) {
  return useQuery({
    queryKey: githubKeys.repos,
    queryFn: async ({ signal }) =>
      githubReposSchema.parse(await apiFetch<unknown>('/api/github/repositories', { signal }))
        .repositories,
    enabled,
    retry: false,
    staleTime: GITHUB_REPOS_STALE_MS,
    meta: { path: '/api/github/repositories' },
  })
}

/**
 * The OAuth URL to open in the popup: https only (defence in depth; the server builds it from GitHub's authorize URL).
 * `about:` is allowed too: it can't run script, and the mock uses about:blank so mock mode never loads github.com.
 */
export async function fetchGithubLoginUrl(): Promise<string> {
  const { auth_url } = await apiFetch<{ auth_url: string }>('/api/github/login')
  if (!['https:', 'about:'].includes(new URL(auth_url).protocol))
    throw new Error('GitHub login URL is not https')
  return auth_url
}

/** One poll of the connection (`github_token`): true once connected with a valid token. */
export async function githubConnectedNow(): Promise<boolean> {
  const body = githubTokenSchema.safeParse(await apiFetch<unknown>('/api/auth/github/token'))
  return body.success && body.data.status === 'connected'
}

export function useGithubLogout() {
  const qc = useQueryClient()
  return useMutation({
    mutationFn: () => apiFetch<unknown>('/api/github/logout', { method: 'DELETE' }),
    onSettled: () => {
      void qc.invalidateQueries({ queryKey: githubKeys.user })
      qc.removeQueries({ queryKey: githubKeys.repos })
    },
  })
}

export interface CloneRequest {
  repository_full_name: string
  branch: string
  agent_name: string
  version_override?: string
}

/** `POST /api/github/clone`: 202 `{upload_id}` (the build id); errors are plain text (403, 422, 400, 503). */
export function useGithubClone() {
  const qc = useQueryClient()
  return useMutation({
    mutationFn: async (body: CloneRequest) => {
      const res = cloneResultSchema.parse(
        await apiFetch<unknown>('/api/github/clone', {
          method: 'POST',
          headers: { 'Content-Type': 'application/json' },
          body: JSON.stringify(body),
        }),
      )
      if (!res.upload_id) throw new ApiError(202, res, '/api/github/clone', res.message)
      return res.upload_id
    },
    // Followed in the background until it finishes (design review 8), even if the page is gone by then.
    // A clone that answers after sign-out (or another account's sign-in) cleared this tab's deploy work is not followed.
    onMutate: () => ({ epoch: uploadsEpoch() }),
    onSuccess: (buildId, body, ctx) => {
      if (ctx?.epoch !== uploadsEpoch()) return
      rememberClone(buildId, body.repository_full_name)
      follow(qc, buildId, body.agent_name)
    },
  })
}

/** `POST /api/import/registry` (synchronous build or pull + deploy; `catalog/import.rs`). */
export const sendRegistryImport = (reference: string) =>
  apiFetch<components['schemas']['ImportResult']>(IMPORT_PATH, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({ reference }),
  })
