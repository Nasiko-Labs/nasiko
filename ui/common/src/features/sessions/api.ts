/**
 * Sessions queries. The server's session/list takes only start_time/limit/offset
 * (newest first), so:
 * - fleet mode pages from the window start, 100 rows per page, "Load more";
 * - day mode scans pages from the day start until the server runs out or the page cap
 *   (3, extendable once to 6), then filters to the day and sorts client-side;
 * - status comes from TraceDetail per trace of the first rows (no status on the list).
 * Keys use absolute times (window key, UTC day), never "now", so Back reuses them.
 */
import {
  infiniteQueryOptions,
  useInfiniteQuery,
  useQueries,
  useQuery,
  useQueryClient,
  type QueryClient,
  type UseQueryResult,
} from '@tanstack/react-query'
import { useEffect, useMemo, useState } from 'react'
import { meQuery } from '@/lib/api/auth'
import { apiFetch, ApiError, withQuery } from '@/lib/api/client'
import { readSse } from '@/lib/sse'
import { bySize, foldDayScan, type ScanPage, type Status } from '@/features/observability/sessions'
import { isTraceFailing } from '@/features/observability/spans'
import { createLimiter } from '@/features/observability/limiter'
import {
  FLEET_PAGE_SIZE,
  FLEET_SEEK_PAGES,
  IN_PROGRESS_MS,
  LIVE_POLL_MS,
  SCAN_PAGE_SIZE,
  STATUS_CHECK_CONCURRENCY,
  STATUS_TRACES_PER_SESSION,
  TEMPO_MAX_SEARCH_MS,
  TEMPO_SAFETY_MS,
  WINDOW_FREEZE_MS,
} from '@/features/observability/tuning'
import { resolveWindow } from '@/features/tokenops/window'
import type { SessionsSearch } from './search'
import type {
  LogLine,
  SessionDetail,
  SessionDetailResponse,
  SessionListResponse,
  SessionSummary,
  TraceDetailResponse,
} from '@/features/observability/types'

const OBS = '/api/observability'

export const sessionKeys = {
  fleet: (windowKey: string) => ['sessions', 'fleet', windowKey] as const,
  day: (day: string) => ['sessions', 'day', day] as const,
  detail: (id: string) => ['sessions', 'detail', id] as const,
  trace: (id: string) => ['trace', id] as const,
}

function listPath(start: Date, limit: number, offset: number) {
  return withQuery(`${OBS}/session/list`, { start_time: start.toISOString(), limit, offset })
}

async function fetchPage(
  start: Date,
  limit: number,
  offset: number,
  signal: AbortSignal,
): Promise<ScanPage & { raw: SessionListResponse }> {
  const raw = await apiFetch<SessionListResponse>(listPath(start, limit, offset), { signal })
  if (!raw?.data?.sessions)
    throw new ApiError(
      200,
      raw,
      listPath(start, limit, offset),
      'session/list → unexpected response',
    )
  return { sessions: raw.data.sessions, hasNextPage: raw.data.pagination.has_next_page, raw }
}

/**
 * session/list's start_time for a window. A 7-day window (its start at Tempo's limit, give or take a minute) starts
 * TEMPO_SAFETY_MS later, so its trace lookups stay under the limit; it drops that first quarter hour. Older starts
 * are sent as they are, and the page explains the missing trace data.
 */
export function tempoSafeStart(start: Date, now: Date): Date {
  const floor = now.getTime() - TEMPO_MAX_SEARCH_MS + TEMPO_SAFETY_MS
  const t = start.getTime()
  return t < floor && t >= floor - 2 * TEMPO_SAFETY_MS ? new Date(floor) : start
}

let frozen: { key: string; at: Date } | null = null
/** One "now" per window key, reused for WINDOW_FREEZE_MS so remounts (and the route's prefetch) keep the same query key. */
export function frozenNow(key: string): Date {
  const t = Date.now()
  if (!frozen || frozen.key !== key || t - frozen.at.getTime() > WINDOW_FREEZE_MS)
    frozen = { key, at: new Date(t) }
  return frozen.at
}

function fleetQuery(windowKey: string, start: Date) {
  return infiniteQueryOptions({
    queryKey: sessionKeys.fleet(windowKey),
    queryFn: ({ pageParam, signal }) => fetchPage(start, FLEET_PAGE_SIZE, pageParam, signal),
    initialPageParam: 0,
    getNextPageParam: (last, pages) =>
      last.hasNextPage ? pages.length * FLEET_PAGE_SIZE : undefined,
    // The key's window is frozen (WINDOW_FREEZE_MS); new rows arrive through the live feed.
    staleTime: Infinity,
    meta: { path: listPath(start, FLEET_PAGE_SIZE, 0) },
  })
}

/** The route's preload prefetch: the fleet list's first page (day mode scans instead). */
export function prefetchSessions(
  client: QueryClient,
  search: Pick<SessionsSearch, 'preset' | 'from' | 'to' | 'day'>,
) {
  if (search.day) return
  const now = frozenNow(`${search.preset}|${search.from}|${search.to}`)
  const win = resolveWindow(search, now)
  void client.prefetchInfiniteQuery(fleetQuery(win.key, tempoSafeStart(win.start, now)))
}

/** Fleet mode: 100 rows per page from the window start; the range end is filtered client-side. */
export function useFleetSessions(windowKey: string, start: Date, end: Date, enabled: boolean) {
  const q = useInfiniteQuery({ ...fleetQuery(windowKey, start), enabled })
  const endMs = end.getTime()
  const rows = useMemo(() => {
    const seen = new Set<string>()
    return (q.data?.pages ?? [])
      .flatMap((p) => p.sessions)
      .filter((s) => {
        if (seen.has(s.session_id)) return false
        seen.add(s.session_id)
        return !s.start_time || Date.parse(s.start_time) <= endMs
      })
  }, [q.data, endMs])
  // A window that ended in the past: the newest rows are after its end, so page forward
  // (newest first) until a row inside the window shows up, at most FLEET_SEEK_PAGES.
  const pages = q.data?.pages ?? []
  const reachedWindow = pages.some((p) =>
    p.sessions.some((s) => !s.start_time || Date.parse(s.start_time) <= endMs),
  )
  const seeking =
    enabled && !reachedWindow && !!q.hasNextPage && pages.length < FLEET_SEEK_PAGES && !q.isError
  const { isFetchingNextPage, fetchNextPage } = q
  // pages.length is a dependency: the in-between "fetching" render can be batched away, so
  // the other values may be identical before and after a page lands.
  useEffect(() => {
    if (seeking && !isFetchingNextPage) void fetchNextPage()
  }, [seeking, isFetchingNextPage, fetchNextPage, pages.length])
  const first = q.data?.pages[0]?.raw.data
  return {
    ...q,
    rows,
    seeking,
    seekCapped: !reachedWindow && !!q.hasNextPage && pages.length >= FLEET_SEEK_PAGES,
    totalAgents: first?.total_agents,
    successfulAgents: first?.successful_agents,
  }
}

/**
 * Day mode: scan pages from the day start (newest first) until complete or `maxPages`.
 * `staleTime: Infinity`, so Back from a trace never rescans; the key is the absolute day.
 */
export function useDayScan(day: string | undefined, maxPages: number) {
  const start = day ? new Date(`${day}T00:00:00.000Z`) : new Date(0)
  const q = useInfiniteQuery({
    queryKey: sessionKeys.day(day ?? ''),
    queryFn: ({ pageParam, signal }) => fetchPage(start, SCAN_PAGE_SIZE, pageParam, signal),
    initialPageParam: 0,
    getNextPageParam: (last, pages) =>
      last.hasNextPage ? pages.length * SCAN_PAGE_SIZE : undefined,
    enabled: !!day,
    staleTime: Infinity,
    meta: { path: listPath(start, SCAN_PAGE_SIZE, 0) },
  })
  const pages = useMemo(() => q.data?.pages ?? [], [q.data])
  const scan = useMemo(
    () => (day ? foldDayScan(day, pages, maxPages) : null),
    [day, pages, maxPages],
  )
  const { hasNextPage, isFetchingNextPage, fetchNextPage, isError } = q
  // Keep paging until the scan is complete or capped; a failed page stops it (partial result).
  useEffect(() => {
    if (!scan || scan.complete || isError || isFetchingNextPage || !hasNextPage) return
    if (pages.length < maxPages) void fetchNextPage()
  }, [scan, pages.length, maxPages, hasNextPage, isFetchingNextPage, fetchNextPage, isError])
  const first = pages[0]?.raw.data
  return {
    ...q,
    scan,
    pages,
    totalAgents: first?.total_agents,
    successfulAgents: first?.successful_agents,
  }
}

// ─── status checks ──────────────────────────────────────────────────────────

/** At most STATUS_CHECK_CONCURRENCY status fetches in flight (the server does a Tempo scan per call). */
const limited = createLimiter(STATUS_CHECK_CONCURRENCY)

function sessionDetailQuery(id: string) {
  const path = `${OBS}/session/${encodeURIComponent(id)}`
  return {
    queryKey: sessionKeys.detail(id),
    queryFn: ({ signal }: { signal: AbortSignal }) =>
      apiFetch<SessionDetailResponse>(path, { signal }).then((b) => b.data.session),
    meta: { path },
  }
}

/** The trace route's preload prefetch: the session detail, the page's first request. */
export const prefetchSessionDetail = (client: QueryClient, id: string) =>
  void client.prefetchQuery({ ...sessionDetailQuery(id), staleTime: Infinity })

export function traceQuery(id: string) {
  const path = `${OBS}/trace/${encodeURIComponent(id)}`
  return {
    queryKey: sessionKeys.trace(id),
    queryFn: ({ signal }: { signal: AbortSignal }) =>
      apiFetch<TraceDetailResponse>(path, { signal }).then((b) => b.data.trace),
    meta: { path },
  }
}

const inProgress = (s: SessionSummary, now: number) =>
  !s.end_time || now - Date.parse(s.end_time) < IN_PROGRESS_MS

// The status fan-outs' combiners. `combine` output is structurally shared (plain arrays and
// objects only, never a Map), so equal answers keep their identity; module-level, so they
// re-run only when a query result changes.
/** Per session: the trace ids its status reads, 'error', or null while loading. */
const checkedTraceIds = (rs: UseQueryResult<SessionDetail>[]) =>
  rs.map((r) =>
    r.isError
      ? ('error' as const)
      : r.data
        ? bySize(r.data.traces)
            .slice(0, STATUS_TRACES_PER_SESSION)
            .map((t) => t.trace_id)
        : null,
  )
interface Verdict {
  failing: boolean | undefined
  error: boolean
}
const verdicts = (rs: UseQueryResult<boolean>[]): Verdict[] =>
  rs.map((r) => ({ failing: r.data, error: r.isError }))

/**
 * Status for the first rows: SessionDetail (first page of traces) → TraceDetail for its
 * STATUS_TRACES_PER_SESSION largest traces → failing if any has an unrecovered error.
 * Completed sessions are cached; in-progress ones re-check every LIVE_POLL_MS while Live runs. No retries: a failure
 * reads "status unknown".
 */
export function useSessionStatuses(
  rows: readonly SessionSummary[],
  now: number,
  recheck: boolean,
): Map<string, Status> {
  // Re-check only while Live runs, and only while the session is still in progress by the
  // real clock (`now` is the page's frozen window time).
  const pollWhile = (s: SessionSummary) => () =>
    recheck && inProgress(s, Date.now()) ? LIVE_POLL_MS : false
  const details = useQueries({
    queries: rows.map((s) => {
      const base = sessionDetailQuery(s.session_id)
      // In-progress sessions are re-checked on the live interval; finished ones are cached.
      // Per-row fan-outs never retry: a failing trace store gets one lookup per row, not three.
      return {
        ...base,
        queryFn: (ctx: { signal: AbortSignal }) => limited(ctx.signal, () => base.queryFn(ctx)),
        retry: false,
        staleTime: inProgress(s, now) ? 0 : Infinity,
        refetchInterval: pollWhile(s),
      }
    }),
    combine: checkedTraceIds,
  })
  const toCheck = details.flatMap((ids, i) =>
    Array.isArray(ids)
      ? ids.map((id) => ({ id, row: rows[i], live: inProgress(rows[i], now) }))
      : [],
  )
  const traces = useQueries({
    queries: toCheck.map(({ id, row, live }) => {
      const base = traceQuery(id)
      // `select` keeps only the verdict, so a refetch with the same answer doesn't re-render rows.
      return {
        ...base,
        queryFn: (ctx: { signal: AbortSignal }) => limited(ctx.signal, () => base.queryFn(ctx)),
        select: isTraceFailing,
        retry: false,
        staleTime: live ? 0 : Infinity,
        refetchInterval: pollWhile(row),
      }
    }),
    combine: verdicts,
  })
  // Both combined results keep their identity while the answers don't change, so this Map does too.
  return useMemo(() => {
    const byTrace = new Map<string, Verdict>()
    details
      .flatMap((ids) => (Array.isArray(ids) ? ids : []))
      .forEach((id, i) => byTrace.set(id, traces[i]))
    const out = new Map<string, Status>()
    rows.forEach((s, i) => {
      const ids = details[i]
      if (ids === 'error') return void out.set(s.session_id, 'unknown')
      if (!ids) return void out.set(s.session_id, 'checking')
      // No traces to look at: nothing says it failed or succeeded.
      if (!ids.length) return void out.set(s.session_id, 'unknown')
      const ts = ids.map((id) => byTrace.get(id))
      if (ts.some((t) => t?.failing === true)) return void out.set(s.session_id, 'failed')
      if (ts.some((t) => t?.error)) return void out.set(s.session_id, 'unknown')
      out.set(s.session_id, ts.some((t) => t?.failing === undefined) ? 'checking' : 'ok')
    })
    return out
  }, [rows, details, traces])
}

/** Row expand: the token split comes from SessionDetail (the list only has the total). */
export function useSessionDetail(id: string | undefined) {
  return useQuery({ ...sessionDetailQuery(id ?? ''), enabled: !!id, staleTime: Infinity })
}

// ─── log tail ───────────────────────────────────────────────────────────────

export type LogStreamState = 'connecting' | 'open' | 'closed' | 'unavailable' | 'error'

/**
 * Per-agent log tail: GET /api/observability/agents/{agent}/logs/stream (SSE), read with fetch
 * streaming through the shared decoder (`readSse`), not EventSource, so a 401/404 has a status
 * code to act on. Raw fetch, not a query: an open-ended stream has no cacheable result, and SSE
 * needs the raw Response (eslint.config.js allows fetch here for that reason).
 * - Well-formed lines go to `onLines` in batches; a malformed one is skipped, never the stream.
 * - 404 = deleted or not visible (`unavailable`).
 * - 401 re-checks the session the way the chat registry does: its `me` 401 runs the query
 *   client's expiry path (clear the cache, /login?expired=true, src/lib/queryClient.ts).
 * - The server ends streams after an hour (`event: close`): `closed`, no automatic retry;
 *   `reconnect()` opens a new stream.
 */
export function useLogStream(
  agent: string,
  open: boolean,
  onLines: (lines: LogLine[]) => void,
): { state: LogStreamState; reconnect: () => void } {
  const client = useQueryClient()
  const [state, setState] = useState<LogStreamState>('connecting')
  const [generation, setGeneration] = useState(0)
  useEffect(() => {
    if (!open) return
    const ctrl = new AbortController()
    void (async () => {
      setState('connecting')
      try {
        const res = await fetch(`${OBS}/agents/${encodeURIComponent(agent)}/logs/stream`, {
          signal: ctrl.signal,
          credentials: 'same-origin',
          headers: { Accept: 'text/event-stream' },
        })
        if (res.status === 401) {
          void client.fetchQuery({ ...meQuery, staleTime: 0 }).catch(() => undefined)
          setState('error')
          return
        }
        if (res.status === 404) return setState('unavailable')
        if (!res.ok || !res.body) return setState('error')
        setState('open')
        await readSse(
          res,
          (events) => {
            const batch: LogLine[] = []
            for (const e of events) {
              if (e.event === 'close') continue
              try {
                const line = JSON.parse(e.data) as Partial<LogLine> | null
                // Server data: keep only well-formed lines (a bad one must not end the stream).
                if (line && typeof line.message === 'string' && typeof line.timestamp === 'string')
                  batch.push(line as LogLine)
              } catch {
                /* skip malformed lines */
              }
            }
            onLines(batch)
          },
          { signal: ctrl.signal },
        )
        setState('closed')
      } catch (err) {
        if ((err as { name?: string }).name !== 'AbortError') setState('error')
      }
    })()
    return () => ctrl.abort()
  }, [open, agent, generation, onLines, client])
  return { state, reconnect: () => setGeneration((g) => g + 1) }
}
