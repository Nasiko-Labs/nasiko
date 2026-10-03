/**
 * Pure pieces of one chat turn (plan §6.1; v1b §5.2). The registry does the IO; these decide.
 *
 *   direct: dispatch body ─► stream ─► TurnState ─► outcomeOf ─► save once (§3.3) | pause | note
 *   routed: dispatch body ─► stream ─► TurnState ─► stepRouted ─► the server saves at Done; never the client
 */
import { replyText, type TurnState } from './a2aReducer'
import { RECEIVING_STOPPED_MARKER } from './normalize'
import type { SaveMessageBody, TranscriptOwnership } from './types'

export type ChatTargetMode = { chatMode: 'direct'; agentId: string } | { chatMode: 'routed' }

/**
 * JSON-RPC `message/stream`; contextId must equal the session id on both paths (plan §6.1.5,
 * v1b §2.2). A routed dispatch sends no `agent_id`, so the server runs the orchestrator.
 */
export function dispatchBody(
  opts: { rpcId: string; messageId: string; sessionId: string; text: string } & ChatTargetMode,
) {
  return {
    jsonrpc: '2.0',
    id: opts.rpcId,
    method: 'message/stream',
    params: {
      message: {
        messageId: opts.messageId,
        contextId: opts.sessionId,
        role: 'ROLE_USER',
        parts: [{ text: opts.text }],
      },
      metadata:
        opts.chatMode === 'direct'
          ? { agent_id: opts.agentId, session_id: opts.sessionId }
          : { session_id: opts.sessionId },
    },
  }
}

/** Reconnect after a request is answered (a2a_dispatch.rs:147-161): no text, no agent, no session metadata. */
export function resumeBody(opts: {
  rpcId: string
  messageId: string
  sessionId: string
  requestId: string
}) {
  return {
    jsonrpc: '2.0',
    id: opts.rpcId,
    method: 'message/stream',
    params: {
      message: {
        messageId: opts.messageId,
        contextId: opts.sessionId,
        role: 'ROLE_USER',
        parts: [],
      },
      metadata: { reconnect_after_hitl_id: opts.requestId },
    },
  }
}

/** The usage subset the server accepts on an assistant row (`MessageUsage`). */
function usageBody(s: TurnState): SaveMessageBody['usage'] | undefined {
  const u = s.usage
  const traceId = s.traceId ?? u?.trace_id
  if (!u && !traceId) return undefined
  const out: NonNullable<SaveMessageBody['usage']> = {}
  if (u?.input_tokens !== undefined) out.input_tokens = u.input_tokens
  if (u?.output_tokens !== undefined) out.output_tokens = u.output_tokens
  if (u?.model) out.model = u.model
  if (u?.duration_ms !== undefined) out.duration_ms = Math.round(u.duration_ms)
  if (u?.cost_usd !== undefined) out.cost_usd = u.cost_usd
  if (u?.estimated !== undefined) out.estimated = u.estimated
  if (traceId) out.trace_id = traceId
  return out
}

/** Which side writes each row. Absent (every server at cb3aaf0c): the client writes both on a direct chat. */
// quirk: §10.2 — absent `transcript` (every server at cb3aaf0c) means the §3.3 rules apply.
export const clientWrites = (t: TranscriptOwnership | undefined, role: 'user' | 'assistant') =>
  t ? t[role] !== 'server' : true

export type Outcome =
  | { kind: 'paused' }
  | { kind: 'save'; body: SaveMessageBody }
  | { kind: 'agent-failed' }
  | { kind: 'no-reply' }

/**
 * What to do when the stream has ended (EOF) or the user stopped receiving. Called once per turn.
 * A pause saves nothing, stopped or not: the server saves the resumed reply (§3.3), so a saved
 * partial would be a second assistant row for the same turn.
 */
export function outcomeOf(s: TurnState, stopped: boolean): Outcome {
  if (s.request) return { kind: 'paused' }
  const text = replyText(s)
  if (stopped)
    return text.trim()
      ? { kind: 'save', body: { role: 'assistant', content: text + RECEIVING_STOPPED_MARKER } }
      : { kind: 'no-reply' }
  if (s.error && !text.trim()) return { kind: 'agent-failed' }
  if (!text.trim()) return { kind: 'no-reply' }
  return { kind: 'save', body: { role: 'assistant', content: text, usage: usageBody(s) } }
}
