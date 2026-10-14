// Ported from nasiko-ui-lab@1b99bdda common/src/features/trace/api.ts. Local edits: the span key is scoped,
// `productKey(scope, "span", traceId, hex)` (the lab's `['span', traceId, hex]`), so `spanQuery` and the trace reads
// (`traceQuery`) take the scope and hooks read it from `useProductScope()`; the waste-cost map is typed from the
// combined array (`noUncheckedIndexedAccess` here); Prettier formatting.
/**
 * Trace page queries. `span/{trace}/{span}` matches the HEX span id. Retry-waste SpanDetail
 * fetches run at most 2 at a time with no retries (a failure falls back to token wording).
 */
import { useQueries, useQuery, useQueryClient, type UseQueryResult } from "@tanstack/react-query";
import { useMemo, useRef } from "react";
import type { Scope } from "@nasiko/data-access";
import { apiFetch, productKey, useProductScope } from "@nasiko/data-access/product";
import { createLimiter } from "../observability/limiter";
import { ARRIVAL_POLL_MS, ARRIVAL_POLLS, RETRY_FETCH_CONCURRENCY } from "../observability/tuning";
import { flattenAttributes, isTraceFailing, type FlatSpan } from "../observability/spans";
import type { SpanDetail, SpanDetailResponse, TraceDetail } from "../observability/types";
import { traceQuery } from "../sessions/api";

const OBS = "/api/observability";

export const traceKeys = {
  span: (scope: Scope, traceId: string, hex: string) => productKey(scope, "span", traceId, hex),
};

export function spanQuery(scope: Scope, traceId: string, hex: string) {
  const path = `${OBS}/span/${encodeURIComponent(traceId)}/${encodeURIComponent(hex)}`;
  return {
    queryKey: traceKeys.span(scope, traceId, hex),
    queryFn: ({ signal }: { signal: AbortSignal }) =>
      apiFetch<SpanDetailResponse>(path, { signal }).then((b) => b.data.span),
    meta: { path },
    staleTime: Infinity,
  };
}

export function useSpanDetail(traceId: string | undefined, hex: string | undefined) {
  return useQuery({
    ...spanQuery(useProductScope(), traceId ?? "", hex ?? ""),
    enabled: !!traceId && !!hex,
  });
}

const limited = createLimiter(RETRY_FETCH_CONCURRENCY);

// `combine` output is structurally shared, but only plain arrays and objects are (a Map never
// is): the combiners return arrays, so an unchanged answer keeps its identity, and the Maps
// built from them below stay the same object across renders. Module-level, so they re-run only
// when a query result changes.
type SpanResults = UseQueryResult<SpanDetail>[];
const wasteCostsOf = (rs: SpanResults) =>
  rs.every((r) => r.data) ? rs.map((r) => (r.data as SpanDetail).cost_summary.total.cost) : null;
const agentIdsOf = (rs: SpanResults) =>
  rs.map((r) => {
    const target = flattenAttributes(r.data?.attributes)["agent.id"];
    return typeof target === "string" ? target : null;
  });

/** SpanDetail costs for the retry-waste spans; undefined until every fetch succeeded. Per-span fan-outs never retry. */
export function useWasteCosts(
  traceId: string | undefined,
  spans: FlatSpan[],
  enabled: boolean,
): Map<string, number> | undefined {
  const scope = useProductScope();
  const costs = useQueries({
    queries: spans.map((s) => {
      const base = spanQuery(scope, traceId ?? "", s.node.span_id);
      return {
        ...base,
        queryFn: (ctx: { signal: AbortSignal }) => limited(ctx.signal, () => base.queryFn(ctx)),
        enabled: enabled && !!traceId,
        retry: false,
      };
    }),
    combine: wasteCostsOf,
  });
  return useMemo(
    () =>
      enabled && spans.length && costs
        ? new Map(spans.map((s, i): [string, number] => [s.node.id, costs[i] ?? 0]))
        : undefined,
    [enabled, spans, costs],
  );
}

/**
 * The selected trace. While spans may still be arriving (an in-progress session), re-poll
 * every ARRIVAL_POLL_MS, at most ARRIVAL_POLLS times, until `num_spans` stops changing.
 * This is the page's one polling exception besides Live; it stops on unmount.
 */
export function useTraceDetail(traceId: string | undefined, inProgress: boolean) {
  const client = useQueryClient();
  const base = traceQuery(useProductScope(), traceId ?? "");
  // One polling budget per mount and trace: the query cache (shared with the Sessions status
  // checks) outlives the page, so its update count can't be the budget. Per trace id: fetch
  // attempts so far, and whether the last good one changed `num_spans`. Only the query's
  // callbacks touch it, never render.
  const arrival = useRef(new Map<string, { fetches: number; changed: boolean }>());
  const key = traceId ?? "";
  return useQuery({
    ...base,
    // Each fetch records whether it changed the span count, so `refetchInterval` stays a
    // pure read: TanStack evaluates it on every observer update, not once per response.
    // A failed attempt spends the budget too, so a failing trace store can't keep an open
    // in-progress trace re-polling until the page closes.
    queryFn: async (ctx: { signal: AbortSignal }) => {
      const before = client.getQueryData<TraceDetail>(base.queryKey)?.num_spans;
      const prev = arrival.current.get(key);
      const next = { fetches: (prev?.fetches ?? 0) + 1, changed: prev?.changed ?? true };
      try {
        const data = await base.queryFn(ctx);
        next.changed = before === undefined || before !== data.num_spans;
        return data;
      } finally {
        arrival.current.set(key, next);
      }
    },
    enabled: !!traceId,
    staleTime: inProgress ? 0 : Infinity,
    // This visit's first fetch plus at most ARRIVAL_POLLS re-polls, while spans keep arriving.
    refetchInterval: (q) => {
      const a = arrival.current.get(key);
      return inProgress &&
        q.state.data &&
        (a?.fetches ?? 0) <= ARRIVAL_POLLS &&
        a?.changed !== false
        ? ARRIVAL_POLL_MS
        : false;
    },
  });
}

/**
 * Whether the cache already holds this trace as failing. Subscribes without fetching, so a
 * verdict that lands later re-renders (a render-time `getQueryData` would be memoised stale).
 */
export function useKnownFailing(traceId: string | undefined): boolean {
  return (
    useQuery({
      ...traceQuery(useProductScope(), traceId ?? ""),
      enabled: false,
      select: isTraceFailing,
    }).data === true
  );
}

/**
 * Agent-call spans' callee ids, by span id. The proxy span carries `agent.id` = the callee's
 * UUID on SpanDetail attributes (agent_proxy.rs); SpanNode has no attributes.
 */
export function useAgentCallTargets(
  traceId: string | undefined,
  spans: FlatSpan[],
): Map<string, string> {
  const scope = useProductScope();
  const targets = useQueries({
    queries: spans.map((s) => ({
      ...spanQuery(scope, traceId ?? "", s.node.span_id),
      enabled: !!traceId,
      retry: false,
    })),
    combine: agentIdsOf,
  });
  return useMemo(() => {
    const out = new Map<string, string>();
    spans.forEach((s, i) => {
      const target = targets[i];
      if (target) out.set(s.node.id, target);
    });
    return out;
  }, [spans, targets]);
}
