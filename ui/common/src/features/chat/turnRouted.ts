/**
 * Routed turns (v1b §5.3-§5.4): the stream, drain and cut rules, and `routedStep`, which applies the pure
 * per-attempt table (`stepRouted`) and records each attempt's end once. The server saves a routed reply, so
 * nothing here saves a message; history settles the turn (`reconcile`).
 */
import { emptyTurn, reduceSseEvent, routedReplyText } from './a2aReducer'
import { readSse, SseOverflowError } from '@/lib/sse'
import { ChatError, dispatchError } from './errors'
import { dispatchBody, resumeBody } from './send'
import { tuning } from './tuning'
import type { Core } from './turnCore'
import {
  END_KIND,
  UNCERTAIN,
  attemptKeyOf,
  isLivePhase,
  sawTerminal,
  stepRouted,
  type LiveTurn,
  type Phase,
  type RoutedEvent,
  type TurnEnd,
} from './turnMachine'

export function createRouted(core: Core) {
  const { deps, turns, controllers, idleTimers, unknown, update, notify } = core
  const { isCurrent, armIdle, pushFrame, keepSteps, emitEnd } = core

  function recordEnd(t: LiveTurn, phase: Phase) {
    const kind = END_KIND[phase]
    if (!kind) return
    const end: TurnEnd = {
      id: t.id,
      sessionId: t.sessionId,
      attemptKey: attemptKeyOf(t),
      userMessageId: t.userMessageId,
      traceId: t.state.traceId,
      kind,
      terminal: sawTerminal(t.state, t.operation === 'resume'),
      finishedAt: deps.now(),
      chatMode: 'routed',
    }
    const next = new Map(core.ends)
    next.delete(end.attemptKey)
    next.set(end.attemptKey, end)
    if (next.size > tuning.TURN_ENDS_MAX) {
      const oldest = next.keys().next().value
      if (oldest !== undefined) next.delete(oldest)
    }
    core.ends = next
    notify()
    deps.onTurnEnd?.(end)
    emitEnd(end)
  }

  /** Keep a routed reply's steps for its Activity after the forget, by trace id (R7). */
  function keepRoutedSteps(t: LiveTurn) {
    if (!t.state.traceId || !t.state.steps.length) return
    keepSteps(`trace:${t.state.traceId}`, t.state.steps)
  }

  /** Apply one routed event; a terminal phase is recorded once. Never saves a message. */
  function routedStep(sessionId: string, ev: RoutedEvent, patch: Partial<LiveTurn> = {}) {
    const t = turns.get(sessionId)
    if (!t || t.phase === 'aborted') return
    const next = stepRouted(t.phase, ev)
    if (next === 'settled') {
      keepRoutedSteps(t)
      // The reply is saved: nothing is uncertain any more.
      unknown.delete(sessionId)
      // A reply that arrived after an empty ending clears that ending (EN-5).
      if (core.ends.get(attemptKeyOf(t))?.kind === 'empty') {
        const next = new Map(core.ends)
        next.delete(attemptKeyOf(t))
        core.ends = next
      }
      // A late saved reply (loading_saved, lost, error → settled) earns its dot (v1c E3). Only the signals hear
      // it: `endFor` and the page's routed outcomes stay as they were (EN-5, NE-4), and history already holds it.
      if (core.ends.get(attemptKeyOf(t))?.kind !== 'reply')
        emitEnd({
          id: t.id,
          sessionId,
          attemptKey: attemptKeyOf(t),
          userMessageId: t.userMessageId,
          traceId: t.state.traceId,
          kind: 'reply',
          terminal: true,
          finishedAt: deps.now(),
          chatMode: 'routed',
        })
      turns.delete(sessionId)
      notify()
      return
    }
    const done = next !== t.phase && !isLivePhase(next)
    const cutShort =
      (next === 'draining' || next === 'loading_saved') &&
      (ev.type === 'limit' || ev.type === 'ceiling' || ev.type === 'truncated')
        ? { incomplete: true }
        : {}
    update(sessionId, { ...patch, ...cutShort, phase: next, finalized: done || t.finalized })
    if (UNCERTAIN.has(next)) unknown.add(sessionId)
    if (done) {
      const cur = turns.get(sessionId)
      if (cur) {
        if (next === 'done') keepRoutedSteps(cur)
        recordEnd(cur, next)
      }
    }
  }

  /**
   * A routed attempt's stream (v1b §5.3-§5.4). A first send drains an over-limit stream to EOF so
   * the server reaches Done and saves; a resume cancels (its continuation is buffered).
   */
  async function streamRouted(sessionId: string) {
    const t = turns.get(sessionId)
    if (!t || t.phase === 'aborted') return
    const tid = t.id
    const resume = t.operation === 'resume'
    const ctrl = new AbortController()
    controllers.set(sessionId, ctrl)
    update(sessionId, {
      phase: 'waiting',
      state: emptyTurn(),
      finalized: false,
      idle: false,
      stopped: false,
      error: null,
    })
    let res: Response
    try {
      const body =
        resume && t.resumedRequestId
          ? resumeBody({
              rpcId: deps.newId(),
              messageId: deps.newId(),
              sessionId,
              requestId: t.resumedRequestId,
            })
          : dispatchBody({
              rpcId: deps.newId(),
              messageId: deps.newId(),
              sessionId,
              text: t.userText,
              chatMode: 'routed',
            })
      res = await deps.dispatch(body, ctrl.signal)
      if (!isCurrent(sessionId, tid)) {
        void res.body?.cancel().catch(() => undefined)
        return
      }
    } catch (err) {
      if ((err as { name?: string }).name === 'AbortError') return
      routedStep(
        sessionId,
        { type: 'dispatch_unknown', resume },
        { error: new ChatError({ phase: 'dispatch', key: 'cutOff', certainty: 'unknown' }) },
      )
      return
    }
    if (res.status === 401) {
      deps.onUnauthorized()
      void res.body?.cancel().catch(() => undefined)
      // Nothing ran: forget the turn, so after signing back in history's no-reply notice (with
      // Run again) shows instead of a turn that blocks it (ship review D4).
      turns.delete(sessionId)
      notify()
      return
    }
    if (!res.ok) {
      const raw = await res.text().catch(() => '')
      let body: unknown = raw
      try {
        body = raw ? JSON.parse(raw) : null
      } catch {
        /* plain text body */
      }
      const error = dispatchError(res.status, body, {
        chatMode: 'routed',
        operation: t.operation,
      })
      const ev: RoutedEvent =
        error.certainty === 'rejected-before-run'
          ? { type: 'dispatch_rejected', resume, status: res.status }
          : { type: 'dispatch_unknown', resume }
      routedStep(sessionId, ev, { error })
      return
    }
    armIdle(sessionId)
    let draining = false
    let stopReason: 'ceiling' | 'truncated' | 'limit' | 'expired' | null = null
    let ceiling: ReturnType<typeof setTimeout> | undefined
    // A reconnect to an expired continuation buffer stays open with no frames for up to an hour
    // (continuation.rs `watch` recreates a non-terminal buffer): give up on a silent one.
    let firstFrame = resume
      ? setTimeout(() => {
          stopReason = 'expired'
          ctrl.abort()
        }, tuning.RESUME_FIRST_FRAME_MS)
      : undefined
    const clearTimers = () => {
      clearTimeout(ceiling)
      clearTimeout(firstFrame)
      firstFrame = undefined
    }
    // scaffolding: remove when the server routed reply tap ships (rec 1)
    const startDrain = () => {
      if (draining) return
      draining = true
      // Draining decodes nothing, so the idle timer would call a busy stream stalled; the ceiling bounds it.
      clearTimeout(idleTimers.get(sessionId))
      idleTimers.delete(sessionId)
      if (turns.get(sessionId)?.idle) update(sessionId, { idle: false })
      // The cap can trip on the first chunk, before any event moved the turn to streaming.
      if (turns.get(sessionId)?.phase === 'waiting') update(sessionId, { phase: 'streaming' })
      routedStep(sessionId, { type: 'limit', resume: false })
      ceiling = setTimeout(() => {
        stopReason = 'ceiling'
        ctrl.abort()
      }, tuning.DRAIN_CEILING_MS)
    }
    try {
      await readSse(
        res,
        (events) => {
          const cur = turns.get(sessionId)
          if (!cur || cur.id !== tid || cur.phase === 'aborted' || draining) return
          if (firstFrame) {
            clearTimeout(firstFrame)
            firstFrame = undefined
          }
          let state = cur.state
          let frames = cur.frames
          for (const ev of events) {
            frames = pushFrame({ ...cur, frames }, ev.data)
            const r = reduceSseEvent(state, ev)
            state = r.state
            if (r.bad)
              deps.warn?.('chat: skipped an unreadable stream event', {
                turnId: cur.id,
                frameIndex: frames.length - 1,
              })
          }
          armIdle(sessionId)
          turns.set(sessionId, {
            ...cur,
            state,
            frames,
            phase: cur.phase === 'waiting' ? 'streaming' : cur.phase,
            idle: false,
          })
          notify()
          if (state.badFrames > tuning.MAX_BAD_FRAMES) {
            if (resume) {
              stopReason = 'limit'
              ctrl.abort()
            } else startDrain()
          } else if (resume && state.truncated) {
            stopReason = 'truncated'
            ctrl.abort()
          }
        },
        {
          signal: ctrl.signal,
          maxEventChars: tuning.MAX_EVENT_CHARS,
          maxStreamBytes: tuning.MAX_TURN_BYTES,
          onLimit: resume ? 'cancel' : 'drain',
          onDrain: startDrain,
        },
      )
    } catch (err) {
      clearTimers()
      const cur = turns.get(sessionId)
      if (!cur || cur.id !== tid || cur.phase === 'aborted') return
      if (
        !stopReason &&
        !(err instanceof SseOverflowError) &&
        (err as { name?: string }).name === 'AbortError'
      )
        return
      // A cap or a cut can land on the first chunk, before any event moved the turn off `waiting`.
      if (cur.phase === 'waiting') update(sessionId, { phase: 'streaming' })
      if (stopReason === 'ceiling') routedStep(sessionId, { type: 'ceiling' })
      else if (stopReason === 'truncated') routedStep(sessionId, { type: 'truncated' })
      else if (stopReason === 'expired')
        routedStep(
          sessionId,
          { type: 'eof', resume: true },
          {
            error: new ChatError({
              phase: 'stream',
              key: 'routedMayStillArrive',
              certainty: 'unknown',
            }),
          },
        )
      else if (stopReason === 'limit' || err instanceof SseOverflowError)
        routedStep(sessionId, { type: 'limit', resume: true })
      else
        routedStep(
          sessionId,
          { type: 'eof', resume },
          { error: new ChatError({ phase: 'stream', key: 'cutOff', certainty: 'unknown' }) },
        )
      return
    }
    clearTimers()
    const cur = turns.get(sessionId)
    if (!cur || cur.id !== tid || cur.phase === 'aborted') return
    // The stream ended without a single frame: nothing to read, the reply is unconfirmed.
    if (cur.phase === 'waiting') update(sessionId, { phase: 'streaming' })
    const st = cur.state
    if (st.request) routedStep(sessionId, { type: 'pause' })
    else if (st.taskState === 'completed' && !st.error)
      routedStep(sessionId, { type: 'terminal', text: !!routedReplyText(st, resume).trim() })
    else if (
      st.taskState === 'failed' ||
      st.taskState === 'rejected' ||
      st.taskState === 'canceled' ||
      st.error
    ) {
      routedStep(
        sessionId,
        { type: 'task_failed' },
        {
          error: new ChatError({
            phase: 'stream',
            key: 'routedFailed',
            certainty: 'unknown',
            serverDetail: st.error?.message,
          }),
        },
      )
    } else
      routedStep(
        sessionId,
        { type: 'eof', resume },
        { error: new ChatError({ phase: 'stream', key: 'cutOff', certainty: 'unknown' }) },
      )
  }

  /**
   * A routed turn's history check (§5.4): `match` when history holds its reply (routedSettles),
   * `no_match` after a refetch without it (the page re-checks with backoff), `timeout` once those
   * re-checks gave up, `error` when the refetch failed.
   */
  function reconcile(sessionId: string, result: 'match' | 'no_match' | 'timeout' | 'error') {
    const t = turns.get(sessionId)
    if (!t || t.chatMode !== 'routed' || isLivePhase(t.phase)) return
    routedStep(sessionId, {
      type: 'history',
      result,
      hadText: !!routedReplyText(t.state, t.operation === 'resume').trim() || !!t.incomplete,
    })
  }

  return { routedStep, streamRouted, reconcile }
}
