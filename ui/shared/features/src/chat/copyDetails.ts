/**
 * "Copy details" on a routed notice (plan §5.6, NE-6): phase, chat id, trace id, HTTP status, RPC code, the
 * server's reason (the one line the notice already shows; NAS-1624), the doc anchor and a pointer to
 * `?debug=turn`. Never headers or raw bodies.
 */
import type { ErrorKey } from './copy'
import type { ChatError } from './errors'

/** The `docs/chat.md#errors-<code>` anchor for each routed notice. */
const ANCHOR: Partial<Record<ErrorKey, string>> = {
  createFailed: 'create',
  saveUserFailed: 'create',
  routedBadRequest: '400',
  routedForbidden: '403',
  routedReconnectForbidden: '403-reconnect',
  routedMayStillArrive: 'reconnect',
  routedRateLimited: '429',
  routedInternal: '500',
  cutOff: '500',
  serverError: '500',
  refusedWithReason: '503',
  routedNoAgents: '503',
  routedFailed: 'failed',
  tooManyLive: 'cap',
  routedAnsweredNoReply: 'answered',
  routedAgentLostTask: 'answered',
}

export const docAnchor = (key: ErrorKey) => `docs/chat.md#errors-${ANCHOR[key] ?? 'failed'}`

export function copyDetails(
  error: Pick<ChatError, 'phase' | 'key' | 'status' | 'rpcCode' | 'serverDetail'>,
  ids: { sessionId?: string; traceId?: string | null },
): string {
  return [
    `phase: ${error.phase}`,
    `chat: ${ids.sessionId ?? 'none'}`,
    `trace: ${ids.traceId ?? 'none'}`,
    `http: ${error.status ?? 'none'}`,
    `rpc: ${error.rpcCode ?? 'none'}`,
    `server: ${error.serverDetail ?? 'none'}`,
    `docs: ${docAnchor(error.key)}`,
    'more: reopen the chat with ?debug=turn',
  ].join('\n')
}
