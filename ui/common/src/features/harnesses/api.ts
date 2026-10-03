/**
 * Harnesses queries. Every key starts with ['harnesses', persona] so a mock persona switch
 * can never show the previous persona's data (E4). In live mode the persona is undefined.
 */
import { keepPreviousData, useInfiniteQuery, useQuery } from '@tanstack/react-query'
import { ApiError, apiData, apiFetch, withQuery } from '@/lib/api/client'
import { errorCode, isEndpointAbsent } from '@/lib/api/detect'
import { USER_PAGE_LIMIT } from './constants'
import type { UsageError, UsageResponse, UserMe } from './types'

const USAGE = '/api/observability/coding-agents/usage'

export interface UsageRequest {
  scope?: string
  unit_id?: string
  user_id?: string
  group_by?: 'unit' | 'user'
  range?: string
  start_time?: string
  end_time?: string
  compare?: boolean
  cursor?: string
}

export const harnessKeys = {
  all: (persona?: string) => ['harnesses', persona ?? 'live'] as const,
  me: (persona?: string) => [...harnessKeys.all(persona), 'users-me'] as const,
  /** `windowKey` is the resolved window: a rolling range ({range:'30d'}) sends the same params
   *  after "now" moves, so the key must still change for the data to follow the dates. */
  usage: (persona: string | undefined, r: UsageRequest, windowKey: string) =>
    [...harnessKeys.all(persona), 'usage', windowKey, r] as const,
  /** Live fallback (the viewer's own usage from existing endpoints), per signed-in user. */
  live: (userId: string | undefined, ...rest: readonly unknown[]) =>
    ['harnesses', 'live', userId, ...rest] as const,
}

export function usagePath(r: UsageRequest): string {
  // withQuery drops undefined/empty values; only compare and the page limit are computed.
  return withQuery(USAGE, {
    ...r,
    compare: r.compare ? 1 : undefined,
    limit: r.group_by === 'user' ? USER_PAGE_LIMIT : undefined,
  })
}

/** A coded API_CONVENTIONS error body ({error, code}), if the error carries one. */
export function usageErrorCode(err: unknown): UsageError['code'] | null {
  return errorCode(err) as UsageError['code'] | null
}

/** A 404 WITHOUT the coded body: the endpoint is absent (plan §4, X4, N22; the rule is in lib/api/detect.ts). */
export { isEndpointAbsent }

/** A coded 404: the unit/user is not found or not visible. */
export function isNotVisible(err: unknown): boolean {
  const code = usageErrorCode(err)
  return (
    err instanceof ApiError &&
    err.status === 404 &&
    (code === 'unit_not_visible' || code === 'user_not_visible' || code === 'not_found')
  )
}

/** OSS gates /api/users/me behind require_user_manager (oss/server/src/auth/rbac.rs at ea233d20): a non-admin OSS
 *  login gets 403 "requires admin role". Fall back to the token claims from /api/me. */
async function fetchUsersMe(signal: AbortSignal): Promise<UserMe> {
  try {
    return await apiFetch<UserMe>('/api/users/me', { signal })
  } catch (err) {
    if (!(err instanceof ApiError) || err.status !== 403) throw err
    const c = await apiFetch<{ sub: string; username?: string; is_superuser?: boolean }>(
      '/api/me',
      { signal },
    )
    return {
      id: c.sub,
      username: c.username ?? '',
      email: null,
      display_name: null,
      is_superuser: !!c.is_superuser,
      is_active: true,
      role: 'member',
      created_at: '',
      last_login: null,
    }
  }
}

export function useUsersMe(persona: string | undefined) {
  return useQuery({
    queryKey: harnessKeys.me(persona),
    queryFn: ({ signal }) => fetchUsersMe(signal),
    staleTime: 60_000,
  })
}

/** One level's usage, paged by `next_cursor` ("Load more", group_by=user). The key is the level's
 *  request plus the window, so a new level never pairs an old cursor with a new request. A
 *  window move (a rolling range on return) starts a new query, so it shows page one again. */
export function useUsage(
  persona: string | undefined,
  r: UsageRequest,
  windowKey: string,
  enabled: boolean,
) {
  const queryKey = harnessKeys.usage(persona, r, windowKey)
  return useInfiniteQuery({
    queryKey,
    queryFn: ({ signal, pageParam }) =>
      apiData<UsageResponse>(usagePath({ ...r, cursor: pageParam }), { signal }),
    initialPageParam: undefined as string | undefined,
    // A cursor the server hands back twice would loop: stop instead.
    getNextPageParam: (last, _pages, _param, params) =>
      last.next_cursor && !params.includes(last.next_cursor) ? last.next_cursor : undefined,
    // Keep the previous level's numbers (dimmed) while the next loads, but never another
    // persona's: a persona switch must not show the last viewer's data (E4).
    placeholderData: (prev, prevQuery) =>
      prevQuery?.queryKey[1] === queryKey[1] ? keepPreviousData(prev) : undefined,
    enabled,
    meta: { path: usagePath(r) },
  })
}
