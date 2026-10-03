/**
 * Background-turn signals (v1c §5.8, EN-9, E-A3, S13, E2-E4, E10, E11, C4): what finished while you
 * looked elsewhere. Per user and in memory, created with the registry (so a turn that ends while the user
 * is on another page still counts) and cleared with it.
 * - `seen`: when each chat was last looked at (its page open in a visible tab).
 * - `completions`: each attempt's current outcome, from the registry's turn ends (a later end for the same
 *   attempt replaces it). Only a `reply` earns the dot and "Reply ready"; an `error` earns the failed mark.
 * - The open chat (`setOpenChat`, with a mount token), "Reply ready" (dismiss, a visible-time clock) and the
 *   hidden-tab title count.
 */
import { createStore } from 'zustand/vanilla'
import { baseAttemptKey, isLivePhase, type TurnEnd, type TurnRegistry } from './turnRegistry'
import { tuning } from './tuning'

export interface Completion {
  sessionId: string
  /** The attempt, without the `:reply` / `:error` suffix of a superseding end. */
  attempt: string
  kind: TurnEnd['kind']
  finishedAt: number
}

export interface RowSignal {
  live: boolean
  failed: boolean
  unseen: boolean
}

/** A chat with no marks. */
const NO_ROW: RowSignal = { live: false, failed: false, unseen: false }

/**
 * What a render reads (the rail, the phone trigger, the title): an immutable value, rebuilt on every change,
 * so compiled memoisation never holds an old answer.
 */
export interface SignalsSnapshot {
  /** Marks per chat that has any; `NO_ROW` for the rest (`rowOf`). */
  rows: ReadonlyMap<string, RowSignal>
  unseenChats: number
  replyReady: Completion | null
  titlePrefix: string
  /** `?debug=turn` (DX9): last looks and completions, as of this snapshot. */
  seen: ReadonlyMap<string, number>
  completions: readonly Completion[]
}

export const rowOf = (s: SignalsSnapshot | null, sessionId: string): RowSignal =>
  s?.rows.get(sessionId) ?? NO_ROW

export function signalsDebug(s: SignalsSnapshot, sessionId: string) {
  return {
    lastSeenAt: s.seen.get(sessionId) ?? null,
    completions: s.completions.filter((c) => c.sessionId === sessionId),
  }
}

interface Doc {
  readonly visibilityState: DocumentVisibilityState
  title: string
  addEventListener(type: 'visibilitychange', l: () => void): void
  removeEventListener(type: 'visibilitychange', l: () => void): void
}

export interface SignalsDeps {
  registry: Pick<TurnRegistry, 'subscribe' | 'subscribeEnds' | 'snapshot'>
  now(): number
  /** Refresh a chat's list row and history (a direct end with no page open, a save that settled, E4). */
  invalidate(sessionId: string): void
  /** A background failure, once (DS13, DS-T2). */
  onBackgroundError?(sessionId: string): void
  doc?: Doc | null
}

/** Our own title prefix, stripped before a new one goes on (E11). */
const PREFIX = /^\(\d+\) /

export function createSignals(deps: SignalsDeps) {
  const doc =
    deps.doc === undefined ? (typeof document === 'undefined' ? null : document) : deps.doc
  const visible = () => !doc || doc.visibilityState !== 'hidden'
  const seen = new Map<string, number>()
  const completions = new Map<string, Completion>()
  const notified = new Set<string>()
  const dismissed = new Set<string>()
  const expired = new Set<string>()
  let open: { id: string | null; token: number } = { id: null, token: 0 }
  const emit = () => {
    applyTitle()
    store.setState(snapshot(), true)
  }

  // `seen` holds one number per chat opened this sign-in; trimming it would turn old completions back into dots.
  const look = (sessionId: string, at = deps.now()) =>
    seen.set(sessionId, Math.max(seen.get(sessionId) ?? 0, at))
  /** Drop an attempt's completion and its once-only marks. */
  const forget = (attempt: string) => {
    completions.delete(attempt)
    for (const k of [`${attempt}:reply`, `${attempt}:error`]) notified.delete(k)
    dismissed.delete(attempt)
    expired.delete(attempt)
  }
  const isUnseen = (c: Completion) => c.finishedAt > (seen.get(c.sessionId) ?? 0)

  // Turn ends, routed and direct, from the moment the registry exists (E2).
  const unsubscribeEnds = deps.registry.subscribeEnds((end) => {
    const attempt = baseAttemptKey(end.attemptKey)
    // Newest last, capped like the registry's ends, so a long sign-in doesn't grow without bound.
    completions.delete(attempt)
    completions.set(attempt, {
      sessionId: end.sessionId,
      attempt,
      kind: end.kind,
      finishedAt: end.finishedAt,
    })
    if (completions.size > tuning.TURN_ENDS_MAX) {
      const oldest = completions.keys().next().value
      if (oldest !== undefined) forget(oldest)
    }
    const isOpen = open.id === end.sessionId
    // The open chat in a visible tab never gets a dot: it saw the end happen.
    if (isOpen && visible()) look(end.sessionId, end.finishedAt)
    // A direct chat that ended with no page open: refresh its rail row once (routed ends refresh already, NE-4).
    if (end.chatMode === 'direct' && !isOpen) deps.invalidate(end.sessionId)
    if (end.kind === 'error' && !isOpen && !notified.has(`${attempt}:error`)) {
      notified.add(`${attempt}:error`)
      deps.onBackgroundError?.(end.sessionId)
    }
    emit()
  })

  // Live turns for the spinner; the open chat counts as seen while its turn moves (visible tab only).
  let liveKey = ''
  const unsubscribeTurns = deps.registry.subscribe(() => {
    if (open.id && visible()) look(open.id)
    const next = deps.registry
      .snapshot()
      .filter((t) => isLivePhase(t.phase))
      .map((t) => t.sessionId)
      .sort()
      .join('|')
    if (next !== liveKey) {
      liveKey = next
      emit()
    }
  })

  // "Reply ready" counts time while it's shown in a visible tab (DS4).
  let clock: {
    attempt: string | null
    visibleMs: number
    since: number | null
    timer?: ReturnType<typeof setTimeout>
  } = { attempt: null, visibleMs: 0, since: null }
  const runClock = () => {
    if (!clock.attempt || clock.since !== null || !visible()) return
    clock.since = deps.now()
    const attempt = clock.attempt
    clock.timer = setTimeout(
      () => {
        if (clock.attempt !== attempt) return
        expired.add(attempt)
        clock = { attempt: null, visibleMs: 0, since: null }
        emit()
      },
      Math.max(0, tuning.REPLY_READY_MS - clock.visibleMs),
    )
  }
  const pauseClock = () => {
    if (clock.since === null) return
    clock.visibleMs += deps.now() - clock.since
    clock.since = null
    clearTimeout(clock.timer)
  }

  // The hidden-tab title count (C4): React owns the title while a chat page renders <title>.
  let titleOwners = 0
  let written: string | null = null
  let base = ''
  const restoreTitle = () => {
    if (doc && written !== null && doc.title === written) doc.title = base
    written = null
  }
  function applyTitle() {
    if (!doc) return
    const n = visible() ? 0 : titleCount()
    if (titleOwners > 0 || n === 0) {
      if (titleOwners === 0) restoreTitle()
      else written = null
      return
    }
    // Someone else changed the title while we held it: theirs is the new base (never overwritten back).
    if (written === null || doc.title !== written) base = doc.title.replace(PREFIX, '')
    written = `(${n}) ${base}`
    doc.title = written
  }

  const onVisibility = () => {
    if (visible()) {
      if (open.id) look(open.id)
      runClock()
    } else pauseClock()
    emit()
  }
  doc?.addEventListener('visibilitychange', onVisibility)

  function unseenReplies(): Completion[] {
    return [...completions.values()]
      .filter((c) => c.kind === 'reply' && isUnseen(c))
      .sort((a, b) => b.finishedAt - a.finishedAt)
  }

  function titleCount(): number {
    return unseenReplies().length
  }

  function unseenChats(): number {
    return new Set(unseenReplies().map((c) => c.sessionId)).size
  }

  function row(sessionId: string): RowSignal {
    let latest: Completion | undefined
    for (const c of completions.values())
      if (c.sessionId === sessionId && (!latest || c.finishedAt >= latest.finishedAt)) latest = c
    return {
      live: liveKey.split('|').includes(sessionId),
      failed: !!latest && latest.kind === 'error' && isUnseen(latest),
      unseen: !!latest && latest.kind === 'reply' && isUnseen(latest),
    }
  }

  function replyReady(): Completion | null {
    const c = unseenReplies()[0]
    if (!c || c.sessionId === open.id || dismissed.has(c.attempt) || expired.has(c.attempt))
      return null
    return c
  }

  function titlePrefix(): string {
    const n = visible() ? 0 : titleCount()
    return n > 0 ? `(${n}) ` : ''
  }

  function snapshot(): SignalsSnapshot {
    const rows = new Map<string, RowSignal>()
    const ids = new Set([...completions.values()].map((c) => c.sessionId))
    for (const id of liveKey.split('|')) if (id) ids.add(id)
    for (const id of ids) rows.set(id, row(id))
    return {
      rows,
      unseenChats: unseenChats(),
      replyReady: replyReady(),
      titlePrefix: titlePrefix(),
      seen: new Map(seen),
      completions: [...completions.values()],
    }
  }

  // Built last: the snapshot reads every piece of state above.
  const store = createStore<SignalsSnapshot>(() => snapshot())
  const signals = {
    /** Renders read this (a selector over its snapshot); the methods below are for effects, handlers and tests. */
    store,

    /** A chat page mounted (E10). Returns its token; only that token clears it, so /chat/a → /chat/b never clears b. */
    setOpenChat(sessionId: string): number {
      open = { id: sessionId, token: open.token + 1 }
      look(sessionId)
      emit()
      return open.token
    },
    releaseOpenChat(token: number) {
      if (open.token !== token) return
      open = { id: null, token }
      emit()
    },
    openChat: () => open.id,

    /** One rail row's marks (§5.8): the live spinner, a failure and an unseen reply, from its newest attempt. */
    row,

    /** Chats with an unseen reply (the phone trigger's dot and name). */
    unseenChats,

    /** "Reply ready": the newest unseen reply, not dismissed or timed out, and never for the open chat. */
    replyReady,
    /** The line is on screen: start (or keep) its visible-time clock. */
    replyReadyShown(attempt: string) {
      if (clock.attempt !== attempt) {
        pauseClock()
        clearTimeout(clock.timer)
        clock = { attempt, visibleMs: 0, since: null }
      }
      runClock()
    },
    /** Announce a finish once (DS13). Returns whether this call should speak. */
    firstNotice(attempt: string): boolean {
      if (notified.has(`${attempt}:reply`)) return false
      notified.add(`${attempt}:reply`)
      return true
    },
    /** Dismiss hides the line for every reply unseen now; their dots stay. */
    dismissReplyReady() {
      for (const c of unseenReplies()) dismissed.add(c.attempt)
      emit()
    },

    /** The title prefix a chat page renders (C4): "(N) " while the tab is hidden and N > 0. */
    titlePrefix,
    /** A chat page renders <title> itself while mounted. */
    ownTitle(): () => void {
      titleOwners++
      restoreTitle()
      return () => {
        titleOwners = Math.max(0, titleOwners - 1)
        applyTitle()
      }
    },

    dispose() {
      unsubscribeEnds()
      unsubscribeTurns()
      doc?.removeEventListener('visibilitychange', onVisibility)
      clearTimeout(clock.timer)
      restoreTitle()
    },
  }
  return signals
}

export type Signals = ReturnType<typeof createSignals>
