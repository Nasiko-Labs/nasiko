/**
 * One error shape for every Chat failure (plan §6.6, DX-E1). `certainty` drives recovery:
 * - `not-dispatched`: nothing reached the agent (create or user-row save failed).
 * - `rejected-before-run`: the server refused the turn before running it (400/403/404/429/503).
 * - `unknown`: the request was sent; the agent may have run (5xx, network, stream drop).
 * Only `unknown` asks before "Run again" (DS13).
 */
import { ApiError } from '@/lib/api/client'
import { errorCopy, type ErrorKey } from './copy'

export type ChatErrorPhase = 'create' | 'save-user' | 'dispatch' | 'stream' | 'save-assistant'
export type Certainty = 'not-dispatched' | 'rejected-before-run' | 'unknown' | 'definite'

export class ChatError extends Error {
  readonly phase: ChatErrorPhase
  readonly key: ErrorKey
  readonly certainty: Certainty
  readonly status?: number
  readonly rpcCode?: number
  readonly serverDetail?: string
  constructor(init: {
    phase: ChatErrorPhase
    key: ErrorKey
    certainty: Certainty
    status?: number
    rpcCode?: number
    serverDetail?: string
  }) {
    super(init.key)
    this.name = 'ChatError'
    this.phase = init.phase
    this.key = init.key
    this.certainty = init.certainty
    this.status = init.status
    this.rpcCode = init.rpcCode
    this.serverDetail = init.serverDetail
  }
}

/** JSON-RPC error body `{error:{code,message}}`, when the server sent one. */
function rpcError(body: unknown): { code?: number; message?: string } {
  if (body && typeof body === 'object' && 'error' in body) {
    const e = (body as { error: unknown }).error
    if (e && typeof e === 'object')
      return {
        code:
          typeof (e as { code?: unknown }).code === 'number'
            ? (e as { code: number }).code
            : undefined,
        message:
          typeof (e as { message?: unknown }).message === 'string'
            ? (e as { message: string }).message
            : undefined,
      }
    if (typeof e === 'string') return { message: e }
  }
  return {}
}

/** Which request failed: routed chats and reconnects get their own copy (v1b §5.6). */
export interface DispatchContext {
  chatMode: 'direct' | 'routed'
  operation: 'send' | 'rerun' | 'resume'
}

/**
 * Map a failed dispatch response (before any frame) to a ChatError. HTTP status decides; a
 * JSON-RPC body, when it parses, only adds its code and message (R1). A plain-text body (429)
 * is kept as the server detail.
 */
export function dispatchError(
  status: number,
  body: unknown,
  ctx: DispatchContext = { chatMode: 'direct', operation: 'send' },
): ChatError {
  const rpc = rpcError(body)
  const detail = rpc.message ?? (typeof body === 'string' ? body.trim() || undefined : undefined)
  const base = { phase: 'dispatch' as const, status, rpcCode: rpc.code, serverDetail: detail }
  if (ctx.chatMode === 'routed')
    return routedDispatchError(status, base, ctx.operation === 'resume')
  if (status === 400)
    return new ChatError({ ...base, key: 'badRequest', certainty: 'rejected-before-run' })
  if (status === 403)
    return new ChatError({ ...base, key: 'forbidden', certainty: 'rejected-before-run' })
  if (status === 404)
    return new ChatError({ ...base, key: 'agentGone', certainty: 'rejected-before-run' })
  // quirk: §10.11 — 429 has no Retry-After, so the copy says "a minute".
  if (status === 429)
    return new ChatError({ ...base, key: 'rateLimited', certainty: 'rejected-before-run' })
  if (status === 503)
    return new ChatError({ ...base, key: 'noAgents', certainty: 'rejected-before-run' })
  return new ChatError({ ...base, key: 'cutOff', certainty: 'unknown' })
}

function routedDispatchError(
  status: number,
  base: { phase: 'dispatch'; status: number; rpcCode?: number; serverDetail?: string },
  resume: boolean,
): ChatError {
  const refused = (key: ErrorKey) =>
    new ChatError({ ...base, key, certainty: 'rejected-before-run' })
  if (resume) {
    // A reconnect never re-runs anything: the continuation already ran server-side (§2.7).
    if (status === 403) return refused('routedReconnectForbidden')
    if (status === 429) return refused('routedRateLimited')
    return new ChatError({ ...base, key: 'routedMayStillArrive', certainty: 'unknown' })
  }
  if (status === 400) return refused('routedBadRequest')
  if (status === 403) return refused('routedForbidden')
  if (status === 429) return refused('routedRateLimited')
  if (status === 503 || status === 404) return refused('routedNoAgents')
  return new ChatError({ ...base, key: 'routedInternal', certainty: 'unknown' })
}

/** Map an ApiError (or network failure) from a REST call to a ChatError for that phase. */
export function restError(phase: ChatErrorPhase, err: unknown): ChatError {
  const status = err instanceof ApiError ? err.status : undefined
  const detail = err instanceof ApiError ? (err.serverMessage ?? undefined) : undefined
  if (phase === 'create')
    return new ChatError({
      phase,
      key: status === 403 ? 'forbidden' : 'createFailed',
      certainty: 'not-dispatched',
      status,
      serverDetail: detail,
    })
  if (phase === 'save-user')
    return new ChatError({
      phase,
      key: 'saveUserFailed',
      certainty: 'not-dispatched',
      status,
      serverDetail: detail,
    })
  if (phase === 'save-assistant') {
    // 4xx: the server refused the row, so it is definitely not saved. Anything else may have committed.
    const definite = status !== undefined && status >= 400 && status < 500 && status !== 408
    return new ChatError({
      phase,
      key: definite ? 'saveDefinite' : 'saveUnknown',
      certainty: definite ? 'definite' : 'unknown',
      status,
      serverDetail: detail,
    })
  }
  return new ChatError({ phase, key: 'cutOff', certainty: 'unknown', status, serverDetail: detail })
}

/** Whether "Run again" must ask first. */
export const needsRunAgainConfirm = (e: ChatError | null | undefined) =>
  !e || e.certainty === 'unknown'

/** A failure before any turn exists, as one line: the notice under the composer and the announcement. */
export const sendErrorText = (error: unknown): string | null => {
  if (!error) return null
  const c = errorCopy[error instanceof ChatError ? error.key : 'createFailed']
  return `${c.problem} ${c.action}`
}
