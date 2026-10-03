/**
 * TokenOps queries. Every key contains the resolved window key plus every filter, so
 * a stale response for an old filter set can never render for a new one (plan A17).
 */
import { keepPreviousData, queryOptions, useQuery } from '@tanstack/react-query'
import { apiData, apiFetch, withQuery } from '@/lib/api/client'
import type {
  FinopsDashboardData,
  FinopsDayDrilldown,
  FinopsSpendCalendar,
  FinopsSpendTimeseries,
  ProviderCatalogEntry,
  SavingsData,
  TopTracesData,
} from './types'
import { finopsDashboardSchema, finopsDaySchema, providerCatalogSchema } from './types'
import type { ApiWindowParams, ResolvedWindow } from './window'
import { dedupeCatalog } from './catalog'

const FINOPS = '/api/observability/finops'

/** Rows requested from the (proposed) top-traces endpoint; the drawer copy reads this too. */
export const TOP_TRACES_LIMIT = 25

export interface Filters {
  agent?: string
  provider?: string
  model?: string
}

function filterParams(f: Filters) {
  return { agent_id: f.agent, provider: f.provider, model: f.model }
}

const tokenopsKeys = {
  dashboard: (w: string, f: Filters, view: string) =>
    ['tokenops', 'dashboard', w, f, view] as const,
  timeseries: (w: string, f: Filters) => ['tokenops', 'timeseries', w, f] as const,
  calendar: (month: string, f: Filters) => ['tokenops', 'calendar', month, f] as const,
  day: (date: string, f: Filters) => ['tokenops', 'day', date, f] as const,
  topTraces: (w: string, f: Filters) => ['tokenops', 'top-traces', w, f] as const,
  savings: (w: string, f: Filters, scope: string) => ['tokenops', 'savings', w, f, scope] as const,
  /** Outside the 'tokenops' root: the router shares the catalog, and TokenOps' return refresh leaves it. */
  providers: ['providers'] as const,
}

function dashboardPath(params: ApiWindowParams, f: Filters, view: string) {
  return withQuery(`${FINOPS}/dashboard`, { ...params, ...filterParams(f), view })
}

export function dashboardQuery(
  win: ResolvedWindow,
  f: Filters,
  view: 'agent' | 'workflow',
  which: 'current' | 'previous',
) {
  const params = which === 'current' ? win.params : win.prevParams
  const windowKey =
    which === 'current'
      ? win.key
      : `prev|${win.prevStart.toISOString()}|${win.prevEnd.toISOString()}`
  const path = dashboardPath(params, f, view)
  return queryOptions({
    queryKey: tokenopsKeys.dashboard(windowKey, f, view),
    queryFn: ({ signal }) =>
      apiData<FinopsDashboardData>(path, { signal, schema: finopsDashboardSchema }),
    meta: { path },
  })
}

export function useDashboard(
  win: ResolvedWindow,
  f: Filters,
  view: 'agent' | 'workflow',
  which: 'current' | 'previous',
  enabled: boolean,
) {
  return useQuery({
    ...dashboardQuery(win, f, view, which),
    placeholderData: keepPreviousData,
    enabled,
  })
}

export function timeseriesQuery(win: ResolvedWindow, f: Filters) {
  const path = withQuery(`${FINOPS}/spend-timeseries`, { ...win.params, ...filterParams(f) })
  return queryOptions({
    queryKey: tokenopsKeys.timeseries(win.key, f),
    queryFn: ({ signal }) => apiData<FinopsSpendTimeseries>(path, { signal }),
    meta: { path },
  })
}

export function useTimeseries(win: ResolvedWindow, f: Filters, enabled: boolean) {
  return useQuery({ ...timeseriesQuery(win, f), placeholderData: keepPreviousData, enabled })
}

export function calendarQuery(month: string, f: Filters) {
  const path = withQuery(`${FINOPS}/spend-calendar`, { month, ...filterParams(f) })
  return queryOptions({
    queryKey: tokenopsKeys.calendar(month, f),
    queryFn: ({ signal }) => apiData<FinopsSpendCalendar>(path, { signal }),
    staleTime: 60_000,
    meta: { path },
  })
}

export function useCalendar(month: string, f: Filters, enabled: boolean) {
  return useQuery({ ...calendarQuery(month, f), placeholderData: keepPreviousData, enabled })
}

export function useDay(date: string | undefined, f: Filters, enabled: boolean) {
  const path = withQuery(`${FINOPS}/spend-calendar/day`, { date, ...filterParams(f) })
  return useQuery({
    queryKey: tokenopsKeys.day(date ?? '', f),
    queryFn: ({ signal }) => apiData<FinopsDayDrilldown>(path, { signal, schema: finopsDaySchema }),
    enabled: enabled && !!date,
    meta: { path },
  })
}

/** PROPOSED endpoint — live servers return 404 until it exists (handled by the drawer). */
export function useTopTraces(win: ResolvedWindow, f: Filters, enabled: boolean) {
  const path = withQuery(`${FINOPS}/top-traces`, {
    ...win.params,
    ...filterParams(f),
    sort_by: 'cost',
    limit: TOP_TRACES_LIMIT,
  })
  return useQuery({
    queryKey: tokenopsKeys.topTraces(win.key, f),
    queryFn: ({ signal }) => apiData<TopTracesData>(path, { signal }),
    enabled,
    meta: { path },
  })
}

export const providersQuery = queryOptions({
  queryKey: tokenopsKeys.providers,
  // Overlapping open pricing rows collapse to one entry per model (catalog.ts).
  queryFn: ({ signal }) =>
    apiFetch<{ data: ProviderCatalogEntry[] }>('/api/llm-router/providers', {
      signal,
      schema: providerCatalogSchema,
    }).then((b) => dedupeCatalog(b.data)),
  staleTime: 60_000,
})

export function useProviders(enabled: boolean) {
  return useQuery({ ...providersQuery, enabled })
}

/**
 * Token-optimisation savings for the window. `scope` picks the rollup; every scope returns the
 * total and the category breakdown, so the panel needs one request rather than three.
 */
export function useSavings(
  win: ResolvedWindow,
  f: Filters,
  scope: 'total' | 'agent' | 'session',
  enabled: boolean,
) {
  const path = withQuery(`${FINOPS}/savings`, { ...win.params, ...filterParams(f), scope })
  return useQuery({
    queryKey: tokenopsKeys.savings(win.key, f, scope),
    queryFn: ({ signal }) => apiData<SavingsData>(path, { signal }),
    placeholderData: keepPreviousData,
    enabled,
    meta: { path },
  })
}

/**
 * Savings for one session.
 *
 * A fixed 90-day lookback rather than the page's range: a session page has no range control, and a
 * conversation opened from a link can be older than any default window — returning nothing for it
 * would read as "this session saved nothing" rather than "you are looking outside the window".
 */
export function useSessionSavings(sessionId: string, enabled = true) {
  return useQuery({
    queryKey: ['tokenops', 'savings', 'session', sessionId] as const,
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
  })
}
