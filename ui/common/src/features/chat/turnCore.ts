/**
 * The turn registry's shared state and primitives (plan §6.1-§6.2): the working maps, the published store,
 * the direct turn-end store, Web Locks, timers and the start claim. `turnSend.ts`, `turnResume.ts` and
 * `turnRouted.ts` build the pipelines on it; `turnRegistry.ts` assembles them.
 */
import { createStore } from 'zustand/vanilla'
import { ApiError } from '@/lib/api/client'
import { replyText, type Step } from './a2aReducer'
import { ChatError } from './errors'
import { tuning } from './tuning'
import type { TranscriptOwnership } from './types'
import {
  attemptKeyOf,
  sawTerminal,
  type ChatDeps,
  type LiveTurn,
  type RegistryState,
  type TurnEnd,
} from './turnMachine'

export type StartKind = 'send' | 'resume' | 'runAgain'

/** Replies whose tool steps a tab keeps for the finished-turn chip (ISSUE-001). */
const SAVED_STEPS_MAX = 200

export function createCore(deps: ChatDeps) {
  /** The working copy: pipelines read and write it; renders read `store` (published by `notify`). */
  const turns = new Map<string, LiveTurn>()
  const store = createStore<RegistryState>(() => ({
    turns: new Map(),
    list: [],
    ends: new Map(),
    steps: new Map(),
  }))

  const core = {
    deps,
    turns,
    store,
    controllers: new Map<string, AbortController>(),
    /** The lock holder per chat, tagged with the run that took it, so a stale run can't release a newer one. */
    releases: new Map<string, { run: string; fn: () => void }>(),
    /** Sends and runs claimed synchronously, before the first await (a double Enter must not send twice). */
    starting: new Map<
      string,
      { kind: StartKind; text?: string; p: Promise<LiveTurn | undefined> }
    >(),
    saving: new Set<string>(),
    /**
     * Tool steps of replies this tab saved, by message id. The server stores no steps, so once
     * history replaces the live turn this is what keeps the finished turn's "N tools" chip (§7.4).
     * Tab-scoped: a reload shows the reply without steps. Replaced, never mutated (it is published).
     */
    savedSteps: new Map() as ReadonlyMap<string, Step[]>,
    idleTimers: new Map<string, ReturnType<typeof setTimeout>>(),
    unknown: new Set<string>(),
    transcripts: new Map<string, TranscriptOwnership | undefined>(),
    /** Routed turn ends, by attempt key, oldest first (NE-4). Replaced, never mutated (it is published). */
    ends: new Map() as ReadonlyMap<string, TurnEnd>,
    /** Direct turn ends (v1c §5.8), apart from `ends` so `endFor` and the direct page's E5 status are unchanged. */
    directEnds: new Map<string, TurnEnd>(),
    /** Direct attempts whose end the save cap recorded: the later save records only a failure (DS-T2). */
    capped: new Set<string>(),
    capTimers: new Map<string, ReturnType<typeof setTimeout>>(),
    /** Every end, routed and direct, as it is recorded (v1c §5.8). */
    endListeners: new Set<(end: TurnEnd) => void>(),
    /** A routed resume asked for while the paused stream still held the chat (EN-4, NE-3). */
    pendingResume: new Map<string, QueuedResume>(),
    /** Set by clearAll: retries scheduled before it must not reconnect for a signed-out user. */
    generation: 0,
    /** Per chat, bumped by abort (delete): a lock retry scheduled before it must not reconnect there. */
    abortCount: new Map<string, number>(),
    /** Hitl ids this registry already reconnected for: at most one reconnect each (NE-3). */
    reconnected: new Set<string>(),
    reruns: new Map<string, number>(),
    /** Starts a queued routed resume once the chat is free (set by the resume module, EN-4). */
    startQueuedResume: (_sessionId: string, _queued: QueuedResume): void => undefined,

    /** Publish: the working maps become the next immutable snapshot, and subscribers hear it. */
    notify() {
      store.setState({
        turns: new Map(turns),
        list: [...turns.values()],
        ends: core.ends,
        steps: core.savedSteps,
      })
    },

    update(sessionId: string, patch: Partial<LiveTurn>) {
      const t = turns.get(sessionId)
      // An aborted turn (deleted chat, user switch) stays aborted: a save or error read that was
      // already in flight must not bring it back as "unsaved" and hold the leave-page prompt forever.
      if (!t || t.phase === 'aborted') return
      const next = { ...t, ...patch }
      turns.set(sessionId, next)
      if (patch.phase && patch.phase !== t.phase) directPhase(t, next)
      core.notify()
    },

    emitEnd: (end: TurnEnd) => core.endListeners.forEach((l) => l(end)),

    /** The turn a start path just set: nothing between its `turns.set` and its return can remove it. */
    started(sessionId: string): LiveTurn {
      const t = turns.get(sessionId)
      if (!t) throw new Error(`[chat] the turn for ${sessionId} vanished as it started`)
      return t
    },

    /** The turn this pipeline started is still the chat's turn and wasn't aborted (delete, user switch). */
    isCurrent(sessionId: string, turnId: string) {
      const t = turns.get(sessionId)
      return !!t && t.id === turnId && t.phase !== 'aborted'
    },

    /** A 401 from a REST call: hand off to the login redirect, as the dispatch path does. */
    expired(err: unknown) {
      if (!(err instanceof ApiError) || err.status !== 401) return false
      deps.onUnauthorized()
      return true
    },

    /** Resolves to a release function, or null when another tab holds the chat. */
    acquire(sessionId: string): Promise<(() => void) | null> {
      const locks = deps.locks
      if (!locks) return Promise.resolve(() => undefined)
      return new Promise((resolve) => {
        locks
          .request(lockName(sessionId), { ifAvailable: true }, (lock) => {
            if (!lock) {
              resolve(null)
              return undefined
            }
            // Hold the lock until the turn releases it.
            return new Promise<void>((release) => resolve(release))
            // A lock manager that throws (sandboxed or opaque origin) is the same as having none.
          })
          .catch(() => resolve(() => undefined))
      })
    },

    hold(sessionId: string, run: string, fn: () => void) {
      core.releases.set(sessionId, { run, fn })
    },

    /** Release the chat's lock and timers; with `run`, only if that run still holds them. */
    release(sessionId: string, run?: string) {
      const held = core.releases.get(sessionId)
      if (run && held?.run !== run) return
      held?.fn()
      core.releases.delete(sessionId)
      const timer = core.idleTimers.get(sessionId)
      if (timer) clearTimeout(timer)
      core.idleTimers.delete(sessionId)
      core.controllers.delete(sessionId)
    },

    armIdle(sessionId: string) {
      const prev = core.idleTimers.get(sessionId)
      if (prev) clearTimeout(prev)
      core.idleTimers.set(
        sessionId,
        setTimeout(() => {
          // EN2: a notice, never an abort. The agent may be in a long tool call.
          core.unknown.add(sessionId)
          core.update(sessionId, { idle: true })
        }, tuning.STREAM_IDLE_MS),
      )
    },

    /** Keep a reply's tool steps; published with the next notify (every caller moves the turn right after). */
    keepSteps(key: string, steps: Step[]) {
      // Tool results can be large: keep the start of each so the chip still expands.
      const next = new Map(core.savedSteps)
      next.set(
        key,
        steps.map((s) =>
          typeof s.detail === 'string' && s.detail.length > tuning.SAVED_DETAIL_MAX
            ? { ...s, detail: `${s.detail.slice(0, tuning.SAVED_DETAIL_MAX)}…` }
            : s,
        ),
      )
      // Oldest first out once the tab has kept SAVED_STEPS_MAX replies' steps.
      if (next.size > SAVED_STEPS_MAX) {
        const oldest = next.keys().next().value
        if (oldest !== undefined) next.delete(oldest)
      }
      core.savedSteps = next
    },

    pushFrame(t: LiveTurn, data: string): string[] {
      const frames = [
        ...t.frames,
        data.length > tuning.DEBUG_FRAME_CHARS
          ? `${data.slice(0, tuning.DEBUG_FRAME_CHARS)}…`
          : data,
      ]
      return frames.length > tuning.DEBUG_FRAMES ? frames.slice(-tuning.DEBUG_FRAMES) : frames
    },

    /** A direct end (v1c §5.8): the attempt's stream, save or failure has settled. */
    putDirectEnd(t: LiveTurn, kind: TurnEnd['kind'], attemptKey = attemptKeyOf(t)) {
      const end: TurnEnd = {
        id: t.id,
        sessionId: t.sessionId,
        attemptKey,
        userMessageId: t.userMessageId,
        traceId: t.state.traceId,
        kind,
        terminal: sawTerminal(t.state, t.operation === 'resume'),
        finishedAt: deps.now(),
        chatMode: 'direct',
      }
      const { directEnds } = core
      directEnds.delete(attemptKey)
      directEnds.set(attemptKey, end)
      if (directEnds.size > tuning.TURN_ENDS_MAX) {
        const oldest = directEnds.keys().next().value
        if (oldest !== undefined) directEnds.delete(oldest)
      }
      core.emitEnd(end)
    },

    /**
     * Claim a chat synchronously for one start. A second start of the same kind while the first is
     * pending gets the same promise (a double Enter); a different kind is refused, so a send never
     * resolves with a resume's turn and loses its text.
     */
    claim<T extends LiveTurn | undefined>(
      sessionId: string,
      kind: StartKind,
      start: () => Promise<T>,
      text?: string,
    ): Promise<T> {
      const pending = core.starting.get(sessionId)
      // Same kind and text is the same action twice (a double Enter), so it shares the pending
      // promise, which then has this caller's result type. Anything else is refused: a second,
      // different message must not resolve with the first one's turn and vanish.
      if (pending) {
        return pending.kind === kind && pending.text === text
          ? (pending.p as Promise<T>)
          : Promise.reject(
              new ChatError({ phase: 'dispatch', key: 'busy', certainty: 'not-dispatched' }),
            )
      }
      const p = start().finally(() => core.starting.delete(sessionId))
      core.starting.set(sessionId, { kind, text, p })
      return p
    },

    /** Run one pipeline under the chat's lock; a resume queued behind it starts after (EN-4). */
    async run(sessionId: string, runId: string, work: () => Promise<void>) {
      try {
        await work()
      } finally {
        core.release(sessionId, runId)
        // A resume asked for while this stream still held the chat starts now (EN-4).
        // Next tick: a Web Lock is released asynchronously after its holder settles.
        const queued = core.pendingResume.get(sessionId)
        if (queued && !core.releases.has(sessionId)) {
          core.pendingResume.delete(sessionId)
          // Only for the paused turn it was queued behind; a later, unrelated run drops it.
          if (!queued.forTurn || turns.get(sessionId)?.id === queued.forTurn)
            setTimeout(() => core.startQueuedResume(sessionId, queued), 0)
        }
      }
    },
  }

  const lockName = (sessionId: string) => `openruntime-chat:${deps.userId}:${sessionId}`

  /**
   * Direct terminal phases (v1c §5.8): `done` → reply (empty without text), `no_reply` → empty, `error` and
   * `unsaved` → error, `paused` → paused. One end per attempt, and only once its user row exists (E12: its id
   * is known, or the server writes it and the stream started). A
   * stopped attempt records none (the user saw it stop). A Save again that succeeds after `unsaved`
   * supersedes the error with `<key>:reply`; after a save-cap end only a failure adds `<key>:error`.
   */
  function directPhase(prev: LiveTurn, t: LiveTurn) {
    if (t.chatMode === 'routed' || t.stopped || !(t.userMessageId || t.userRowServer)) return
    const kind: TurnEnd['kind'] | null =
      t.phase === 'done'
        ? replyText(t.state).trim() || t.saved
          ? 'reply'
          : 'empty'
        : t.phase === 'no_reply'
          ? 'empty'
          : t.phase === 'error' || t.phase === 'unsaved'
            ? 'error'
            : t.phase === 'paused'
              ? 'paused'
              : null
    if (!kind) return
    const key = attemptKeyOf(t)
    const { capped, directEnds } = core
    if (capped.has(key)) {
      if (t.phase === 'unsaved' && !directEnds.has(`${key}:error`))
        core.putDirectEnd(t, 'error', `${key}:error`)
      return
    }
    if (!directEnds.has(key)) return core.putDirectEnd(t, kind)
    if (prev.phase === 'unsaved' && t.phase === 'done' && !directEnds.has(`${key}:reply`))
      core.putDirectEnd(t, 'reply', `${key}:reply`)
  }

  return core
}

export type Core = ReturnType<typeof createCore>
export type QueuedResume = {
  requestId: string
  userMessageId?: string
  forTurn?: string
  baselineReplies?: number
}
