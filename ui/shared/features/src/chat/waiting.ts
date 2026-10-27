/**
 * The Waiting queue's state for the chat layout (v1c §5.9): the poll, matched against the loaded rail rows
 * and the request index. A result fetched under an older sign-in generation is ignored (E2).
 */
import { useEffect, useMemo, useSyncExternalStore } from 'react'
import { useStore } from 'zustand'
import { createStore } from 'zustand/vanilla'
import { signInGeneration } from '@nasiko/app-core/session'
import { sessionRows, useChatSessions, usePendingRequests } from './api'
import { hitlIndexStore, matchPending, selectHitlIndex, type PendingMatch } from './pending'
import type { HitlDto } from './types'

export interface Waiting {
  match: PendingMatch
  /** No answer yet. */
  isPending: boolean
  /** The first poll failed and nothing is known: the count-less Waiting option (E14) and the error state. */
  failedCold: boolean
  /** A poll failed after a success: the last good rows stay, with "Last checked" (DP3). */
  stale: boolean
  lastChecked: number
  error: unknown
  retry(): void
  /** Waiting requests per chat, for the rail row pill. */
  countFor(sessionId: string): number
}

const EMPTY: PendingMatch = { chats: [], outside: 0, dropped: 0 }

/**
 * The last match per user, for `?debug=turn` only (DX9). Only the debug line subscribes, so a poll re-renders that
 * line and never the chat.
 */
const debugStore = createStore<{ byUser: ReadonlyMap<string, PendingMatch> }>(() => ({
  byUser: new Map(),
}))

/** Cleared with the chat registry. */
export const clearWaitingDebug = () => debugStore.setState({ byUser: new Map() })

/** `?debug=turn` (DX9): how this chat's pending requests were matched, or null when none are. */
export function useWaitingDebug(
  userId: string,
  sessionId: string,
): { source: string; requests: string[] } | null {
  const match = useStore(debugStore, (s) => s.byUser.get(userId))
  const c = match?.chats.find((x) => x.sessionId === sessionId)
  return c ? { source: c.source, requests: c.requests.map((r) => r.id) } : null
}

/** Another tab's sign-in changes the generation in storage; read it as a subscribed value, never mid-render. */
const onStorage = (l: () => void) => {
  window.addEventListener('storage', l)
  return () => window.removeEventListener('storage', l)
}

export function useWaiting(userId: string, isSuperuser: boolean): Waiting {
  const q = usePendingRequests(userId)
  const list = useChatSessions()
  const index = useStore(hitlIndexStore, selectHitlIndex(userId))
  const generation = useSyncExternalStore(onStorage, signInGeneration)
  const data = q.data && q.data.generation === generation ? q.data : undefined
  const match = useMemo(() => {
    if (!data) return EMPTY
    const loaded = new Set(sessionRows(list.data).map((r) => r.session_id))
    return matchPending(data.items, loaded, index, isSuperuser)
  }, [data, list.data, index, isSuperuser])
  const counts = useMemo(
    () => new Map(match.chats.map((c) => [c.sessionId, c.requests.length])),
    [match],
  )
  useEffect(() => {
    debugStore.setState((s) => ({ byUser: new Map(s.byUser).set(userId, match) }))
  }, [userId, match])
  return {
    match,
    isPending: q.isPending,
    failedCold: q.isError && !data,
    stale: !!data && q.errorUpdatedAt > q.dataUpdatedAt,
    lastChecked: q.dataUpdatedAt,
    error: q.error,
    retry: () => void q.refetch(),
    countFor: (sessionId) => counts.get(sessionId) ?? 0,
  }
}

/** A pending request the viewer can count against an agent (the home's per-agent counts, the hub's Needs you). */
export interface PendingRequestRow {
  id: string
  agentId: string | null
  /** The chat it was matched to, when one was. */
  chat: string | null
  /** It counts as the viewer's: always for a member; for a superuser only once a chat proves it (ISSUE-007). */
  proven: boolean
}

/**
 * Every pending request with its agent and whether it counts as the viewer's. `GET /api/hitl/pending` answers a member
 * with their own requests only; a superuser gets every user's with no owner field, so theirs are only those a loaded
 * chat (or the request index) proves theirs (chat's Waiting rule, `matchPending`).
 */
export function pendingRequestRows(
  superuser: boolean,
  match: PendingMatch,
  items: readonly HitlDto[] | undefined,
): PendingRequestRow[] {
  const claimed = match.chats.flatMap((c) => c.requests.map((r) => [r.id, c.sessionId] as const))
  const chatOf = new Map(claimed)
  // The match's order (oldest first), so a caller's first linked row is the oldest request, whatever the server's order.
  const rank = new Map(claimed.map(([id], i) => [id, i]))
  return (items ?? [])
    .filter((r) => r.status === 'pending')
    .map((r) => {
      const chat = chatOf.get(r.id) ?? null
      return { id: r.id, agentId: r.execution.agent_id, chat, proven: !superuser || chat !== null }
    })
    .sort((a, b) => (rank.get(a.id) ?? Infinity) - (rank.get(b.id) ?? Infinity))
}
