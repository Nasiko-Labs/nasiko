/**
 * The Waiting queue's state for the chat layout (v1c §5.9): the poll, matched against the loaded rail rows
 * and the request index. A result fetched under an older sign-in generation is ignored (E2).
 */
import { useEffect, useMemo, useSyncExternalStore } from 'react'
import { useStore } from 'zustand'
import { createStore } from 'zustand/vanilla'
import { signInGeneration } from '@/lib/session'
import { sessionRows, useChatSessions, usePendingRequests } from './api'
import { hitlIndexStore, matchPending, selectHitlIndex, type PendingMatch } from './pending'

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
