import { queryOptions } from '@tanstack/react-query'
import { apiFetch } from './client'

/** The caller's claims from GET /api/me (nasiko-server `Claims`). App-wide, not feature-specific. */
export interface Me {
  sub: string
  username: string
  is_superuser: boolean
}

const authKeys = { me: ['me'] as const }

/** GET /api/me — returns the caller's claims ({sub, username, is_superuser}); 401 when not logged in. */
export const meQuery = queryOptions({
  queryKey: authKeys.me,
  queryFn: ({ signal }) => apiFetch<Me>('/api/me', { signal }),
  // The _app guard awaits it on every entry: a down server must show its page now, not after retries.
  retry: false,
  staleTime: 60_000,
})

export interface LoginResult {
  token: string
  user_id: string
  username: string
  is_superuser: boolean
  expires_in: number
}

/** POST /api/auth/login — sets the HttpOnly access_token cookie (same-origin via the proxy). */
export function login(username: string, password: string, timeout?: number): Promise<LoginResult> {
  return apiFetch<LoginResult>('/api/auth/login', {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({ username, password }),
    timeout,
  })
}

/**
 * Only same-app relative paths may be used as a post-login redirect target.
 * Rejects absolute URLs, protocol-relative `//host`, backslash tricks and any
 * /login variant (which would loop).
 */
export function safeRedirect(target: unknown, fallback = '/'): string {
  if (typeof target !== 'string') return fallback
  if (!target.startsWith('/') || target.startsWith('//') || target.includes('\\')) return fallback
  if (/^\/login(?=$|[/?#])/.test(target)) return fallback
  return target
}

/** POST /api/auth/logout — clears the login cookie (nasiko-server `auth/login.rs`). */
export function logout(timeout?: number): Promise<unknown> {
  return apiFetch<unknown>('/api/auth/logout', { method: 'POST', timeout })
}

/**
 * Minutes until a locked account can sign in again, or null when a 429 is the plain rate limit. nasiko-server
 * (auth/service.rs, ea233d20) locks an account for 15 minutes after 3 failed sign-ins: 429 `account_locked`.
 */
export function lockedFor(body: unknown): number | null {
  if (!body || typeof body !== 'object' || (body as { code?: unknown }).code !== 'account_locked')
    return null
  const secs = Number((body as { retry_after_secs?: unknown }).retry_after_secs)
  return Number.isFinite(secs) && secs > 0 ? Math.max(1, Math.ceil(secs / 60)) : 15
}
