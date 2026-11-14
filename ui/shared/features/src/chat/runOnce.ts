/**
 * One chat turn run without the chat page (Home's try-a-run card, overview/components/TryAgentRun.tsx): the same steps
 * a direct chat takes (turnSend.ts), in order. The session is created with the agent, the user row saved, `text`
 * dispatched (`POST /api/orchestrator/a2a`, `message/stream`, send.ts's body) and read with the chat's SSE reducer, then
 * the reply saved with its usage, each row only where the client owns it (`clientWrites`). So the run is a chat like
 * any other: it is listed in Chat and Sessions, and its session page shows what the agent did. The LLM router meters the
 * run as it happens, and the stream's usage frame says what it cost.
 */
import { failureMessage, fetchApi, safeJson } from '@nasiko/data-access/client'
import { readSse } from '@nasiko/data-access/sse'
import { emptyTurn, reduceSseEvent, type TurnState } from './a2aReducer'
import { createSession, saveMessage } from './api'
import { clientWrites, dispatchBody, outcomeOf } from './send'

/** What one finished run cost, from the stream's usage frame (absent when the agent made no metered call). */
export interface RunResult {
  sessionId: string
  tokens: number | null
  costUsd: number | null
}

export class TryRunError extends Error {}

const total = (u: NonNullable<TurnState['usage']>) =>
  u.total_tokens ??
  (u.input_tokens ?? 0) +
    (u.output_tokens ?? 0) +
    (u.cache_read_tokens ?? 0) +
    (u.cache_creation_tokens ?? 0)

async function stream(
  res: Response,
  onWorking: () => void,
  signal: AbortSignal | undefined,
): Promise<TurnState> {
  let state = emptyTurn()
  let working = false
  await readSse(
    res,
    (events) => {
      for (const ev of events) state = reduceSseEvent(state, ev).state
      if (!working && state.taskState) {
        working = true
        onWorking()
      }
    },
    { signal },
  )
  return state
}

/**
 * Runs `text` on `agentId` once as a new chat, calling `onWorking` when the agent picks it up. Resolves with the run's
 * usage; rejects with a TryRunError when a step fails or the agent's task ends in anything but completed.
 */
export async function runAgentOnce(opts: {
  agentId: string
  text: string
  newId: () => string
  onWorking: () => void
  signal?: AbortSignal
}): Promise<RunResult> {
  const sessionId = opts.newId()
  const row = await createSession({
    session_id: sessionId,
    agent_id: opts.agentId,
    first_prompt: opts.text,
  })
  if (clientWrites(row.transcript, 'user'))
    await saveMessage(sessionId, { role: 'user', content: opts.text })
  const res = await fetchApi('/api/orchestrator/a2a', {
    method: 'POST',
    signal: opts.signal,
    headers: { 'Content-Type': 'application/json', Accept: 'text/event-stream' },
    body: JSON.stringify(
      dispatchBody({
        rpcId: opts.newId(),
        messageId: opts.newId(),
        sessionId,
        text: opts.text,
        chatMode: 'direct',
        agentId: opts.agentId,
      }),
    ),
  })
  if (!res.ok) {
    // The server's reason (its JSON-RPC error message), never the raw body (NAS-1624).
    const text = await res.text().catch(() => '')
    throw new TryRunError(
      failureMessage(res.status, text ? safeJson(text) : null, 'POST', '/api/orchestrator/a2a'),
    )
  }
  const state = await stream(res, opts.onWorking, opts.signal)
  const outcome = outcomeOf(state, false)
  if (outcome.kind === 'save' && clientWrites(row.transcript, 'assistant'))
    await saveMessage(sessionId, outcome.body)
  if (state.error) throw new TryRunError(state.error.message)
  if (state.taskState !== 'completed') throw new TryRunError(state.taskState ?? 'no reply')
  const u = state.usage
  return { sessionId, tokens: u ? total(u) : null, costUsd: u?.cost_usd ?? null }
}
