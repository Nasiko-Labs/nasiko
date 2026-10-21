// Ported from nasiko-ui-lab@1b99bdda common/src/features/tokenops/api.ts. Local edits: every query key is scoped,
// `productKey(scope, "tokenops", <the lab's key parts>)` (`providers` is `productKey(scope, "providers")`, still outside
// the "tokenops" feature as in the lab), so the `*Query` builders take `scope` first and the hooks read it from
// `useProductScope()`; `dayQuery`/`savingsQuery` are split out of `useDay`/`useSavings` (development 49950eb1e: GenUI
// reads them); `apiData`/`apiFetch`/`withQuery` come from `@nasiko/data-access/product`; Prettier formatting.
/**
 * TokenOps queries. Every key contains the resolved window key plus every filter, so
 * a stale response for an old filter set can never render for a new one (plan A17).
 */
import { keepPreviousData, queryOptions, useQuery } from "@tanstack/react-query";
import {
  apiData,
  apiFetch,
  productKey,
  useProductScope,
  withQuery,
} from "@nasiko/data-access/product";
import type { Scope } from "@nasiko/data-access";
import type {
  FinopsDashboardData,
  FinopsDayDrilldown,
  FinopsSpendCalendar,
  FinopsSpendTimeseries,
  ProviderCatalogEntry,
  SavingsData,
  TopTracesData,
} from "./types";
import { finopsDashboardSchema, finopsDaySchema, providerCatalogSchema } from "./types";
import type { ApiWindowParams, ResolvedWindow } from "./window";
import { dedupeCatalog } from "./catalog";

const FINOPS = "/api/observability/finops";

/** Rows requested from the (proposed) top-traces endpoint; the drawer copy reads this too. */
export const TOP_TRACES_LIMIT = 25;

export interface Filters {
  agent?: string | undefined;
  provider?: string | undefined;
  model?: string | undefined;
}

function filterParams(f: Filters) {
  return { agent_id: f.agent, provider: f.provider, model: f.model };
}

/** The feature every TokenOps key sits under (`productPrefix(scope, TOKENOPS_FEATURE)` matches them all). */
export const TOKENOPS_FEATURE = "tokenops";

const tokenopsKeys = {
  dashboard: (scope: Scope, w: string, f: Filters, view: string, compare = false) =>
    productKey(scope, TOKENOPS_FEATURE, "dashboard", w, f, view, compare),
  timeseries: (scope: Scope, w: string, f: Filters, shape = "") =>
    productKey(scope, TOKENOPS_FEATURE, "timeseries", w, f, shape),
  calendar: (scope: Scope, month: string, f: Filters) =>
    productKey(scope, TOKENOPS_FEATURE, "calendar", month, f),
  day: (scope: Scope, date: string, f: Filters) =>
    productKey(scope, TOKENOPS_FEATURE, "day", date, f),
  topTraces: (scope: Scope, w: string, f: Filters) =>
    productKey(scope, TOKENOPS_FEATURE, "top-traces", w, f),
  savings: (scope: Scope, w: string, f: Filters, s: string) =>
    productKey(scope, TOKENOPS_FEATURE, "savings", w, f, s),
  /** Outside the 'tokenops' root: the router shares the catalog, and TokenOps' return refresh leaves it. */
  providers: (scope: Scope) => productKey(scope, "providers"),
};

function dashboardPath(params: ApiWindowParams, f: Filters, view: string, compare: boolean) {
  return withQuery(`${FINOPS}/dashboard`, {
    ...params,
    ...filterParams(f),
    view,
    ...(compare ? { compare: "previous" } : {}),
  });
}

export function dashboardQuery(
  scope: Scope,
  win: ResolvedWindow,
  f: Filters,
  view: "agent" | "workflow",
  which: "current" | "previous",
  /**
   * Ask the server for each agent's preceding-window totals in the same
   * response (`compare=previous`), instead of issuing a second whole dashboard
   * request for the earlier window. Only meaningful with `which: 'current'` —
   * the server compares against the window it was given.
   */
  compare = false,
) {
  const params = which === "current" ? win.params : win.prevParams;
  const windowKey =
    which === "current"
      ? win.key
      : `prev|${win.prevStart.toISOString()}|${win.prevEnd.toISOString()}`;
  const path = dashboardPath(params, f, view, compare);
  return queryOptions({
    queryKey: tokenopsKeys.dashboard(scope, windowKey, f, view, compare),
    queryFn: ({ signal }) =>
      apiData<FinopsDashboardData>(path, { signal, schema: finopsDashboardSchema }),
    meta: { path },
  });
}

export function useDashboard(
  win: ResolvedWindow,
  f: Filters,
  view: "agent" | "workflow",
  which: "current" | "previous",
  enabled: boolean,
  compare = false,
) {
  const scope = useProductScope();
  return useQuery({
    ...dashboardQuery(scope, win, f, view, which, compare),
    placeholderData: keepPreviousData,
    enabled,
  });
}

/** Extra shapes the server can fold into one spend-timeseries response. */
export interface TimeseriesShape {
  /** Per-agent series for the top `top` spenders plus an `other` aggregate. */
  groupByAgent?: boolean;
  top?: number;
  /** The preceding window of the same length, in the same response. */
  compare?: boolean;
}

export function timeseriesQuery(
  scope: Scope,
  win: ResolvedWindow,
  f: Filters,
  shape: TimeseriesShape = {},
) {
  const path = withQuery(`${FINOPS}/spend-timeseries`, {
    ...win.params,
    ...filterParams(f),
    ...(shape.groupByAgent ? { group_by: "agent" } : {}),
    ...(shape.groupByAgent && shape.top ? { top: String(shape.top) } : {}),
    ...(shape.compare ? { compare: "previous" } : {}),
  });
  return queryOptions({
    // The shape changes the payload, so it has to be part of the key — two
    // callers on the same window must not share a cache entry that is missing
    // the series one of them needs.
    queryKey: tokenopsKeys.timeseries(
      scope,
      win.key,
      f,
      `${shape.groupByAgent ? `agent:${shape.top ?? ""}` : ""}${shape.compare ? "+prev" : ""}`,
    ),
    queryFn: ({ signal }) => apiData<FinopsSpendTimeseries>(path, { signal }),
    meta: { path },
  });
}

export function useTimeseries(
  win: ResolvedWindow,
  f: Filters,
  enabled: boolean,
  shape: TimeseriesShape = {},
) {
  const scope = useProductScope();
  return useQuery({
    ...timeseriesQuery(scope, win, f, shape),
    placeholderData: keepPreviousData,
    enabled,
  });
}

export function calendarQuery(scope: Scope, month: string, f: Filters) {
  const path = withQuery(`${FINOPS}/spend-calendar`, { month, ...filterParams(f) });
  return queryOptions({
    queryKey: tokenopsKeys.calendar(scope, month, f),
    queryFn: ({ signal }) => apiData<FinopsSpendCalendar>(path, { signal }),
    staleTime: 60_000,
    meta: { path },
  });
}

export function useCalendar(month: string, f: Filters, enabled: boolean) {
  const scope = useProductScope();
  return useQuery({
    ...calendarQuery(scope, month, f),
    placeholderData: keepPreviousData,
    enabled,
  });
}

export function dayQuery(scope: Scope, date: string | undefined, f: Filters) {
  const path = withQuery(`${FINOPS}/spend-calendar/day`, { date, ...filterParams(f) });
  return queryOptions({
    queryKey: tokenopsKeys.day(scope, date ?? "", f),
    queryFn: ({ signal }) => apiData<FinopsDayDrilldown>(path, { signal, schema: finopsDaySchema }),
    meta: { path },
  });
}

export function useDay(date: string | undefined, f: Filters, enabled: boolean) {
  return useQuery({ ...dayQuery(useProductScope(), date, f), enabled: enabled && !!date });
}

/** PROPOSED endpoint — live servers return 404 until it exists (handled by the drawer). */
export function useTopTraces(win: ResolvedWindow, f: Filters, enabled: boolean) {
  const scope = useProductScope();
  const path = withQuery(`${FINOPS}/top-traces`, {
    ...win.params,
    ...filterParams(f),
    sort_by: "cost",
    limit: TOP_TRACES_LIMIT,
  });
  return useQuery({
    queryKey: tokenopsKeys.topTraces(scope, win.key, f),
    queryFn: ({ signal }) => apiData<TopTracesData>(path, { signal }),
    enabled,
    meta: { path },
  });
}

export function providersQuery(scope: Scope) {
  return queryOptions({
    queryKey: tokenopsKeys.providers(scope),
    // Overlapping open pricing rows collapse to one entry per model (catalog.ts).
    queryFn: ({ signal }) =>
      apiFetch<{ data: ProviderCatalogEntry[] }>("/api/llm-router/providers", {
        signal,
        schema: providerCatalogSchema,
      }).then((b) => dedupeCatalog(b.data)),
    staleTime: 60_000,
  });
}

export function useProviders(enabled: boolean) {
  const scope = useProductScope();
  return useQuery({ ...providersQuery(scope), enabled });
}

/**
 * Token-optimisation savings for the window. `scope` picks the rollup; every scope returns the
 * total and the category breakdown, so the panel needs one request rather than three.
 */
export function savingsQuery(
  product: Scope,
  win: ResolvedWindow,
  f: Filters,
  scope: "total" | "agent" | "session",
) {
  const path = withQuery(`${FINOPS}/savings`, { ...win.params, ...filterParams(f), scope });
  return queryOptions({
    queryKey: tokenopsKeys.savings(product, win.key, f, scope),
    queryFn: ({ signal }) => apiData<SavingsData>(path, { signal }),
    meta: { path },
  });
}

export function useSavings(
  win: ResolvedWindow,
  f: Filters,
  scope: "total" | "agent" | "session",
  enabled: boolean,
) {
  return useQuery({
    ...savingsQuery(useProductScope(), win, f, scope),
    placeholderData: keepPreviousData,
    enabled,
  });
}

/**
 * Savings for one session.
 *
 * A fixed 90-day lookback rather than the page's range: a session page has no range control, and a
 * conversation opened from a link can be older than any default window — returning nothing for it
 * would read as "this session saved nothing" rather than "you are looking outside the window".
 */
export function useSessionSavings(sessionId: string, enabled = true) {
  const scope = useProductScope();
  return useQuery({
    queryKey: productKey(scope, TOKENOPS_FEATURE, "savings", "session", sessionId),
    // The window is resolved here, not during render: a clock read in the component body is impure,
    // and a start_time that moved every render would change the key and refetch forever.
    queryFn: ({ signal }) =>
      apiData<SavingsData>(
        withQuery(`${FINOPS}/savings`, {
          session_id: sessionId,
          start_time: new Date(Date.now() - 90 * 86_400_000).toISOString(),
        }),
        { signal },
      ),
    enabled: enabled && !!sessionId,
  });
}
