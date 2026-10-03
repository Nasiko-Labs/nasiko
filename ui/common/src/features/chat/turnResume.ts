/**
 * Resuming after a request is answered (§3.3, EN9; routed §5.4, EN-4, NE-3): stream the continuation into the
 * chat's turn. The server saves a resumed reply, so this never POSTs a message.
 */
import { emptyTurn } from './a2aReducer'
import { tuning } from './tuning'
import type { Core } from './turnCore'
import { isLivePhase, type ChatMode } from './turnMachine'

export function createResume(core: Core, stream: (sessionId: string) => Promise<void>) {
  const { deps, turns, starting, releases, pendingResume, abortCount, reconnected } = core
  const { notify, acquire, hold, claim, run, started } = core

  /**
   * Reconnect after a routed request is answered (§5.4): one reconnect per hitl id; asked while
   * the paused stream still holds the chat, it waits (the newest ask replaces an older one).
   * The server saves the resumed reply; this never POSTs a message.
   */
  async function resumeRouted(
    sessionId: string,
    requestId: string,
    userMessageId?: string,
    baselineReplies?: number,
    attempt = 0,
  ): Promise<void> {
    if (reconnected.has(requestId)) return
    const t = turns.get(sessionId)
    if ((t && isLivePhase(t.phase)) || starting.has(sessionId) || releases.has(sessionId)) {
      pendingResume.set(sessionId, { requestId, userMessageId, forTurn: t?.id, baselineReplies })
      return
    }
    reconnected.add(requestId)
    const gen = core.generation
    const aborts = abortCount.get(sessionId) ?? 0
    await claim(sessionId, 'resume', async () => {
      const releaseLock = await acquire(sessionId)
      if (!releaseLock) {
        // The chat's lock is held: this tab's own lock mid-release, or another tab still reading the
        // paused stream (which never hears about an answer given here). Try again for a while.
        reconnected.delete(requestId)
        if (attempt < tuning.RESUME_LOCK_RETRIES)
          setTimeout(() => {
            if (gen === core.generation && (abortCount.get(sessionId) ?? 0) === aborts)
              void resumeRouted(sessionId, requestId, userMessageId, baselineReplies, attempt + 1)
          }, tuning.RESUME_LOCK_RETRY_MS)
        return undefined
      }
      const runId = deps.newId()
      hold(sessionId, runId, releaseLock)
      turns.set(sessionId, {
        id: deps.newId(),
        sessionId,
        agentId: null,
        userText: t?.userText ?? '',
        userMessageId: userMessageId ?? t?.userMessageId,
        phase: 'waiting',
        state: emptyTurn(),
        startedAt: deps.now(),
        idle: false,
        stopped: false,
        finalized: false,
        error: null,
        saved: null,
        pendingSave: null,
        frames: [],
        resumedRequestId: requestId,
        chatMode: 'routed',
        operation: 'resume',
        attempt: `resume:${requestId}`,
        baselineReplies,
      })
      notify()
      void run(sessionId, runId, () => stream(sessionId))
      return started(sessionId)
    })
  }

  /**
   * After a request is answered: stream the resumed reply into the same turn. One active
   * resume per request id; the server saves the reply, so this never POSTs a message.
   * An unsaved reply is kept: history picks up the resumed reply without this stream.
   */
  async function resume(
    sessionId: string,
    requestId: string,
    agentId: string | null,
    userMessageId?: string,
    chatMode: ChatMode = 'direct',
    baselineReplies?: number,
  ): Promise<void> {
    if (chatMode === 'routed')
      return resumeRouted(sessionId, requestId, userMessageId, baselineReplies)
    const t = turns.get(sessionId)
    if (t && (isLivePhase(t.phase) || t.resumedRequestId === requestId || t.phase === 'unsaved'))
      return
    if (starting.has(sessionId)) return
    await claim(sessionId, 'resume', async () => {
      const releaseLock = await acquire(sessionId)
      if (!releaseLock) return undefined
      const runId = deps.newId()
      hold(sessionId, runId, releaseLock)
      turns.set(sessionId, {
        id: deps.newId(),
        sessionId,
        agentId: agentId ?? t?.agentId ?? null,
        userText: t?.userText ?? '',
        userMessageId: userMessageId ?? t?.userMessageId,
        userRowServer: t?.userRowServer,
        phase: 'waiting',
        state: emptyTurn(),
        startedAt: deps.now(),
        idle: false,
        stopped: false,
        finalized: false,
        error: null,
        saved: null,
        pendingSave: null,
        frames: [],
        resumedRequestId: requestId,
        chatMode: 'direct',
        operation: 'resume',
        // Its own attempt (v1c §5.8): a resume after `paused` ends on its own.
        attempt: `resume:${requestId}`,
      })
      notify()
      void run(sessionId, runId, () => stream(sessionId))
      return started(sessionId)
    })
  }

  core.startQueuedResume = (sessionId, q) =>
    void resumeRouted(sessionId, q.requestId, q.userMessageId, q.baselineReplies)
  return { resume }
}
