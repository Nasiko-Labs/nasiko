import { ApiError } from '@nasiko/data-access/client'

/**
 * Whether an error is nasiko-server saying it has no trace store (`observability_unconfigured`, a 503 whose body names
 * TEMPO_URL and LOKI_URL). Any other 503 — a harness not ready, a runtime with no resource stats, a paused workspace —
 * is not, and must not be worded as a missing TEMPO_URL.
 */
export function isObservabilityUnconfigured(error: unknown): boolean {
  return (
    error instanceof ApiError &&
    error.status === 503 &&
    /not configured/i.test(error.serverMessage ?? '')
  )
}
