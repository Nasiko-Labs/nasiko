/**
 * Which pending request a waiting call is (plans/feat-flows.md F20, eng review O2): a request names its chat
 * (`execution.context_id`, or `chat_session_id`) and its agent, never a flow, so it's the one pending request whose
 * chat and agent both match. Exactly one, or none: two candidates are ambiguous and the page sends the user to chat's
 * Waiting view instead.
 */
import type { HitlDto } from '@nasiko/features/chat/types'

export function matchRequest(
  requests: readonly HitlDto[],
  sessionId: string | null,
  agentId: string | null,
): HitlDto | undefined {
  if (!sessionId || !agentId) return undefined
  const hits = requests.filter(
    (r) =>
      r.status === 'pending' &&
      (r.execution.context_id === sessionId || r.execution.chat_session_id === sessionId) &&
      r.execution.agent_id === agentId,
  )
  return hits.length === 1 ? hits[0] : undefined
}
