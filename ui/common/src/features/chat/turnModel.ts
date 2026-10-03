/**
 * History pages + the live registry turn → the turns the transcript renders (plan §5, EN14).
 *
 *   pages (newest first, from useInfiniteQuery) ─► dedupe by id ─► sort by time ─► group
 *     each user row opens a turn; assistant rows attach to the turn before them;
 *     rows before the first user row (a page that starts mid-turn) form a boundary turn;
 *     requests attach to the turn whose user row is the latest one at or before them.
 *   live turn ─► attached to its user row once history has it; dropped once history holds
 *     its saved reply (persisted wins); otherwise appended as a pending turn.
 */
import { tuning } from './tuning'
import type { ChatMessage, HitlDto, MessagesPage } from './types'

export interface LiveRef {
  /** Set once the user row is saved. */
  userMessageId?: string
  userText: string
  finalized: boolean
  /** Routed turns stay attached until the registry settles them on their own reply (v1b §5.4). */
  chatMode?: 'direct' | 'routed'
}

export interface DisplayTurn<L extends LiveRef = LiveRef> {
  key: string
  user: ChatMessage | null
  replies: ChatMessage[]
  requests: HitlDto[]
  live?: L
  /** Orphan rows from before the oldest loaded user message ("Earlier messages"). */
  boundary?: true
}

/**
 * Server timestamps carry 0-9 fractional digits (`…:00Z`, `…:00.5Z`, `…:00.123456Z`), which
 * don't sort as strings; compare instants, then ids.
 */
const at = (iso: string) => Date.parse(iso) || 0
const byTime = (a: ChatMessage, b: ChatMessage) =>
  at(a.timestamp) - at(b.timestamp) || a.id.localeCompare(b.id)

export function mergeTurns<L extends LiveRef>(
  pages: readonly MessagesPage[],
  live?: L,
): DisplayTurn<L>[] {
  const seen = new Map<string, ChatMessage>()
  const requests = new Map<string, HitlDto>()
  for (const p of pages) {
    for (const m of p.data) if (m.role !== 'system') seen.set(m.id, m)
    for (const h of p.hitl) requests.set(h.id, h)
  }
  const rows = [...seen.values()].sort(byTime)
  const turns: DisplayTurn<L>[] = []
  for (const m of rows) {
    if (m.role === 'user') turns.push({ key: m.id, user: m, replies: [], requests: [] })
    else if (turns.length) turns[turns.length - 1].replies.push(m)
    else
      turns.push({
        key: `boundary-${m.id}`,
        user: null,
        replies: [m],
        requests: [],
        boundary: true,
      })
  }
  const reqs = [...requests.values()].sort((a, b) => at(a.created_at) - at(b.created_at))
  for (const r of reqs) {
    let target: DisplayTurn<L> | undefined
    for (const t of turns) if (t.user && at(t.user.timestamp) <= at(r.created_at)) target = t
    ;(target ?? turns[turns.length - 1])?.requests.push(r)
  }
  if (!live) return turns
  const own = live.userMessageId ? turns.find((t) => t.user?.id === live.userMessageId) : undefined
  if (own) {
    // A routed attempt's own reply is matched by trace id, not by "any reply" (a rerun or resume
    // has older replies): the registry forgets it once settled, so keep it until then.
    if (live.chatMode === 'routed' || !(live.finalized && own.replies.length)) own.live = live
    return turns
  }
  turns.push({
    key: `live-${live.userMessageId ?? 'pending'}`,
    user: null,
    replies: [],
    requests: [],
    live,
  })
  return turns
}

export type ReplyStatus = 'checking' | 'unconfirmed' | null

/**
 * Whether the newest turn is still waiting for a reply that nothing in this tab is producing
 * (plan §6.7). `checking` inside the window, `unconfirmed` after it, null when answered,
 * live, paused on a pending request, or closed by a canceled/expired request.
 */
// quirk: §10.1 — a reply the client failed to save is gone; this note is how the user finds out.
export function replyStatus(
  turns: readonly DisplayTurn[],
  now: number,
  windowMs: number,
): ReplyStatus {
  const last = turns[turns.length - 1]
  if (!last?.user || last.live || last.replies.length) return null
  const { user } = last
  const after = last.requests.filter((r) => at(r.created_at) >= at(user.timestamp))
  if (after.some((r) => r.status === 'pending')) return null
  const newest = after[after.length - 1]
  if (newest && (newest.status === 'canceled' || newest.status === 'expired')) return null
  // The server gave up resuming it (hitl resume_status): no reply is coming, so no checking wait.
  if (newest && (newest.resume_status === 'failed' || newest.resume_status === 'skipped'))
    return 'unconfirmed'
  // A resolved (or rejected) request resumes the agent and the server saves its reply;
  // the clock starts at resolution.
  const since = Date.parse(newest?.resolved_at ?? last.user.timestamp)
  return now - since < windowMs ? 'checking' : 'unconfirmed'
}

/**
 * A routed turn whose request was answered, with no reply saved after it (ship review D3). The
 * server's continuation may still save one: delivery retries keep `resume_status` at
 * `not_started` (hitl/mod.rs MAX_RESUME_ATTEMPTS, each up to the 300 s agent timeout), and a
 * delivered answer then runs the orchestrator's turn. Recovery is Refresh status only: Run again
 * would repeat the approved step. `mayArrive` holds the composer while a reply can still land.
 *
 * `seenEnded`: this tab saw the attempt end with nothing to save, so nothing is coming.
 * `firstSeen(id, now)`: when this tab first saw the answer, so a client clock behind the
 * server's `resolved_at` can't stretch the wait.
 */
export function answeredResume(
  turn: DisplayTurn | undefined,
  now: number,
  opts: { seenEnded: boolean; firstSeen(id: string, now: number): number },
): { mayArrive: boolean } | null {
  if (!turn?.user || turn.replies.length || turn.requests.some((r) => r.status === 'pending'))
    return null
  const answered = turn.requests.filter((r) => r.status === 'resolved' || r.status === 'rejected')
  if (!answered.length) return null
  // Only the newest answer can still reply: an older one's continuation already ran (it asked again).
  const r = answered.reduce((a, b) => (at(b.created_at) >= at(a.created_at) ? b : a))
  // `completed` is set when delivery succeeds, anywhere in the retry window, before the
  // orchestrator's turn runs: it gets the whole window too (hitl/mod.rs, cb3aaf0c).
  const wait =
    r.resume_status === 'not_started' || r.resume_status === 'completed'
      ? tuning.RESUME_DELIVERY_MS
      : r.resume_status === 'delivery_outcome_unknown'
        ? tuning.LOST_REPLY_AFTER_MS
        : 0
  const resolved = Date.parse(r.resolved_at ?? '')
  const mayArrive =
    !opts.seenEnded &&
    wait > 0 &&
    !Number.isNaN(resolved) &&
    now - Math.min(resolved, opts.firstSeen(r.id, now)) < wait
  return { mayArrive }
}

/** When this tab first saw each routed request answered (D3): bounds the wait under clock skew. */
const answersSeen = new Map<string, number>()
const ANSWERS_SEEN_MAX = 100
export function firstSeenAnswer(id: string, now: number): number {
  const known = answersSeen.get(id)
  if (known !== undefined) return known
  answersSeen.set(id, now)
  if (answersSeen.size > ANSWERS_SEEN_MAX) {
    const oldest = answersSeen.keys().next().value
    if (oldest !== undefined) answersSeen.delete(oldest)
  }
  return now
}

/** User switch and tests: forget what this tab saw. */
export function clearAnswersSeen() {
  answersSeen.clear()
}
