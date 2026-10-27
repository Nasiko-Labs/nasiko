/**
 * A finished live turn against history (EN14, §5.4, ship review D2): the chat page's decisions, pure. History is
 * the truth once it has caught up, so a settled turn is forgotten (or, routed, reconciled through `stepRouted`);
 * otherwise a turn paused on a request handled elsewhere, or an "unsaved" reply that actually committed, would
 * lock the composer forever. The page reports what it saw and dispatches the returned action.
 */
import type { DisplayTurn } from './turnModel'
import {
  attemptKeyOf,
  isLivePhase,
  routedSettles,
  type LiveTurn,
  type RoutedEvent,
} from './turnRegistry'
import { tuning } from './tuning'
import type { HitlDto } from './types'

type HistoryResult = Extract<RoutedEvent, { type: 'history' }>['result']

export interface ReconcileInput {
  live: LiveTurn | undefined
  /** When this view saw the turn end; null while it runs (or before the view saw it). */
  endedAt: number | null
  history: { dataUpdatedAt: number; errorUpdatedAt: number; isError: boolean }
  /** History turns with the live overlay (`mergeTurns`). */
  turns: readonly DisplayTurn<LiveTurn>[]
  /** Every request in the loaded history. */
  requests: readonly HitlDto[]
  /** Pending requests in the loaded history. */
  pending: number
  /** An answer or dismissal is in flight: its refetch can land before the resume starts. */
  answering: boolean
  /** Replies the turn's message gained beyond its baseline: its own reply committed. */
  newReplies: number
}

/** The routed re-check budget, per attempt (`attemptKeyOf` plus its start). */
export interface Rechecks {
  attempt: string
  n: number
  /** The history update already looked at. */
  seenAt: number
}
export const NO_RECHECKS: Rechecks = { attempt: '', n: 0, seenAt: 0 }

export type ReconcileAction =
  | { type: 'none' }
  | { type: 'forget' }
  | { type: 'reconcile'; result: HistoryResult }
  /** `no_match` now, then refetch history after `delayMs` (the bounded backoff). */
  | { type: 'recheck'; delayMs: number }

const NONE: ReconcileAction = { type: 'none' }
const FORGET: ReconcileAction = { type: 'forget' }

/** The history turn of the live turn's user message. */
export const ownTurn = (turns: ReconcileInput['turns'], live: LiveTurn | undefined) =>
  live?.userMessageId ? turns.find((t) => t.user?.id === live.userMessageId) : undefined

/** The turn just stopped running: note when, and refresh (a routed turn's registry already invalidated). */
export function turnEnded(
  live: LiveTurn | undefined,
  endedAt: number | null,
  now: number,
): { endedAt: number | null; refresh: 'invalidate' | 'refetch' | null } {
  if (!live || isLivePhase(live.phase)) return { endedAt: null, refresh: null }
  if (endedAt !== null) return { endedAt, refresh: null }
  // A routed turn's queries are invalidated by the registry when it ends (NE-4). That refetch
  // can land before the page sees the end, so always start one that finishes after `endedAt`.
  return { endedAt: now, refresh: live.chatMode === 'routed' ? 'refetch' : 'invalidate' }
}

/** A routed attempt settles on its own reply: its trace id, or a new reply without one (§5.4). */
export const routedMatches = (i: Pick<ReconcileInput, 'live' | 'turns' | 'newReplies'>) =>
  !!i.live &&
  i.live.chatMode === 'routed' &&
  routedSettles(i.live, ownTurn(i.turns, i.live)?.replies ?? [], i.newReplies)

/**
 * A routed turn after its end (§5.4). The server saves the reply at the end of its stream, possibly just after
 * the client saw EOF: history is re-checked a few times with backoff before the reply is called unconfirmed.
 */
export function reconcileRouted(
  i: ReconcileInput,
  rechecks: Rechecks,
): { action: ReconcileAction; rechecks: Rechecks } {
  const { live, endedAt, history } = i
  const keep = (action: ReconcileAction) => ({ action, rechecks })
  if (!live || live.chatMode !== 'routed' || isLivePhase(live.phase) || endedAt === null)
    return keep(NONE)
  // A routed pause answered elsewhere (another tab, expiry): history is the truth, as v1a.
  if (live.phase === 'paused')
    return keep(history.dataUpdatedAt > endedAt && i.pending === 0 && !i.answering ? FORGET : NONE)
  // A pause the drain couldn't decode (§5.3): history's pending request on this turn is the truth.
  // A resume's turn can hold a request that was pending all along: only one raised after its
  // answer (a chained pause the client never decoded) counts.
  const answeredAt =
    live.operation === 'resume'
      ? Date.parse(i.requests.find((q) => q.id === live.resumedRequestId)?.resolved_at ?? '')
      : -Infinity
  const raised = (q: HitlDto) =>
    q.status === 'pending' &&
    q.id !== live.resumedRequestId &&
    Date.parse(q.created_at) >= answeredAt
  if (history.dataUpdatedAt > endedAt && ownTurn(i.turns, live)?.requests.some(raised))
    return keep(FORGET)
  if (history.isError && history.errorUpdatedAt > endedAt)
    return keep({ type: 'reconcile', result: 'error' })
  // Per attempt: a rerun keeps the turn id but gets its own re-check budget.
  const attempt = `${attemptKeyOf(live)}:${live.startedAt}`
  if (
    history.dataUpdatedAt <= endedAt ||
    (rechecks.attempt === attempt && rechecks.seenAt === history.dataUpdatedAt)
  )
    return keep(NONE)
  const seen: Rechecks = {
    ...(rechecks.attempt === attempt ? rechecks : { attempt, n: 0 }),
    seenAt: history.dataUpdatedAt,
  }
  const to = (action: ReconcileAction, next = seen) => ({ action, rechecks: next })
  if (routedMatches(i)) return to({ type: 'reconcile', result: 'match' })
  const awaitingSave =
    live.phase === 'done' || live.phase === 'loading_saved' || live.phase === 'history_failed'
  if (!awaitingSave) return to({ type: 'reconcile', result: 'no_match' })
  if (seen.n >= tuning.ROUTED_RECHECKS) return to({ type: 'reconcile', result: 'timeout' })
  return to(
    { type: 'recheck', delayMs: tuning.ROUTED_RECHECK_MS * 2 ** seen.n },
    { ...seen, n: seen.n + 1 },
  )
}

/**
 * A direct turn after its end: forget it once history has caught up and holds the outcome. A routed dispatch
 * refused before it ran keeps its saved user row (the notice's Retry re-sends it), so this never touches routed.
 */
export function reconcileDirect(i: ReconcileInput): ReconcileAction {
  const { live, endedAt, history } = i
  if (!live || live.chatMode === 'routed' || endedAt === null || history.dataUpdatedAt <= endedAt)
    return NONE
  const settled =
    live.phase === 'done' ||
    // Not while an answer or dismissal is in flight: its refetch can land before the resume starts.
    (live.phase === 'paused' && i.pending === 0 && !i.answering) ||
    ((live.phase === 'unsaved' || live.phase === 'error' || live.phase === 'no_reply') &&
      i.newReplies > 0)
  return settled ? FORGET : NONE
}
