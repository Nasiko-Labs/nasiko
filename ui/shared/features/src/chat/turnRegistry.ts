/**
 * Live turns, outside React (plan §6.1-§6.2, EN1-EN4). A page subscribes; leaving the page never
 * stops a turn. One live turn per chat, guarded across tabs by a Web Lock.
 *
 *   send ─ lock ─► creating ─► saving_user ─► waiting ─► streaming ──EOF──► finalize (once)
 *                                   │             │          │                ├─ saved   → done
 *                                   │             │          │                ├─ pause   → paused
 *                                   │             │          │                ├─ no text → no_reply  (execution unknown)
 *                                   │             │          │                └─ save failed → unsaved
 *                                   │             │          ├─ Stop receiving → finalize(stopped)  (execution unknown)
 *                                   │             │          ├─ idle > STREAM_IDLE_MS → notice only, read continues
 *                                   ▼             ▼          └─ error → error (certainty decides Run again)
 *                                 error         error
 *   abort (delete, user switch) → aborted, nothing saved
 *
 * Routed turns (v1b §5.4) share the pipeline up to the stream and then follow `stepRouted`, the
 * per-attempt state table: the server saves their reply, so the client never does, and history
 * decides when the turn is settled (`reconcile`; the page's decisions are the pure `reconcile.ts`).
 *
 * This file is the public registry and re-exports the types; the parts: `turnMachine.ts` (types, the pure
 * state table), `turnCore.ts` (shared state, the store, locks, the start claim), `turnSend.ts` (send, the
 * direct stream and save, Run again, Save again), `turnResume.ts` and `turnRouted.ts`.
 */
import type { Step } from './a2aReducer'
import { createCore } from './turnCore'
import {
  endOf,
  isLivePhase,
  stepsOf,
  type ChatDeps,
  type LiveTurn,
  type TurnEnd,
} from './turnMachine'
import { createResume } from './turnResume'
import { createRouted } from './turnRouted'
import { createSend } from './turnSend'

export * from './turnMachine'

export function createTurnRegistry(deps: ChatDeps) {
  const core = createCore(deps)
  const { turns, store, controllers, unknown, transcripts, directEnds, capped, capTimers } = core
  const { pendingResume, abortCount, reconnected, reruns, endListeners, update, notify, release } =
    core
  const routed = createRouted(core)
  const sending = createSend(core, routed)
  const { resume } = createResume(core, sending.stream)

  const registry = {
    get: (sessionId: string): LiveTurn | undefined => turns.get(sessionId),

    /** Renders select from this store's immutable state (`useLiveTurn`); effects and tests use the methods. */
    store,

    subscribe: (listener: () => void) => store.subscribe(listener),

    isExecutionUnknown: (sessionId: string) => unknown.has(sessionId),
    clearExecutionUnknown(sessionId: string) {
      unknown.delete(sessionId)
      notify()
    },

    /** Start a turn. Throws ChatError for failures that happen before anything is dispatched. */
    send: sending.send,
    /**
     * After a request is answered: stream the resumed reply into the same turn. One active
     * resume per request id; the server saves the reply, so this never POSTs a message.
     */
    resume,
    /** Re-dispatch the same message without a new user row. The UI confirms first when needed. */
    runAgain: sending.runAgain,
    /** Save again after a failed assistant save. Unknown outcomes check history first (EN7). */
    saveAgain: sending.saveAgain,

    /** Stop receiving (direct chats only; the UI hides it elsewhere). Saves the partial once. */
    // quirk: §10.3 — no cancel endpoint: Stop receiving only stops reading; the agent keeps running.
    stop(sessionId: string) {
      const t = turns.get(sessionId)
      const ctrl = controllers.get(sessionId)
      // Only once the turn is dispatched: before that there is nothing running to stop. Never on a
      // routed chat: the server drops the reply when the client stops reading (§2.3, DS3).
      if (
        !t ||
        !ctrl ||
        t.chatMode === 'routed' ||
        (t.phase !== 'waiting' && t.phase !== 'streaming')
      )
        return
      update(sessionId, { stopped: true })
      ctrl.abort()
    },

    /** Drop a turn without saving (delete, user switch). */
    abort(sessionId: string) {
      // Even with no turn: a resume may be waiting to retry for this chat's lock.
      pendingResume.delete(sessionId)
      abortCount.set(sessionId, (abortCount.get(sessionId) ?? 0) + 1)
      const t = turns.get(sessionId)
      if (!t) return
      controllers.get(sessionId)?.abort()
      release(sessionId)
      turns.set(sessionId, { ...t, phase: 'aborted', finalized: true })
      notify()
    },

    /** Discard reply: drop an unsaved reply; the chat stays execution unknown until a reply shows. */
    discard(sessionId: string) {
      const t = turns.get(sessionId)
      if (!t || t.phase !== 'unsaved') return
      unknown.add(sessionId)
      turns.delete(sessionId)
      notify()
    },

    /** Steps recorded for a reply this tab saved, or a routed reply by its trace id (see savedSteps, R7). */
    stepsFor: (messageId: string, traceId?: string | null): Step[] | undefined =>
      stepsOf(core.savedSteps, messageId, traceId),

    /** Every chat's turn, as one array that keeps its identity until the next change (EN-3b). */
    snapshot: (): readonly LiveTurn[] => store.getState().list,

    /** Live turns in this tab, direct and routed (G-10, G-21). */
    liveCount: sending.liveCount,

    /**
     * A routed turn's history check (§5.4): `match` when history holds its reply (routedSettles),
     * `no_match` after a refetch without it (the page re-checks with backoff), `timeout` once those
     * re-checks gave up, `error` when the refetch failed.
     */
    reconcile: routed.reconcile,

    /** Routed turn ends, oldest first (NE-4). */
    ends: (): TurnEnd[] => [...core.ends.values()],

    /** Direct turn ends, oldest first (v1c §5.8). */
    directEnds: (): TurnEnd[] => [...directEnds.values()],

    /** Every end as it is recorded, routed and direct (v1c §5.8). */
    subscribeEnds(listener: (end: TurnEnd) => void) {
      endListeners.add(listener)
      return () => {
        endListeners.delete(listener)
      }
    },

    /** The newest recorded end for one user message (EN-5 outcomes read this). */
    endFor: (sessionId: string, userMessageId: string): TurnEnd | undefined =>
      endOf(core.ends, sessionId, userMessageId),

    /** Whether leaving the page would lose something: a turn still running, or a reply not saved. */
    busy(): boolean {
      for (const t of turns.values()) if (isLivePhase(t.phase) || t.phase === 'unsaved') return true
      return false
    },

    /** Forget a finished turn once history holds it. */
    forget(sessionId: string) {
      const t = turns.get(sessionId)
      if (!t || isLivePhase(t.phase)) return
      turns.delete(sessionId)
      notify()
    },

    /** User switch (or a future logout): abort everything and forget it. */
    clearAll() {
      for (const id of [...turns.keys()]) registry.abort(id)
      turns.clear()
      unknown.clear()
      transcripts.clear()
      core.savedSteps = new Map()
      core.ends = new Map()
      directEnds.clear()
      capped.clear()
      for (const t of capTimers.values()) clearTimeout(t)
      capTimers.clear()
      core.generation++
      pendingResume.clear()
      reconnected.clear()
      reruns.clear()
      notify()
    },
  }
  return registry
}

export type TurnRegistry = ReturnType<typeof createTurnRegistry>
