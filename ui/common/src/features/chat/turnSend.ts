/**
 * Sending (plan §6.1, EN1-EN4, EN7-EN8): create the chat, save the user row, stream a direct reply and save it
 * (a routed stream goes to `turnRouted`); Run again and Save again. A direct reply is the client's to save.
 */
import { emptyTurn, reduceSseEvent } from './a2aReducer'
import { readSse, SseOverflowError } from '@/lib/sse'
import { ChatError, dispatchError, restError } from './errors'
import { clientWrites, dispatchBody, outcomeOf, resumeBody } from './send'
import { tuning } from './tuning'
import type { SaveMessageBody } from './types'
import type { Core } from './turnCore'
import {
  attemptKeyOf,
  isLivePhase,
  type LiveTurn,
  type RerunFrom,
  type SendInput,
} from './turnMachine'
import type { createRouted } from './turnRouted'

class Unreadable extends Error {}

export function createSend(core: Core, routed: ReturnType<typeof createRouted>) {
  const { deps, turns, controllers, unknown, transcripts, directEnds, capped, capTimers, saving } =
    core
  const { update, notify, isCurrent, expired, acquire, hold, armIdle, pushFrame, keepSteps } = core
  const { claim, run, started, putDirectEnd, reruns } = core
  const { routedStep, streamRouted } = routed
  const liveCount = () => {
    let n = 0
    for (const t of turns.values()) if (isLivePhase(t.phase)) n++
    return n
  }

  async function finalize(sessionId: string) {
    const t = turns.get(sessionId)
    if (!t || t.finalized || t.phase === 'aborted') return
    update(sessionId, { finalized: true })
    const outcome = outcomeOf(t.state, t.stopped)
    if (t.stopped) unknown.add(sessionId)
    if (t.operation === 'resume') {
      // The server saves the resumed reply; history is the truth. A silent or cut-short
      // resume reconciles through history too (EN9), so nothing here marks it failed.
      update(sessionId, { phase: outcome.kind === 'paused' ? 'paused' : 'done' })
      return
    }
    switch (outcome.kind) {
      case 'paused':
        update(sessionId, { phase: 'paused' })
        return
      case 'no-reply':
        unknown.add(sessionId)
        update(sessionId, { phase: 'no_reply' })
        return
      case 'agent-failed':
        unknown.add(sessionId)
        update(sessionId, {
          phase: 'error',
          error: new ChatError({
            phase: 'stream',
            key: 'agentFailed',
            certainty: 'unknown',
            serverDetail: t.state.error?.message,
          }),
        })
        return
      case 'save': {
        if (!clientWrites(transcripts.get(sessionId), 'assistant')) {
          update(sessionId, { phase: 'done' })
          return
        }
        await saveAssistant(sessionId, outcome.body)
      }
    }
  }

  // quirk: §10.1, §10.4 — the client saves direct replies (lost if the tab closes) and a retry can duplicate them.
  async function saveAssistant(sessionId: string, body: SaveMessageBody) {
    // A save that hangs must not swallow the signal (v1c §5.8): past DIRECT_END_SAVE_CAP_MS the end is
    // recorded as a reply (a turn that is saving has one). Never for a stopped attempt.
    const t0 = turns.get(sessionId)
    const key = t0 ? attemptKeyOf(t0) : ''
    const gen = core.generation
    if (
      t0 &&
      t0.chatMode !== 'routed' &&
      !t0.stopped &&
      (t0.userMessageId || t0.userRowServer) &&
      !directEnds.has(key) &&
      !capped.has(key)
    ) {
      clearTimeout(capTimers.get(sessionId))
      capTimers.set(
        sessionId,
        setTimeout(() => {
          capTimers.delete(sessionId)
          const cur = turns.get(sessionId)
          if (
            gen !== core.generation ||
            !cur ||
            cur.id !== t0.id ||
            cur.phase === 'aborted' ||
            cur.stopped ||
            directEnds.has(key)
          )
            return
          capped.add(key)
          putDirectEnd(cur, 'reply')
        }, tuning.DIRECT_END_SAVE_CAP_MS),
      )
    }
    try {
      const saved = await deps.saveMessage(sessionId, body)
      const steps =
        turns.get(sessionId)?.phase === 'aborted' ? undefined : turns.get(sessionId)?.state.steps
      if (steps?.length) keepSteps(saved.id, steps)
      if (!turns.get(sessionId)?.stopped) unknown.delete(sessionId)
      update(sessionId, { phase: 'done', saved, pendingSave: null, error: null })
      // Persistence refreshes on its own (E4), even after a cap end already notified.
      if (gen === core.generation && turns.get(sessionId)?.phase !== 'aborted')
        deps.onDirectSaved?.(sessionId)
    } catch (err) {
      // A 401 keeps the reply for Save again after signing back in.
      expired(err)
      update(sessionId, {
        phase: 'unsaved',
        pendingSave: body,
        error: restError('save-assistant', err),
      })
    } finally {
      if (t0 && turns.get(sessionId)?.id === t0.id) {
        clearTimeout(capTimers.get(sessionId))
        capTimers.delete(sessionId)
      }
    }
  }

  async function stream(sessionId: string) {
    const t = turns.get(sessionId)
    if (!t || t.phase === 'aborted') return
    if (t.chatMode === 'routed') return streamRouted(sessionId)
    const tid = t.id
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
        t.operation === 'resume' && t.resumedRequestId
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
              chatMode: 'direct',
              agentId: t.agentId ?? '',
              text: t.userText,
            })
      res = await deps.dispatch(body, ctrl.signal)
      if (!isCurrent(sessionId, tid)) {
        void res.body?.cancel().catch(() => undefined)
        return
      }
    } catch (err) {
      if ((err as { name?: string }).name === 'AbortError') {
        if (turns.get(sessionId)?.stopped) await finalize(sessionId)
        return
      }
      unknown.add(sessionId)
      update(sessionId, {
        phase: 'error',
        finalized: true,
        error: new ChatError({ phase: 'dispatch', key: 'cutOff', certainty: 'unknown' }),
      })
      return
    }
    if (res.status === 401) {
      // The app redirects to login; no "cut off" copy for an expired session.
      deps.onUnauthorized()
      void res.body?.cancel().catch(() => undefined)
      update(sessionId, { phase: 'aborted', finalized: true })
      return
    }
    if (!res.ok && turns.get(sessionId)?.operation === 'resume') {
      // 404/409/429 on a resume: reconcile through history instead of a generic error (EN9).
      void res.body?.cancel().catch(() => undefined)
      update(sessionId, { phase: 'done', finalized: true })
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
      const error = dispatchError(res.status, body)
      if (error.certainty === 'unknown') unknown.add(sessionId)
      update(sessionId, { phase: 'error', finalized: true, error })
      return
    }
    armIdle(sessionId)
    try {
      await readSse(
        res,
        (events) => {
          const cur = turns.get(sessionId)
          if (!cur || cur.id !== tid || cur.phase === 'aborted') return
          let state = cur.state
          let frames = cur.frames
          for (const ev of events) {
            frames = pushFrame({ ...cur, frames }, ev.data)
            const r = reduceSseEvent(state, ev)
            state = r.state
            if (r.bad) {
              deps.warn?.('chat: skipped an unreadable stream event', {
                turnId: cur.id,
                frameIndex: frames.length - 1,
              })
              if (state.badFrames > tuning.MAX_BAD_FRAMES) throw new Unreadable()
            }
          }
          armIdle(sessionId)
          turns.set(sessionId, { ...cur, state, frames, phase: 'streaming', idle: false })
          notify()
        },
        {
          signal: ctrl.signal,
          maxEventChars: tuning.MAX_EVENT_CHARS,
          maxStreamBytes: tuning.MAX_TURN_BYTES,
        },
      )
    } catch (err) {
      const cur = turns.get(sessionId)
      if (!cur) return
      if ((err as { name?: string }).name === 'AbortError') {
        if (cur.stopped) await finalize(sessionId)
        return
      }
      if (cur.operation === 'resume') {
        update(sessionId, { phase: 'done', finalized: true })
        return
      }
      unknown.add(sessionId)
      const key =
        err instanceof SseOverflowError
          ? 'tooLarge'
          : err instanceof Unreadable
            ? 'unreadable'
            : 'cutOff'
      update(sessionId, {
        phase: 'error',
        finalized: true,
        error: new ChatError({ phase: 'stream', key, certainty: 'unknown' }),
      })
      return
    }
    await finalize(sessionId)
  }

  /** Start a turn. Throws ChatError for failures that happen before anything is dispatched. */
  function send(input: SendInput): Promise<LiveTurn> {
    const text = input.text.trim()
    if (!text)
      return Promise.reject(
        new ChatError({ phase: 'save-user', key: 'saveUserFailed', certainty: 'not-dispatched' }),
      )
    if (text.length > tuning.MESSAGE_MAX_CHARS)
      return Promise.reject(
        new ChatError({ phase: 'save-user', key: 'tooLong', certainty: 'not-dispatched' }),
      )
    const isNew = input.create === true || !input.sessionId
    const sessionId = input.sessionId ?? deps.newId()
    const routed = input.chatMode === 'routed'
    // HTTP/1.1 gives a page 6 connections per origin: a 4th live routed stream would starve the app (G-10).
    if (routed && liveCount() >= tuning.MAX_LIVE_TURNS)
      return Promise.reject(
        new ChatError({ phase: 'dispatch', key: 'tooManyLive', certainty: 'not-dispatched' }),
      )
    const agentId = routed ? null : input.agentId
    return claim(
      sessionId,
      'send',
      async () => {
        const existing = turns.get(sessionId)
        if (existing && isLivePhase(existing.phase))
          throw new ChatError({ phase: 'dispatch', key: 'otherTab', certainty: 'not-dispatched' })
        const releaseLock = await acquire(sessionId)
        if (!releaseLock)
          throw new ChatError({ phase: 'dispatch', key: 'otherTab', certainty: 'not-dispatched' })
        const runId = deps.newId()
        hold(sessionId, runId, releaseLock)
        // Ownership is per chat: a send without it keeps what the chat's row already said.
        // Routed turns ignore it: the client never saves their reply, whatever a field says (G-4).
        if (!routed) transcripts.set(sessionId, input.transcript ?? transcripts.get(sessionId))
        const tid = deps.newId()
        turns.set(sessionId, {
          id: tid,
          sessionId,
          agentId,
          userText: text,
          phase: isNew ? 'creating' : 'saving_user',
          state: emptyTurn(),
          startedAt: deps.now(),
          idle: false,
          stopped: false,
          finalized: false,
          error: null,
          saved: null,
          pendingSave: null,
          frames: [],
          chatMode: routed ? 'routed' : 'direct',
          operation: 'send',
          attempt: 'send',
        })
        notify()
        input.onStart?.(sessionId)
        void run(sessionId, runId, async () => {
          if (isNew) {
            const ctrl = new AbortController()
            const timer = setTimeout(() => ctrl.abort(), tuning.CREATE_TIMEOUT_MS)
            try {
              const row = await deps.createSession(
                routed
                  ? { session_id: sessionId, first_prompt: text }
                  : { session_id: sessionId, agent_id: input.agentId, first_prompt: text },
                ctrl.signal,
              )
              if (!isCurrent(sessionId, tid)) return
              if (!routed) transcripts.set(sessionId, row.transcript ?? input.transcript)
              input.onCreated?.(row)
            } catch (err) {
              expired(err)
              if (!isCurrent(sessionId, tid)) return
              if (routed)
                routedStep(
                  sessionId,
                  { type: 'create_failed' },
                  { error: restError('create', err) },
                )
              else
                update(sessionId, {
                  phase: 'error',
                  finalized: true,
                  error: restError('create', err),
                })
              input.onCreateFailed?.(sessionId, text)
              return
            } finally {
              clearTimeout(timer)
            }
            update(sessionId, { phase: 'saving_user' })
          }
          // The client always owns a routed chat's user row (§2.3).
          if (routed || clientWrites(transcripts.get(sessionId), 'user')) {
            try {
              const saved = await deps.saveMessage(sessionId, { role: 'user', content: text })
              if (!isCurrent(sessionId, tid)) return
              update(sessionId, { userMessageId: saved.id })
            } catch (err) {
              expired(err)
              if (!isCurrent(sessionId, tid)) return
              if (routed)
                routedStep(
                  sessionId,
                  { type: 'create_failed' },
                  { error: restError('save-user', err) },
                )
              else
                update(sessionId, {
                  phase: 'error',
                  finalized: true,
                  error: restError('save-user', err),
                })
              return
            }
          } else update(sessionId, { userRowServer: true })
          if (!isCurrent(sessionId, tid)) return
          await stream(sessionId)
        })
        return started(sessionId)
      },
      text,
    )
  }

  /** Re-dispatch the same message without a new user row. The UI confirms first when needed. */
  // quirk: §10.3 — no dispatch idempotency, so the UI confirms before running a message twice.
  async function runAgain(sessionId: string, from?: RerunFrom): Promise<void> {
    let t = turns.get(sessionId)
    let synthetic = false
    if (!t && from) {
      synthetic = true
      t = {
        id: deps.newId(),
        sessionId,
        agentId: from.agentId,
        userText: from.userText,
        userMessageId: from.userMessageId,
        phase: 'done',
        state: emptyTurn(),
        startedAt: deps.now(),
        idle: false,
        stopped: false,
        finalized: true,
        error: null,
        saved: null,
        pendingSave: null,
        frames: [],
        chatMode: from.chatMode === 'routed' ? 'routed' : 'direct',
        operation: 'send',
        attempt: 'send',
      }
      turns.set(sessionId, t)
    }
    if (!t || isLivePhase(t.phase) || t.operation === 'resume' || t.phase === 'aborted') return
    const turn = t
    try {
      await claim(sessionId, 'runAgain', async () => {
        const releaseLock = await acquire(sessionId)
        if (!releaseLock)
          throw new ChatError({ phase: 'dispatch', key: 'otherTab', certainty: 'not-dispatched' })
        const runId = deps.newId()
        hold(sessionId, runId, releaseLock)
        unknown.delete(sessionId)
        const n = (reruns.get(sessionId) ?? 0) + 1
        reruns.set(sessionId, n)
        // Direct or routed, a Run again is its own attempt (v1c §5.8), so its end isn't the earlier attempt's.
        update(sessionId, { startedAt: deps.now(), operation: 'rerun', attempt: `rerun:${n}` })
        void run(sessionId, runId, () => stream(sessionId))
        return turns.get(sessionId) ?? turn
      })
    } catch (err) {
      // The history turn made for this Run again never started: don't leave it behind.
      if (synthetic && turns.get(sessionId) === turn) {
        turns.delete(sessionId)
        notify()
      }
      throw err
    }
  }

  /** Save again after a failed assistant save. Unknown outcomes check history first (EN7). */
  async function saveAgain(sessionId: string): Promise<void> {
    const t = turns.get(sessionId)
    // One save at a time: a double click must not post the reply twice.
    if (!t || t.phase !== 'unsaved' || !t.pendingSave || saving.has(sessionId)) return
    saving.add(sessionId)
    try {
      // quirk: §10.4 — no idempotent writes, so an ambiguous save checks history before retrying.
      if (t.error?.certainty === 'unknown' && t.userMessageId) {
        let exists: boolean
        try {
          exists = await deps.replyExists(sessionId, t.userMessageId, t.pendingSave.content)
        } catch (err) {
          // Couldn't check: stay unsaved with this error so the notice says what happened.
          expired(err)
          update(sessionId, { error: restError('save-assistant', err) })
          return
        }
        if (exists) {
          update(sessionId, { phase: 'done', pendingSave: null, error: null })
          deps.onDirectSaved?.(sessionId)
          return
        }
      }
      await saveAssistant(sessionId, t.pendingSave)
    } finally {
      saving.delete(sessionId)
    }
  }

  return { send, runAgain, saveAgain, stream, liveCount }
}
