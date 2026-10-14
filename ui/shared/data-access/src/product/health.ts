// Ported from nasiko-ui-lab@1b99bdda common/src/lib/api/health.ts. Local edits: the `useHealth` hook
// became `fetchHealth(signal)` plus `healthQueryOptions(scope, opts)`, a plain options object under a
// scoped product key (React stays out of this module; a page wraps it in `useQuery`); Prettier formatting.

import { apiFetch } from "./client";
import { productKey, type ProductKey } from "./keys";
import type { Scope } from "../canonical";

/** The Status page checks every 15 s; the sidebar's status row, mounted on every page, every 60 s (review). */
export const HEALTH_INTERVAL_MS = 15_000;
export const SIDEBAR_HEALTH_INTERVAL_MS = 60_000;

export const HEALTH_FEATURE = "health";

/** GET /health — public, unauthenticated. Proves the rewrite (or the ingress) reaches the server. */
export function fetchHealth(o: { signal?: AbortSignal | undefined } = {}): Promise<unknown> {
  return apiFetch<unknown>("/health", o.signal === undefined ? {} : { signal: o.signal });
}

export interface HealthQueryOptions {
  refetchInterval?: number | undefined;
  refetchOnWindowFocus?: boolean | undefined;
}

/**
 * Each observer keeps its own timer, so while the Status page is open both it and the sidebar poll
 * (about 5 requests a minute). Focus refetch follows the client default (off) unless a caller sets it.
 */
export function healthQueryOptions(
  scope: Scope,
  opts: HealthQueryOptions = {},
): {
  readonly queryKey: ProductKey;
  readonly queryFn: (ctx: { signal: AbortSignal }) => Promise<unknown>;
  /** It is the probe: the next poll is the retry. */
  readonly retry: false;
  readonly refetchInterval: number;
  readonly refetchOnWindowFocus?: boolean;
} {
  return {
    queryKey: productKey(scope, HEALTH_FEATURE),
    queryFn: ({ signal }) => fetchHealth({ signal }),
    retry: false,
    refetchInterval: opts.refetchInterval ?? HEALTH_INTERVAL_MS,
    ...(opts.refetchOnWindowFocus === undefined
      ? {}
      : { refetchOnWindowFocus: opts.refetchOnWindowFocus }),
  };
}
