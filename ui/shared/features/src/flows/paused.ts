/**
 * Paused flows by chat, for the Overview's Needs you (plans/feat-flows.md F19, O2 as amended 2026-10-06): a request
 * row links its chat's paused flow instead of a second row for the same pause. One read of the caller's paused flows
 * (`GET /api/flows?status=paused`, newest first); a flow without a known chat session can't be matched and is left
 * out. A failed read just means no links: it never blocks the inbox.
 */
import { useQuery } from '@tanstack/react-query'
import { apiFetch, withQuery } from '@nasiko/data-access/client'
import { flowKeys } from './queryKeys'
import { LIST_PAGE } from './tuning'
import { flowListSchema, flowSession, type Flow } from './types'

/** Chat session → its newest paused flow's id. */
function pausedByChat(rows: readonly Flow[]): Map<string, string> {
  const out = new Map<string, string>()
  for (const f of rows) {
    const s = flowSession(f)
    if (s && !out.has(s)) out.set(s, f.flow_id)
  }
  return out
}

const EMPTY = new Map<string, string>()

export function usePausedFlows(enabled: boolean): ReadonlyMap<string, string> {
  const path = withQuery('/api/flows', { status: 'paused', limit: LIST_PAGE })
  return (
    useQuery({
      queryKey: flowKeys.paused,
      queryFn: ({ signal }) => apiFetch(path, { signal, schema: flowListSchema }),
      select: (b) => pausedByChat(b.data),
      enabled,
      // An enhancement to the inbox: one try, no error state.
      retry: false,
      meta: { path },
    }).data ?? EMPTY
  )
}
