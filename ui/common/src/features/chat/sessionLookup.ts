/**
 * Find a chat's list row beyond the rail's pages (v1b §5.1, EN-6, NE-5). The server has no
 * single-session GET (rec P1), so this pages `GET /api/chat/sessions` on from where the rail
 * stopped (`startCursor`), up to LOOKUP_PAGES, stops at the first hit, and caches the row per chat
 * for the registry's lifetime.
 * Hitting the cap never means "removed": it's `capped`, and the page says it couldn't tell.
 */
import { useQuery } from '@tanstack/react-query'
import { apiFetch, withQuery } from '@/lib/api/client'
import { chatKeys } from './keys'
import { unwrapSessionsPage } from './normalize'
import { tuning } from './tuning'
import type { ChatSessionRow } from './types'

export type SessionLookup =
  { status: 'found'; row: ChatSessionRow } | { status: 'absent' } | { status: 'capped' }

const found = new Map<string, ChatSessionRow>()

/** Drop one cached row (the chat was deleted). */
export function forgetSessionLookup(sessionId: string) {
  found.delete(sessionId)
}

/** Drop cached rows (user switch, logout, tests). */
export function clearSessionLookup() {
  found.clear()
}

export async function lookupSession(
  sessionId: string,
  signal?: AbortSignal,
  startCursor?: string,
): Promise<SessionLookup> {
  const hit = found.get(sessionId)
  if (hit) return { status: 'found', row: hit }
  let cursor = startCursor
  for (let i = 0; i < tuning.LOOKUP_PAGES; i++) {
    const page = unwrapSessionsPage(
      await apiFetch(withQuery('/api/chat/sessions', { limit: tuning.LOOKUP_PAGE_SIZE, cursor }), {
        signal,
      }),
    )
    const row = page.data.find((r) => r.session_id === sessionId)
    if (row) {
      found.set(sessionId, row)
      return { status: 'found', row }
    }
    if (!page.has_more || !page.next_cursor) return { status: 'absent' }
    cursor = page.next_cursor
  }
  return { status: 'capped' }
}

/** Runs only when `enabled` (the rail's own pages didn't have the chat). */
export function useSessionLookup(sessionId: string, enabled: boolean, startCursor?: string) {
  return useQuery({
    queryKey: chatKeys.lookup(sessionId),
    enabled,
    staleTime: Infinity,
    queryFn: ({ signal }) => lookupSession(sessionId, signal, startCursor),
  })
}
