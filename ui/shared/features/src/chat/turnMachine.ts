/**
 * The turn registry's types and pure parts (plan §6.1-§6.2, v1b §5.4): phases, the routed per-attempt state
 * table (`stepRouted`), settle rules and the read-side helpers renders use over the store's snapshot.
 * `turnRegistry.ts` re-exports all of it; import from there.
 */
import type { Step, TurnState } from './a2aReducer'
import type { ChatError } from './errors'
import type { ChatMessage, ChatSessionRow, SaveMessageBody, TranscriptOwnership } from './types'

export type Phase =
  | 'creating'
  | 'saving_user'
  | 'waiting'
  | 'streaming'
  | 'done'
  | 'paused'
  | 'no_reply'
  | 'error'
  | 'unsaved'
  | 'aborted'
  // Routed turns only (v1b §5.4). `done` is "settling": the reply is complete, history not yet caught up.
  | 'draining'
  | 'known_empty'
  | 'lost'
  | 'loading_saved'
  | 'history_failed'
  | 'not_started'
  | 'resume_uncertain'
  | 'resume_forbidden'

const LIVE: ReadonlySet<Phase> = new Set([
  'creating',
  'saving_user',
  'waiting',
  'streaming',
  'draining',
])
export const isLivePhase = (p: Phase) => LIVE.has(p)

export type ChatMode = 'direct' | 'routed'
type Operation = 'send' | 'rerun' | 'resume'

export interface LiveTurn {
  id: string
  sessionId: string
  /** The direct chat's agent; null on routed turns. */
  agentId: string | null
  userText: string
  userMessageId?: string
  /**
   * The server writes this chat's user row (its transcript says so), so the client never learns its id: the
   * row exists once the stream starts, and the attempt is keyed by the turn id (v1c E12).
   */
  userRowServer?: boolean
  phase: Phase
  state: TurnState
  startedAt: number
  /** No frame for STREAM_IDLE_MS; the read continues. */
  idle: boolean
  /** The user pressed Stop receiving. */
  stopped: boolean
  finalized: boolean
  error: ChatError | null
  saved: ChatMessage | null
  /** Unsaved assistant body, kept for Save again. */
  pendingSave: SaveMessageBody | null
  /** Last raw frames, for `?debug=turn`. */
  frames: string[]
  /** The request this turn resumed, once per id (EN9). */
  resumedRequestId?: string
  /** Who the chat talks to (EN-3). */
  chatMode: ChatMode
  /** What started this attempt (EN-3). A `resume` reconnects after a request was answered; the server saves that reply (§3.3). */
  operation: Operation
  /** This attempt's label within its user message: `send`, `rerun:<n>` or `resume:<hitl id>` (§5.4). */
  attempt: string
  /** The stream was cut short on purpose (drain, ceiling, truncation): its text is partial, never "no reply". */
  incomplete?: boolean
  /**
   * A resume's baseline: the replies its turn had when the request was answered, before the
   * resolve's refetch could already show the continuation's saved reply (settle without a trace id).
   */
  baselineReplies?: number
}

/** The turn-end store key: user-message id plus the attempt label (§5.4). */
export const attemptKeyOf = (t: Pick<LiveTurn, 'id' | 'userMessageId' | 'attempt'>) =>
  `${t.userMessageId ?? t.id}:${t.attempt}`

/** One attempt's end, written once at its terminal state (NE-4). Outcome views read from here. */
export interface TurnEnd {
  id: string
  sessionId: string
  attemptKey: string
  userMessageId?: string
  traceId: string | null
  kind: 'reply' | 'empty' | 'error' | 'paused' | 'not_started'
  /** The stream reached its terminal (completed or failed): this attempt sends nothing more, even if `kind` is a later error. */
  terminal?: boolean
  finishedAt: number
  /** Who answered (v1c §5.8): direct ends are recorded apart from routed ones and never call `onTurnEnd`. */
  chatMode?: ChatMode
}

/** An attempt key without the suffix a superseding end carries (`<key>:reply`, `<key>:error`, v1c §5.8). */
export const baseAttemptKey = (key: string) => key.replace(/:(reply|error)$/, '')

/**
 * The attempt's stream reached its terminal state: nothing more will arrive on it. A resume
 * replays the sub-agent's own stream (its COMPLETED included) before the orchestrator's turn,
 * which starts with trace_meta (hitl/mod.rs): only a terminal after that counts.
 */
export const sawTerminal = (
  s: Pick<TurnState, 'taskState' | 'replyArtifactsFrom'>,
  resume: boolean,
) =>
  (s.taskState === 'completed' || s.taskState === 'failed') &&
  (!resume || s.replyArtifactsFrom !== null)

/** Events of one routed attempt (v1b §5.4). */
export type RoutedEvent =
  | { type: 'create_failed' }
  | { type: 'dispatch_rejected'; resume: boolean; status: number }
  | { type: 'dispatch_unknown'; resume: boolean }
  | { type: 'frame' }
  | { type: 'limit'; resume: boolean }
  | { type: 'terminal'; text: boolean }
  | { type: 'task_failed' }
  | { type: 'pause' }
  | { type: 'truncated' }
  /** EOF without a terminal frame, or a stream exception. */
  | { type: 'eof'; resume: boolean }
  | { type: 'ceiling' }
  /** `no_match`: history loaded without the reply yet; `timeout`: the page's bounded re-checks gave up. */
  | { type: 'history'; result: 'match' | 'no_match' | 'timeout' | 'error'; hadText: boolean }
  | { type: 'abort' }

/**
 * The routed per-attempt state table (v1b §5.4). Pure; `'settled'` means history holds the
 * reply and the registry forgets the live block. Anything not listed leaves the phase as it is.
 */
export function stepRouted(phase: Phase, ev: RoutedEvent): Phase | 'settled' {
  if (ev.type === 'abort') return isLivePhase(phase) || phase === 'paused' ? 'aborted' : phase
  switch (phase) {
    case 'creating':
    case 'saving_user':
      return ev.type === 'create_failed' ? 'not_started' : phase
    case 'waiting':
      if (ev.type === 'dispatch_rejected')
        return ev.resume
          ? ev.status === 403
            ? 'resume_forbidden'
            : 'resume_uncertain'
          : 'not_started'
      // A resume never re-runs anything: the continuation already ran server-side, so it's uncertain, never lost.
      if (ev.type === 'dispatch_unknown') return ev.resume ? 'resume_uncertain' : 'lost'
      if (ev.type === 'frame') return 'streaming'
      // A reconnect after the buffer expired gets an empty 200 stream (continuation.rs `watch`):
      // the resumed reply may still be saved, so it's uncertain, never lost.
      if (ev.type === 'eof') return ev.resume ? 'resume_uncertain' : 'lost'
      return phase
    case 'streaming':
      switch (ev.type) {
        case 'limit':
          return ev.resume ? 'loading_saved' : 'draining'
        case 'terminal':
          return ev.text ? 'done' : 'known_empty'
        case 'task_failed':
          return 'error'
        case 'pause':
          return 'paused'
        case 'truncated':
          return 'loading_saved'
        case 'eof':
          return ev.resume ? 'resume_uncertain' : 'lost'
        default:
          return phase
      }
    case 'draining':
      return ev.type === 'terminal' ||
        ev.type === 'task_failed' ||
        ev.type === 'pause' ||
        ev.type === 'eof' ||
        ev.type === 'ceiling'
        ? 'loading_saved'
        : phase
    case 'done':
      // A complete reply the server hasn't saved (yet): unconfirmed once the re-checks give up.
      if (ev.type !== 'history') return phase
      return ev.result === 'match'
        ? 'settled'
        : ev.result === 'error'
          ? 'history_failed'
          : ev.result === 'timeout'
            ? 'lost'
            : 'done'
    case 'loading_saved':
      if (ev.type !== 'history') return phase
      if (ev.result === 'match') return 'settled'
      if (ev.result === 'error') return 'history_failed'
      return ev.result === 'timeout' ? (ev.hadText ? 'lost' : 'known_empty') : 'loading_saved'
    case 'history_failed':
      if (ev.type !== 'history') return phase
      if (ev.result === 'match') return 'settled'
      if (ev.result === 'error') return 'history_failed'
      return ev.result === 'timeout' ? 'lost' : 'loading_saved'
    case 'known_empty':
    case 'lost':
    case 'error':
    case 'resume_uncertain':
    case 'resume_forbidden':
      // A reply that shows up later still settles the turn.
      return ev.type === 'history' && ev.result === 'match' ? 'settled' : phase
    default:
      return phase
  }
}

/** Terminal routed phases and the turn-end kind each records. */
export const END_KIND: Partial<Record<Phase, TurnEnd['kind']>> = {
  done: 'reply',
  known_empty: 'empty',
  paused: 'paused',
  not_started: 'not_started',
  lost: 'error',
  error: 'error',
  resume_uncertain: 'error',
  resume_forbidden: 'error',
}

/** Phases where a Run again may repeat work the server already did (DS13 confirm). */
// Not known_empty: that turn finished (with no text), so a new send can't collide with it; its own Run again still asks first.
export const UNCERTAIN: ReadonlySet<Phase> = new Set([
  'lost',
  'error',
  'resume_uncertain',
  'history_failed',
])

/**
 * Whether history holds this routed attempt's reply (EN-2, NE-2): the saved row whose `trace_id`
 * is the attempt's `trace_meta` id; with no trace id, any reply newer than the attempt's baseline.
 */
export function routedSettles(
  t: Pick<LiveTurn, 'state'>,
  repliesAfterUser: readonly Pick<ChatMessage, 'role' | 'trace_id'>[],
  newReplies: number,
): boolean {
  const traceId = t.state.traceId
  if (traceId) return repliesAfterUser.some((m) => m.role === 'assistant' && m.trace_id === traceId)
  return newReplies > 0
}

/**
 * What renders read (`useLiveTurn`, `endFor`, `stepsFor` over a snapshot): immutable, replaced as a whole on
 * every change. The registry's own working maps are never read during render.
 */
export interface RegistryState {
  turns: ReadonlyMap<string, LiveTurn>
  /** Every chat's turn, as one array that keeps its identity until the next change (EN-3b). */
  list: readonly LiveTurn[]
  /** Routed turn ends by attempt key, oldest first (NE-4). */
  ends: ReadonlyMap<string, TurnEnd>
  /** Tool steps of replies this tab saved, by message id or `trace:<id>` (see `savedSteps`). */
  steps: ReadonlyMap<string, Step[]>
}

/** The newest recorded routed end for one user message (EN-5 outcomes read this). */
export function endOf(
  ends: RegistryState['ends'],
  sessionId: string,
  userMessageId: string,
): TurnEnd | undefined {
  let hit: TurnEnd | undefined
  for (const e of ends.values())
    if (
      e.sessionId === sessionId &&
      e.userMessageId === userMessageId &&
      (!hit || e.finishedAt >= hit.finishedAt)
    )
      hit = e
  return hit
}

/** Steps recorded for a reply this tab saved, or a routed reply by its trace id (R7). */
export const stepsOf = (
  steps: RegistryState['steps'],
  messageId: string,
  traceId?: string | null,
): Step[] | undefined =>
  steps.get(messageId) ?? (traceId ? steps.get(`trace:${traceId}`) : undefined)

export interface ChatDeps {
  userId: string
  /** No `agent_id` on a routed create (§2.2). */
  createSession(
    body: { session_id: string; agent_id?: string; first_prompt: string },
    signal: AbortSignal,
  ): Promise<ChatSessionRow>
  saveMessage(sessionId: string, body: SaveMessageBody): Promise<ChatMessage>
  dispatch(body: unknown, signal: AbortSignal): Promise<Response>
  /** Refetch the newest history page; true when an assistant row with this content follows the user row. */
  replyExists(sessionId: string, userMessageId: string, content: string): Promise<boolean>
  onUnauthorized(): void
  newId(): string
  now(): number
  locks?: Pick<LockManager, 'request'> | null
  warn?(message: string, context: Record<string, unknown>): void
  /** A routed attempt reached its end: the one place its queries are invalidated (NE-4). */
  onTurnEnd?(end: TurnEnd): void
  /** A direct reply's save settled (v1c E4): persistence refreshes apart from notification. */
  onDirectSaved?(sessionId: string): void
}

interface SendCommon {
  /** Omit for a new chat; the registry mints the id (EN8). */
  sessionId?: string
  text: string
  transcript?: TranscriptOwnership
  /** Called once the chat exists (new chats), before the user row is saved. */
  onCreated?(row: ChatSessionRow): void
  /** Create the chat even though `sessionId` is set: Try again after a create failure reuses the id (EN8). */
  create?: boolean
  /**
   * Called when creating the chat fails, whether or not the sending view is still mounted, so the
   * placeholder row and the draft are put right even after the user navigated away.
   */
  onCreateFailed?(sessionId: string, text: string): void
  /** Called synchronously once the turn exists, before create (placeholder rail row, EN8). */
  onStart?(sessionId: string): void
}

/** A direct send names its agent; a routed one never does (DX-1, EN-3). */
export type SendInput = SendCommon &
  ({ chatMode?: 'direct'; agentId: string } | { chatMode: 'routed'; agentId?: never })

/** A history turn with no reply and no live turn in this tab (after a reload), for Run again. */
export interface RerunFrom {
  /** Null on a routed chat. */
  agentId: string | null
  chatMode?: ChatMode
  userText: string
  userMessageId: string
}
