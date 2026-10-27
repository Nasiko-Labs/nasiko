/** Where each "Answer the request" on a flow page goes (F20, O2), from one pending-requests read for the page. */
import { useQuery } from '@tanstack/react-query'
import { usePendingRequests } from '@nasiko/features/chat/api'
import { meQuery } from '@nasiko/data-access/auth'
import { matchRequest } from './answer'
import type { Call } from './calls'

/** Where a call's "Answer the request" goes: the matched request in its chat, else the Waiting view. */
export interface AnswerTarget {
  sessionId: string | null
  requestId: string | null
  userId: string | null
}

/** One pending-requests read for the page; `active` while any call waits on a human. */
export function useAnswerTargets(
  sessionId: string | null,
  active: boolean,
): (call: Pick<Call, 'agentId'>) => AnswerTarget {
  const me = useQuery(meQuery).data
  const pending = usePendingRequests(me?.sub ?? '', active && !!me)
  const items = pending.data?.items ?? []
  return (call) => ({
    sessionId,
    requestId: me ? (matchRequest(items, sessionId, call.agentId)?.id ?? null) : null,
    userId: me?.sub ?? null,
  })
}
