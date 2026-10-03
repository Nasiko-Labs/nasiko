/**
 * What an API error means, decided in one place so each rule can be tested against the live server's recorded
 * responses (src/test/liveErrors.test.ts, plans/feat-live-contract.md §7.3).
 */
import { ApiError } from './client'

/** A coded API_CONVENTIONS error body (`{error, code}`), if the error carries one. */
export function errorCode(err: unknown): string | null {
  if (!(err instanceof ApiError) || !err.body || typeof err.body !== 'object') return null
  const code = (err.body as { code?: unknown }).code
  return typeof code === 'string' ? code : null
}

/**
 * The server has no such route: a 404 that is empty or the router's envelope (nasiko-server at ea233d20 answers an
 * unknown path with `{data: null, message: "no API route matches …", status_code: 404}`), never coded. A plain-text
 * 404 naming a thing is a handler's own answer ("agent not found", "budget not found"), so the endpoint exists; the
 * bare reason phrase "Not Found" (a proxy, or an older server's fallback) doesn't say, and counts as absent.
 */
export function isEndpointAbsent(err: unknown): boolean {
  if (!(err instanceof ApiError) || err.status !== 404 || errorCode(err) !== null) return false
  if (typeof err.body !== 'string') return true
  const text = err.body.trim().toLowerCase()
  return text === '' || text === 'not found'
}

/** TokenOps F4: the proposed `/finops/top-traces` is missing (the drawer says "needs a newer nasiko-server"). */
export const isTopTracesAbsent = (err: unknown) => isEndpointAbsent(err)

/** Router R2: the proposed `/api/budgets` is missing (the section says "Budgets need a newer OpenRuntime server"). */
export const isBudgetsAbsent = (err: unknown) => isEndpointAbsent(err)

/**
 * nasiko-server 401 reasons that mean the token is already dead (auth/middleware.rs @ cb3aaf0c, sent as the
 * AuthRejection envelope's `message`). "token validation unavailable" is NOT one: the database was unreachable,
 * nothing was revoked, and the cookie will work again once it's back, so that (and any unknown 401) is a failed
 * logout. EE caveat: EE maps a database error in `validate_token` to "invalid token"
 * (docs/designs/openruntime-app-shell-recommendations.md §1).
 */
const DEAD_SESSION_401 = new Set([
  'not authenticated',
  'missing or invalid token',
  'invalid token',
  'token missing jti',
  'token revoked',
  'session user no longer exists',
])

/** A 401 whose reason says the session is already over (sign out then counts as done). */
export function isDeadSession401(err: unknown): boolean {
  return (
    err instanceof ApiError && err.status === 401 && DEAD_SESSION_401.has(err.serverMessage ?? '')
  )
}

/** Any 401: the shared query/mutation handler (src/lib/queryClient.ts) treats it as an expired session. */
export function isUnauthorized(err: unknown): boolean {
  return err instanceof ApiError && err.status === 401
}
