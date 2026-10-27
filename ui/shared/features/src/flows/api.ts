/**
 * Flows queries (plans/feat-flows.md; eng review A1, A2, A4, O4, O5). Every read is under `['flows', …]`, except the
 * trace and span-detail reads, which share the trace page's cache keys.
 */
import {
  queryOptions,
  useQueries,
  useQuery,
  useQueryClient,
  type UseQueryResult,
} from '@tanstack/react-query'
import { useMemo, useState } from 'react'
import { useAgentsDirectory } from '@nasiko/features/agents/api'
import { createLimiter } from '@nasiko/features/observability/limiter'
import {
  flattenAttributes,
  flattenSpans,
  type FlatSpan,
} from '@nasiko/features/observability/spans'
import type { SpanDetail, TraceDetail } from '@nasiko/features/observability/types'
import { traceQuery } from '@nasiko/features/sessions/api'
import { spanQuery } from '@nasiko/features/trace/api'
import { ApiError, apiFetch, withQuery } from '@nasiko/data-access/client'
import { callsFromSpans, callsFromSteps, PROXY_SPAN, proxySpans, type Call } from './calls'
import { mergeCalls } from './merge'
import {
  flowState,
  isWorking,
  lastActivityMs,
  pollMs,
  type FlowState,
  type TraceTiming,
} from './status'
import {
  LIST_CAP,
  LIST_PAGE,
  SPAN_DETAIL_CONCURRENCY,
  TRACE_CALL_CAP,
  TRACE_FINAL_READ_MS,
  TRACE_OVERDUE_MS,
  TRACE_REREAD_MS,
} from './tuning'
import { flowKeys } from './queryKeys'
import { flowDetailSchema, flowListSchema, type Flow, type FlowDetail } from './types'

export { flowKeys } from './queryKeys'

// ---------------------------------------------------------------------------------------------------------------
// The list (A2): the server filters only status and q, newest first, so the page reads back to the window start.

export interface FlowListParams {
  status?: string
  q?: string
  /** The window start, epoch ms: reading stops at the first older flow. */
  sinceMs: number
}

export interface FlowListResult {
  rows: Flow[]
  /** Every flow in the window was read. */
  complete: boolean
  /** Reading stopped at LIST_CAP flows. */
  capped: boolean
  /** A later page failed: `rows` are the pages before it (Retry reads again). */
  error: unknown
}

export async function readFlowsBack(
  p: FlowListParams,
  signal: AbortSignal,
): Promise<FlowListResult> {
  const rows: Flow[] = []
  // Offset paging over a live, newest-first list: a flow created between page reads shifts a row onto the next page
  // too. Each flow counts once (charts, percentiles and the table's keys).
  const seen = new Set<string>()
  for (let offset = 0; offset < LIST_CAP; offset += LIST_PAGE) {
    const path = withQuery('/api/flows', {
      status: p.status || undefined,
      q: p.q || undefined,
      limit: LIST_PAGE,
      offset,
    })
    let page: { data: Flow[] }
    try {
      page = await apiFetch(path, { signal, schema: flowListSchema })
    } catch (err) {
      // The first page failing is the list's error; a later one keeps what was read. A 401 always throws, so the
      // app's one expiry path (queryClient.ts) clears the cache and signs out.
      if (!rows.length || signal.aborted || (err instanceof ApiError && err.status === 401))
        throw err
      return { rows, complete: false, capped: false, error: err }
    }
    for (const f of page.data) {
      if (Date.parse(f.created_at) < p.sinceMs)
        return { rows, complete: true, capped: false, error: null }
      if (seen.has(f.flow_id)) continue
      seen.add(f.flow_id)
      rows.push(f)
    }
    // `total` is the page length (Paginated::new), so a short page is the last one.
    if (page.data.length < LIST_PAGE) return { rows, complete: true, capped: false, error: null }
  }
  return { rows, complete: false, capped: true, error: null }
}

export function flowsListQuery(p: FlowListParams) {
  return queryOptions({
    queryKey: flowKeys.list(p),
    queryFn: ({ signal }) => readFlowsBack(p, signal),
    meta: { path: '/api/flows' },
  })
}

export const useFlowsList = (p: FlowListParams) => useQuery(flowsListQuery(p))

// ---------------------------------------------------------------------------------------------------------------
// One flow.

export function flowQuery(id: string) {
  const path = `/api/flows/${encodeURIComponent(id)}`
  return queryOptions({
    queryKey: flowKeys.detail(id),
    queryFn: ({ signal }) => apiFetch<FlowDetail>(path, { signal, schema: flowDetailSchema }),
    meta: { path },
  })
}

/**
 * What the trace says about the flow's end (O5): its agent calls' latest end and whether any is still open. Only
 * `a2a.proxy` spans count: the dispatch span closing a moment after the flow was marked completed is normal.
 */
export function traceTiming(trace: TraceDetail | undefined): TraceTiming | null {
  if (!trace) return null
  let lastEndMs: number | null = null
  let open = false
  for (const s of Object.values(trace.span_lookup ?? {})) {
    if (s.name !== PROXY_SPAN) continue
    const end = s.end_time ? Date.parse(s.end_time) : NaN
    if (Number.isNaN(end)) open = true
    else lastEndMs = Math.max(lastEndMs ?? end, end)
  }
  return { lastEndMs, open }
}

/**
 * When to read the trace again (O4): every TRACE_REREAD_MS while the flow works, then once TRACE_FINAL_READ_MS after
 * it ended (spans land in Tempo late), then never. An overdue final read is scheduled a moment out, never at 0
 * (TanStack Query reads an interval of 0 as "off"). A 503 (no trace store) can't change, so it ends the re-reads; a
 * 404 keeps them while the flow works, since spans land late.
 */
export function traceRereadMs(
  working: boolean,
  endedAtMs: number | null,
  lastReadMs: number,
  now: number,
  errorStatus: number | null = null,
): number | false {
  if (errorStatus === 503) return false
  if (working) return TRACE_REREAD_MS
  if (endedAtMs === null) return false
  const due = endedAtMs + TRACE_FINAL_READ_MS
  return lastReadMs >= due ? false : Math.max(TRACE_OVERDUE_MS, due - now)
}

/**
 * A failed span-detail read is tried again with the trace while the flow works: its span may not have landed in the
 * trace store yet, and "Partial" shouldn't stick to a running flow until someone presses Retry. At rest it waits for
 * Retry.
 */
export const spanRereadMs = (working: boolean, status: string): number | false =>
  working && status === 'error' ? TRACE_REREAD_MS : false

type TraceState = 'loading' | 'ready' | 'partial' | 'unavailable' | 'expired'

interface TraceCalls {
  state: TraceState
  calls: Call[]
  /** Proxy spans past the cap: "N more calls in the trace" (A1). */
  overflow: number
  timing: TraceTiming | null
  retry: () => void
}

const spanLimit = createLimiter(SPAN_DETAIL_CONCURRENCY)

/** Plain arrays and objects, so an unchanged answer keeps its identity (structural sharing). */
const detailsOf = (rs: UseQueryResult<SpanDetail>[]) => ({
  ids: rs.map((r) => {
    const target = flattenAttributes(r.data?.attributes)['agent.id']
    return typeof target === 'string' ? target : null
  }),
  failed: rs.some((r) => r.isError),
  pending: rs.some((r) => r.isPending),
})

/**
 * Agent-to-agent calls from the flow's trace (A1, O4). The trace is read on open, re-read while the flow works and
 * once after it ends; only proxy spans not seen before get a SpanDetail read (cached forever), through one limiter,
 * and never more than TRACE_CALL_CAP per flow. 503 = Tempo isn't configured (recorded steps only, no caption); a 404
 * = the trace expired, unless the flow is still working (its spans haven't landed yet).
 */
function useFlowTraceCalls(
  flowId: string,
  opts: { working: boolean; endedAtMs: number | null; enabled: boolean },
): TraceCalls {
  const client = useQueryClient()
  const base = traceQuery(flowId)
  const trace = useQuery({
    ...base,
    // Only once the flow record answered: an id that isn't the caller's flow costs no trace or span reads.
    enabled: !!flowId && opts.enabled,
    // A probe: 503 and 404 are answers, not failures to retry.
    retry: false,
    refetchOnWindowFocus: false,
    refetchInterval: (q) =>
      // Any answered read counts, a failed one too: a 404 must not re-read every second.
      traceRereadMs(
        opts.working,
        opts.endedAtMs,
        Math.max(q.state.dataUpdatedAt, q.state.errorUpdatedAt),
        Date.now(),
        q.state.error instanceof ApiError ? q.state.error.status : null,
      ),
  })
  const spans = useMemo(() => (trace.data ? flattenSpans(trace.data) : []), [trace.data])
  const proxies = useMemo(() => proxySpans(spans), [spans])
  // The cap is per flow, across every re-read (A1, O4): spans admitted once stay admitted, and a span that lands late
  // with an earlier start can't push one off the chart or spend a 31st read ("adjust state on prop change", no effect).
  const [admitted, setAdmitted] = useState<readonly string[]>([])
  const nextAdmitted = admit(admitted, proxies, TRACE_CALL_CAP)
  if (nextAdmitted !== admitted) setAdmitted(nextAdmitted)
  const admittedSet = useMemo(() => new Set(nextAdmitted), [nextAdmitted])
  const capped = useMemo(
    () => proxies.filter((s) => admittedSet.has(s.node.id)),
    [proxies, admittedSet],
  )
  const detailQueries = capped.map((s) => {
    const q = spanQuery(flowId, s.node.span_id)
    return {
      ...q,
      queryFn: (ctx: { signal: AbortSignal }) => spanLimit(ctx.signal, () => q.queryFn(ctx)),
      // A fan-out behind a limiter: one try each (a failure shows "Partial" with Retry)…
      retry: false,
      // …but while the flow works a failed read is tried again with the trace: its span may not have landed yet.
      refetchInterval: (sq: { state: { status: string } }) =>
        spanRereadMs(opts.working, sq.state.status),
    }
  })
  const details = useQueries({ queries: detailQueries, combine: detailsOf })
  const directory = useAgentsDirectory()

  const calls = useMemo(() => {
    const byId = new Map<string, string>()
    capped.forEach((s, i) => {
      const t = details.ids[i]
      if (t) byId.set(s.node.id, t)
    })
    return callsFromSpans(spans, byId, (id) => directory.byId.get(id)?.name ?? null)
  }, [capped, details.ids, spans, directory.byId])

  const status = trace.error instanceof ApiError ? trace.error.status : null
  const state: TraceState = trace.isPending
    ? 'loading'
    : status === 503
      ? 'unavailable'
      : status === 404
        ? opts.working
          ? 'loading'
          : 'expired'
        : trace.isError || details.failed
          ? 'partial'
          : details.pending
            ? 'loading'
            : 'ready'

  return {
    state,
    calls,
    overflow: Math.max(0, proxies.length - capped.length),
    timing: traceTiming(trace.data),
    retry: () => {
      void trace.refetch()
      // The failed span-detail reads too (they don't retry on their own, and the same span ids won't refetch them).
      void client.refetchQueries({
        predicate: (q) =>
          q.queryKey[0] === 'span' && q.queryKey[1] === flowId && q.state.status === 'error',
      })
    },
  }
}

/** `admitted` plus newly seen spans, in start order, up to `cap`; the same array when nothing changes. */
export function admit(
  admitted: readonly string[],
  proxies: readonly FlatSpan[],
  cap: number,
): readonly string[] {
  if (admitted.length >= cap) return admitted
  const have = new Set(admitted)
  const add = proxies.filter((s) => !have.has(s.node.id)).slice(0, cap - admitted.length)
  return add.length ? [...admitted, ...add.map((s) => s.node.id)] : admitted
}

// ---------------------------------------------------------------------------------------------------------------
// The flow detail page's data: the record (polled by status, F13), the trace calls, the merge and the status.

/** @public The flow detail page's data (plan T3). */
export interface FlowView {
  query: UseQueryResult<FlowDetail>
  trace: TraceCalls
  /** Recorded steps merged with the trace's calls (F14, A5, O3). */
  calls: Call[]
  state: FlowState | null
}

/** A flow that's gone (404) or no longer yours (403) isn't re-read: the answer won't change. */
export const isGone = (err: unknown) =>
  err instanceof ApiError && (err.status === 404 || err.status === 403)

/**
 * When the flow ended, for the client's clock: the server's `completed_at`, or when this page first read it if that is
 * earlier (a client clock behind the server's would otherwise hold "Finishing" and the trace re-reads open).
 */
export function endedAt(completedAt: string | null | undefined, seenMs: number | null) {
  const t = completedAt ? Date.parse(completedAt) : NaN
  if (Number.isNaN(t)) return null
  return seenMs === null ? t : Math.min(t, seenMs)
}

/** @public The flow detail page (plan T3). */
export function useFlowView(flowId: string): FlowView {
  const client = useQueryClient()
  const traceKey = traceQuery(flowId).queryKey
  // When this page first read the flow's `completed_at` (its own clock), so client clock skew can't stretch
  // "Finishing" or the final trace read ("adjust state on prop change" below, no effect).
  const [doneSeen, setDoneSeen] = useState<{ at: string; ms: number } | null>(null)
  const seenFor = (at: string | null | undefined) =>
    at && doneSeen?.at === at ? doneSeen.ms : null
  const query = useQuery({
    ...flowQuery(flowId),
    // Re-read while the flow moves, by the status the page shows (A4, O5), visible tab only (F13); slowly once
    // nothing new has happened for a while; never once it's gone. The trace timing is read from the cache:
    // `refetchInterval` must stay a pure read.
    refetchInterval: (q) => {
      const d = q.state.data
      if (!d || isGone(q.state.error)) return false
      const t = traceTiming(client.getQueryData<TraceDetail>(traceKey))
      const now = Date.now()
      const status = flowState(d.flow, d.steps, t, now, seenFor(d.flow.completed_at)).status
      return pollMs(status, now - (lastActivityMs(d.flow, d.steps) ?? now))
    },
  })
  // The trace's timing without fetching (the trace hook below owns the read), so a verdict that lands later
  // re-renders. The page's clock is the record's last read, never a render-time Date.now().
  const timing =
    useQuery({ ...traceQuery(flowId), enabled: false, select: traceTiming }).data ?? null
  const d = query.data
  const doneAt = d?.flow.completed_at ?? null
  if (doneAt && doneSeen?.at !== doneAt) setDoneSeen({ at: doneAt, ms: query.dataUpdatedAt })
  const seen = seenFor(doneAt)
  const state = d ? flowState(d.flow, d.steps, timing, query.dataUpdatedAt, seen) : null
  const trace = useFlowTraceCalls(flowId, {
    working: !!state && isWorking(state.status),
    endedAtMs: endedAt(doneAt, seen),
    enabled: !!d,
  })
  // Routed-chat steps carry only the agent's name (FL-9): a unique match in the directory gives its id.
  const directory = useAgentsDirectory()
  const calls = useMemo(() => {
    if (!d) return []
    const agentOf = (name: string) => {
      const same = directory.byNameAll.get(name)
      return same?.length === 1 ? (same[0]?.id ?? null) : null
    }
    return mergeCalls(callsFromSteps(d.steps, agentOf), trace.calls)
  }, [d, trace.calls, directory.byNameAll])
  return { query, trace, calls, state }
}
