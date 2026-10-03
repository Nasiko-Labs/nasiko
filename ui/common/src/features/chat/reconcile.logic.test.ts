/** The chat page's reconcile decisions (EN14, §5.4), pure. Page behaviour is in the ChatPage tests. */
import { describe, expect, it } from 'vitest'
import { emptyTurn } from './a2aReducer'
import {
  NO_RECHECKS,
  reconcileDirect,
  reconcileRouted,
  routedMatches,
  turnEnded,
  type ReconcileInput,
} from './reconcile'
import { tuning } from './tuning'
import type { LiveTurn } from './turnRegistry'
import type { ChatMessage, HitlDto } from './types'

const live = (over: Partial<LiveTurn> = {}): LiveTurn => ({
  id: 't1',
  sessionId: 's',
  agentId: null,
  userText: 'hi',
  userMessageId: 'u1',
  phase: 'done',
  state: emptyTurn(),
  startedAt: 100,
  idle: false,
  stopped: false,
  finalized: true,
  error: null,
  saved: null,
  pendingSave: null,
  frames: [],
  chatMode: 'routed',
  operation: 'send',
  attempt: 'send',
  ...over,
})
const user = { id: 'u1', role: 'user', content: 'hi', timestamp: '' } as ChatMessage
const reply = (over: Partial<ChatMessage> = {}) =>
  ({ id: 'r1', role: 'assistant', content: 'ok', timestamp: '', ...over }) as ChatMessage
const request = (over: Partial<HitlDto>) =>
  ({ id: 'q1', status: 'pending', created_at: '2026-09-30T10:00:00Z', ...over }) as HitlDto

const input = (over: Partial<ReconcileInput> = {}): ReconcileInput => ({
  live: live(),
  endedAt: 1_000,
  history: { dataUpdatedAt: 2_000, errorUpdatedAt: 0, isError: false },
  turns: [{ key: 'u1', user, replies: [], requests: [] }],
  requests: [],
  pending: 0,
  answering: false,
  newReplies: 0,
  ...over,
})

describe('turnEnded', () => {
  it('clears while the turn runs, stamps the end once and refreshes by chat mode', () => {
    expect(turnEnded(live({ phase: 'streaming' }), 5, 9)).toEqual({ endedAt: null, refresh: null })
    expect(turnEnded(undefined, 5, 9)).toEqual({ endedAt: null, refresh: null })
    expect(turnEnded(live(), null, 9)).toEqual({ endedAt: 9, refresh: 'refetch' })
    expect(turnEnded(live({ chatMode: 'direct' }), null, 9)).toEqual({
      endedAt: 9,
      refresh: 'invalidate',
    })
    expect(turnEnded(live(), 5, 9)).toEqual({ endedAt: 5, refresh: null })
  })
})

describe('reconcileRouted', () => {
  it('does nothing for a direct, running or unseen end', () => {
    for (const i of [
      input({ live: live({ chatMode: 'direct' }) }),
      input({ live: live({ phase: 'streaming' }) }),
      input({ endedAt: null }),
      input({ live: undefined }),
    ])
      expect(reconcileRouted(i, NO_RECHECKS).action).toEqual({ type: 'none' })
  })

  it('forgets a pause once fresh history shows it handled, never while an answer is in flight', () => {
    const paused = live({ phase: 'paused' })
    expect(reconcileRouted(input({ live: paused }), NO_RECHECKS).action.type).toBe('forget')
    for (const i of [
      input({ live: paused, answering: true }),
      input({ live: paused, pending: 1 }),
      input({ live: paused, history: { dataUpdatedAt: 1_000, errorUpdatedAt: 0, isError: false } }),
    ])
      expect(reconcileRouted(i, NO_RECHECKS).action.type).toBe('none')
  })

  it('forgets a turn whose history holds a request raised after its answer, not the one it resumed', () => {
    const resumed = live({ operation: 'resume', resumedRequestId: 'q0' })
    const answered = request({ id: 'q0', status: 'resolved', resolved_at: '2026-09-30T10:00:00Z' })
    const turns = (requests: HitlDto[]) => [{ key: 'u1', user, replies: [], requests }]
    const chained = request({ id: 'q2', created_at: '2026-09-30T10:01:00Z' })
    expect(
      reconcileRouted(
        input({ live: resumed, requests: [answered, chained], turns: turns([chained]) }),
        NO_RECHECKS,
      ).action.type,
    ).toBe('forget')
    const older = request({ id: 'q3', created_at: '2026-09-30T09:00:00Z' })
    expect(
      reconcileRouted(
        input({ live: resumed, requests: [answered, older], turns: turns([older]) }),
        NO_RECHECKS,
      ).action,
    ).not.toEqual({ type: 'forget' })
  })

  it('reports a history error after the end', () => {
    const i = input({ history: { dataUpdatedAt: 500, errorUpdatedAt: 3_000, isError: true } })
    expect(reconcileRouted(i, NO_RECHECKS).action).toEqual({ type: 'reconcile', result: 'error' })
  })

  it('settles on its own reply by trace id', () => {
    const own = live({ state: { ...emptyTurn(), traceId: 'tr1' } })
    const i = input({
      live: own,
      turns: [{ key: 'u1', user, replies: [reply({ trace_id: 'tr1' })], requests: [] }],
    })
    expect(routedMatches(i)).toBe(true)
    expect(reconcileRouted(i, NO_RECHECKS).action).toEqual({ type: 'reconcile', result: 'match' })
    expect(routedMatches(input({ live: own }))).toBe(false)
  })

  it('re-checks a complete reply with doubling backoff, then gives up; each history update counts once', () => {
    let r = NO_RECHECKS
    const delays: number[] = []
    for (let n = 0; n < tuning.ROUTED_RECHECKS; n++) {
      const i = input({ history: { dataUpdatedAt: 2_000 + n, errorUpdatedAt: 0, isError: false } })
      const out = reconcileRouted(i, r)
      expect(out.action.type).toBe('recheck')
      if (out.action.type === 'recheck') delays.push(out.action.delayMs)
      // The same history update again is not a new look.
      expect(reconcileRouted(i, out.rechecks).action.type).toBe('none')
      r = out.rechecks
    }
    expect(delays).toEqual(delays.map((_, n) => tuning.ROUTED_RECHECK_MS * 2 ** n))
    const last = input({ history: { dataUpdatedAt: 9_000, errorUpdatedAt: 0, isError: false } })
    expect(reconcileRouted(last, r).action).toEqual({ type: 'reconcile', result: 'timeout' })
    // A rerun (a new start) gets its own budget.
    const rerun = input({ ...last, live: live({ startedAt: 200, attempt: 'rerun:1' }) })
    expect(reconcileRouted(rerun, r).action.type).toBe('recheck')
  })

  it('reports no match once, without re-checks, for an end that is not waiting on a save', () => {
    const i = input({ live: live({ phase: 'lost' }) })
    expect(reconcileRouted(i, NO_RECHECKS).action).toEqual({
      type: 'reconcile',
      result: 'no_match',
    })
  })
})

describe('reconcileDirect', () => {
  const direct = (over: Partial<LiveTurn>) => live({ chatMode: 'direct', ...over })
  it('forgets a finished direct turn once history has caught up', () => {
    expect(reconcileDirect(input({ live: direct({ phase: 'done' }) })).type).toBe('forget')
    expect(
      reconcileDirect(
        input({
          live: direct({ phase: 'done' }),
          history: { dataUpdatedAt: 1_000, errorUpdatedAt: 0, isError: false },
        }),
      ).type,
    ).toBe('none')
    expect(reconcileDirect(input({ live: direct({ phase: 'done' }), endedAt: null })).type).toBe(
      'none',
    )
    expect(reconcileDirect(input()).type).toBe('none')
  })

  it('a pause waits for its answer to settle; an unsaved or failed reply waits for a new reply', () => {
    const paused = direct({ phase: 'paused' })
    expect(reconcileDirect(input({ live: paused })).type).toBe('forget')
    expect(reconcileDirect(input({ live: paused, answering: true })).type).toBe('none')
    expect(reconcileDirect(input({ live: paused, pending: 1 })).type).toBe('none')
    for (const phase of ['unsaved', 'error', 'no_reply'] as const) {
      expect(reconcileDirect(input({ live: direct({ phase }) })).type).toBe('none')
      expect(reconcileDirect(input({ live: direct({ phase }), newReplies: 1 })).type).toBe('forget')
    }
  })
})
