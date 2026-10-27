/**
 * The Waiting queue's matching (v1c §5.9, T2, ND-6, EN10, E13): which of `GET /api/hitl/pending`'s rows
 * belong to which chat, and which of them this viewer may see. Pure, plus the per-user id → chat index.
 * - A pending row finds its chat by the EN10 index (a request id seen in a loaded history's `hitl[]` or a
 *   live `hitl` frame: the only way to place an `mcp_tool` row), then `execution.chat_session_id`, then
 *   `execution.context_id` for direct_chat and agent_proxy origins (a routed row's context is the sub-agent's).
 * - A `context_id` is only a chat id for a direct_chat row whose chat the rail has loaded: an agent_proxy row's
 *   context comes from the outside A2A caller, and an unloaded one could open a chat that isn't there (/review D2).
 * - A normal user's rows are all theirs (§2.1): a match is a Waiting row, the rest count as outside Chat.
 * - A superuser gets every user's rows with no owner field, so only provable matches show: an index match
 *   (the index comes from their own history and streams), or a `chat_session_id` / `context_id` match against
 *   a loaded rail row (the list is caller-scoped). Everything else is dropped, uncounted.
 */
import { createStore } from 'zustand/vanilla'
import { tuning } from './tuning'
import type { HitlDto } from './types'

type MatchSource = 'index' | 'chat_session_id' | 'context_id'

interface WaitingChat {
  sessionId: string
  /** Oldest first. */
  requests: HitlDto[]
  source: MatchSource
}

export interface PendingMatch {
  /** One row per chat, the chat with the oldest pending request first. */
  chats: WaitingChat[]
  /** A normal user's requests no chat claims (workflow and tool requests, mostly). */
  outside: number
  /** A superuser's requests that can't be proven theirs until the server reports owners (R-1). */
  dropped: number
}

export function matchPending(
  items: readonly HitlDto[],
  loadedChatIds: ReadonlySet<string>,
  index: ReadonlyMap<string, string>,
  isSuperuser: boolean,
): PendingMatch {
  const by = new Map<string, WaitingChat>()
  let outside = 0
  let dropped = 0
  for (const r of items) {
    if (r.status !== 'pending') continue
    const e = r.execution
    let sessionId: string | undefined
    let source: MatchSource | undefined
    if (index.has(r.id)) [sessionId, source] = [index.get(r.id), 'index']
    else if (e.chat_session_id) [sessionId, source] = [e.chat_session_id, 'chat_session_id']
    else if (e.context_id && e.origin === 'direct_chat' && loadedChatIds.has(e.context_id))
      [sessionId, source] = [e.context_id, 'context_id']
    const provable = source === 'index' || (!!sessionId && loadedChatIds.has(sessionId))
    if (isSuperuser && !provable) {
      dropped++
      continue
    }
    if (!sessionId || !source) {
      outside++
      continue
    }
    const chat = by.get(sessionId) ?? { sessionId, requests: [], source }
    chat.requests.push(r)
    by.set(sessionId, chat)
  }
  const chats = [...by.values()]
  for (const c of chats) c.requests.sort((a, b) => a.created_at.localeCompare(b.created_at))
  chats.sort((a, b) => a.requests[0].created_at.localeCompare(b.requests[0].created_at))
  return { chats, outside, dropped }
}

/** A request's text as one plain line (a Waiting row's preview): no Markdown, no HTML, never rendered as either. */
export function requestPreview(r: Pick<HitlDto, 'question'>, max = 120): string {
  const text = r.question?.message ?? ''
  return text
    .replace(/<[^>]*>/g, '')
    .replace(/[`*_#>~[\]]+/g, '')
    .replace(/\s+/g, ' ')
    .trim()
    .slice(0, max)
}

// ─── The EN10 index: request id → chat, per user, in memory ─────────────────

interface IndexState {
  user: string | null
  /** Immutable: replaced on every change, so a render reads it as a value (never a mutated Map). */
  index: ReadonlyMap<string, string>
}
const NO_INDEX: ReadonlyMap<string, string> = new Map()
export const hitlIndexStore = createStore<IndexState>(() => ({ user: null, index: NO_INDEX }))

/** Record where requests live (a loaded history's `hitl[]`, a live `hitl` frame). */
export function indexRequests(userId: string, sessionId: string, ids: readonly string[]) {
  const prev = hitlIndexStore.getState()
  const index = new Map(prev.user === userId ? prev.index : NO_INDEX)
  let changed = false
  for (const id of ids) {
    if (index.get(id) === sessionId) continue
    index.delete(id)
    index.set(id, sessionId)
    changed = true
  }
  // Newest last; a long sign-in keeps the most recent HITL_INDEX_MAX ids.
  while (index.size > tuning.HITL_INDEX_MAX) {
    const oldest = index.keys().next().value
    if (oldest === undefined) break
    index.delete(oldest)
  }
  if (changed || prev.user !== userId) hitlIndexStore.setState({ user: userId, index })
}

/** This user's index (a stable value until it changes); empty for anyone else. */
export const selectHitlIndex =
  (userId: string) =>
  (s: IndexState): ReadonlyMap<string, string> =>
    s.user === userId ? s.index : NO_INDEX

/** Cleared with the chat registry (sign out, user switch). */
export function clearHitlIndex() {
  hitlIndexStore.setState({ user: null, index: NO_INDEX })
}
