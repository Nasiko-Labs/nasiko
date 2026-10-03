import { useQuery } from '@tanstack/react-query'
import { apiFetch } from './client'

/** The Status page checks every 15 s; the sidebar's status row, mounted on every page, every 60 s (review). */
export const HEALTH_INTERVAL_MS = 15_000
export const SIDEBAR_HEALTH_INTERVAL_MS = 60_000

const healthKeys = { all: ['health'] as const }

/**
 * GET /health — public, unauthenticated. Proves the proxy reaches the server. Each observer keeps its
 * own timer, so while the Status page is open both it and the sidebar poll (about 5 requests a minute).
 * Focus refetch follows the client default (off) unless a caller sets it.
 */
export function useHealth(opts: { refetchInterval?: number; refetchOnWindowFocus?: boolean } = {}) {
  return useQuery({
    queryKey: healthKeys.all,
    queryFn: () => apiFetch<unknown>('/health'),
    // It is the probe: the next poll is the retry.
    retry: false,
    refetchInterval: opts.refetchInterval ?? HEALTH_INTERVAL_MS,
    ...(opts.refetchOnWindowFocus === undefined
      ? {}
      : { refetchOnWindowFocus: opts.refetchOnWindowFocus }),
  })
}
