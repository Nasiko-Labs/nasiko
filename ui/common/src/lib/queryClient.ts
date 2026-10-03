import {
  MutationCache,
  QueryCache,
  QueryClient,
  type QueryFilters,
  type QueryKey,
  type ResetOptions,
} from '@tanstack/react-query'
import type { AnyRouter } from '@tanstack/react-router'
import { isSigningOut } from './session'
import { ApiError, isRetryable } from './api/client'
import { isUnauthorized } from './api/detect'

const resets = new Set<() => void>()

/**
 * Client state that belongs to the session, like the agents lifecycle watches (a zustand store),
 * registers here to end with the cache: every full `clear()` (sign-out, sign-in, a 401 expiry,
 * another tab's sign-out) and every unfiltered `resetQueries()` (a full reset; another account signing in reloads the tab).
 * Shared code can't import a feature, so the feature registers itself when its module loads.
 */
export function onCacheReset(fn: () => void) {
  resets.add(fn)
}

class AppQueryClient extends QueryClient {
  override clear() {
    super.clear()
    for (const fn of resets) fn()
  }
  override resetQueries<K extends QueryKey = QueryKey>(
    filters?: QueryFilters<K>,
    options?: ResetOptions,
  ) {
    if (!filters) for (const fn of resets) fn()
    return super.resetQueries(filters, options)
  }
}

/**
 * The app's QueryClient. A 401 on any query or mutation means the session ended: clear the
 * cache and send the user to /login, returning to the same URL afterwards (plan A18).
 * Built by a factory so the page tests use the exact same behaviour.
 */
export function createQueryClient(
  getRouter: () => AnyRouter | undefined,
  opts: { retry?: boolean } = {},
): QueryClient {
  let redirecting = false
  const expired = (err: unknown) => {
    const router = getRouter()
    if (!router || redirecting || !isUnauthorized(err)) return
    if (router.state.location.pathname === '/login') return
    // Sign out ends the session on purpose; its own navigation goes to plain /login (eng D6).
    if (isSigningOut()) return
    // Several requests fail together; only the first one redirects.
    redirecting = true
    const redirect = router.state.location.href
    client.clear()
    void router.navigate({ to: '/login', search: { redirect, expired: true } }).finally(() => {
      redirecting = false
    })
  }
  // A new cache is a new session: nothing registered may carry over (the page tests build one per test).
  for (const fn of resets) fn()
  const client: QueryClient = new AppQueryClient({
    defaultOptions: {
      queries: {
        staleTime: 10_000,
        // No refetch on focus: pages freeze their window ("no polling"), and a refetch
        // against a frozen window can mix server-side `range` data with stale client
        // bounds. Returning to the tab moves the window instead (useReturnTick).
        refetchOnWindowFocus: false,
        // 5xx/408/429 and deadlines only, at most twice; never another 4xx (a 404 or 403 is an answer).
        retry: opts.retry === false ? false : (failures, err) => failures < 2 && isRetryable(err),
        // A 429/503's Retry-After wins (retrying sooner spends the same budget), capped so a page never stalls.
        retryDelay: (attempt, err) =>
          err instanceof ApiError && err.retryAfterSeconds
            ? Math.min(err.retryAfterSeconds, 30) * 1000
            : Math.min(1000 * 2 ** attempt, 30_000),
      },
      // A write is never repeated behind the user's back.
      mutations: { retry: 0 },
    },
    queryCache: new QueryCache({
      onError: (err, query) => {
        // A first-load /api/me 401 belongs to the _app route guard (no session yet ≠ expired);
        // once `me` has data, its 401 is an expiry like any other.
        if (query.queryKey[0] === 'me' && query.state.data === undefined) return
        expired(err)
      },
    }),
    mutationCache: new MutationCache({ onError: (err) => expired(err) }),
  })
  return client
}
