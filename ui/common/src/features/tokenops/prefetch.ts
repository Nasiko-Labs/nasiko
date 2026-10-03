import type { QueryClient } from '@tanstack/react-query'
import { calendarQuery, dashboardQuery, providersQuery, timeseriesQuery, type Filters } from './api'
import type { TokenopsSearch } from './search'
import { monthKey, resolveWindow, utcMonthStart } from './window'

export type TokenopsDeps = Pick<
  TokenopsSearch,
  'preset' | 'from' | 'to' | 'view' | 'agent' | 'provider' | 'model'
>

/**
 * The route's preload prefetch (plan §8 Phase 3): hovering a TokenOps link starts every query the page's first
 * paint waits on, so the panels still fill together. Same keys
 * as the page, so the page reuses them. The window is resolved from the clock here and again, frozen, by the page;
 * both floor to the minute, so they share one key.
 * ponytail: a hover in one minute and a click in the next fetches that window twice; move the frozen "now" into
 * the route (loaderDeps + invalidate on return) if that ever shows up in request logs.
 */
export function prefetchTokenops(client: QueryClient, deps: TokenopsDeps, now = new Date()) {
  const win = resolveWindow(deps, now)
  const f: Filters = { agent: deps.agent, provider: deps.provider, model: deps.model }
  void client.prefetchQuery(dashboardQuery(win, f, deps.view, 'current'))
  void client.prefetchQuery(dashboardQuery(win, f, deps.view, 'previous'))
  void client.prefetchQuery(timeseriesQuery(win, f))
  void client.prefetchQuery(calendarQuery(monthKey(utcMonthStart(now)), f))
  void client.prefetchQuery(calendarQuery(monthKey(utcMonthStart(now, -1)), f))
  void client.prefetchQuery(providersQuery)
}
