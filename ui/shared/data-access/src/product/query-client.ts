// Ported from nasiko-ui-lab@1b99bdda common/src/lib/queryClient.ts. Local edits: the 401 reaction is
// injected (`onUnauthorized`) instead of navigating a TanStack Router and consulting `isSigningOut`;
// the `me` exception reads the scoped product key (`ME_FEATURE` at index 4) instead of `['me']`;
// `onCacheReset` returns an unsubscribe; built on `@tanstack/query-core` (no React); Prettier formatting.

import {
  MutationCache,
  QueryCache,
  QueryClient,
  type QueryFilters,
  type QueryKey,
  type ResetOptions,
} from "@tanstack/query-core";
import { ME_FEATURE } from "./auth";
import { isRetryable, ProductApiError } from "./client";
import { isUnauthorized } from "./detect";
import { isProductKey } from "./keys";

const resets = new Set<() => void>();

/**
 * Client state that belongs to the session (the lab's agents lifecycle watches, a zustand store)
 * registers here to end with the cache: every full `clear()` (sign-out, sign-in, a 401 expiry,
 * another tab's sign-out) and every unfiltered `resetQueries()` (a full reset). Shared code can't
 * import a feature, so the feature registers itself when its module loads. Returns the unsubscribe.
 */
export function onCacheReset(fn: () => void): () => void {
  resets.add(fn);
  return () => {
    resets.delete(fn);
  };
}

function runResets(): void {
  for (const fn of resets) fn();
}

class ProductQueryClient extends QueryClient {
  override clear(): void {
    super.clear();
    runResets();
  }
  override resetQueries<K extends QueryKey = QueryKey>(
    filters?: QueryFilters<K>,
    options?: ResetOptions,
  ): Promise<void> {
    if (!filters) runResets();
    return super.resetQueries(filters, options);
  }
}

export interface ProductQueryClientOptions {
  /**
   * Runs once per expiry burst after the cache was cleared: the app sends the user to
   * `/login?expired=true` (returning to the same URL afterwards). Several requests fail together;
   * only the first one reacts, and the next burst can react once this call has settled. It is
   * never called for a first-load `me` 401 (no session yet is not an expiry): that belongs to the
   * route guard.
   */
  readonly onUnauthorized: () => void | Promise<void>;
  /** `false` turns retries off (page tests). */
  readonly retry?: boolean | undefined;
}

/**
 * The product QueryClient. A 401 on any query or mutation means the session ended: clear the cache
 * and hand over to `onUnauthorized`. Built by a factory so page tests use the exact same behaviour.
 */
export function createProductQueryClient(opts: ProductQueryClientOptions): QueryClient {
  let reacting = false;
  const expired = (err: unknown) => {
    if (reacting || !isUnauthorized(err)) return;
    // Several requests fail together; only the first one reacts.
    reacting = true;
    client.clear();
    void Promise.resolve()
      .then(() => opts.onUnauthorized())
      .finally(() => {
        reacting = false;
      });
  };
  // A new cache is a new session: nothing registered may carry over (the page tests build one per test).
  runResets();
  const client: QueryClient = new ProductQueryClient({
    defaultOptions: {
      queries: {
        staleTime: 10_000,
        // No refetch on focus: pages freeze their window ("no polling"), and a refetch
        // against a frozen window can mix server-side `range` data with stale client
        // bounds. Returning to the tab moves the window instead (the lab's useReturnTick).
        refetchOnWindowFocus: false,
        // 5xx/408/429 and deadlines only, at most twice; never another 4xx (a 404 or 403 is an answer).
        retry: opts.retry === false ? false : (failures, err) => failures < 2 && isRetryable(err),
        // A 429/503's Retry-After wins (retrying sooner spends the same budget), capped so a page never stalls.
        retryDelay: (attempt, err) =>
          err instanceof ProductApiError && err.retryAfterSeconds
            ? Math.min(err.retryAfterSeconds, 30) * 1000
            : Math.min(1000 * 2 ** attempt, 30_000),
      },
      // A write is never repeated behind the user's back.
      mutations: { retry: 0 },
    },
    queryCache: new QueryCache({
      onError: (err, query) => {
        // A first-load /api/me 401 belongs to the route guard (no session yet ≠ expired);
        // once `me` has data, its 401 is an expiry like any other.
        const key = query.queryKey;
        if (isProductKey(key) && key[4] === ME_FEATURE && query.state.data === undefined) return;
        expired(err);
      },
    }),
    mutationCache: new MutationCache({
      onError: (err) => {
        expired(err);
      },
    }),
  });
  return client;
}
