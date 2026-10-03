/**
 * Chat queries and mutations (plan §5). Facts that shape them:
 * - Lists and history are keyset-paged; history's first page is the newest, ascending, and
 *   older pages come from `prev_cursor` (chat/routes.rs:611).
 * - The app's global `refetchOnWindowFocus` is off; chat queries opt in (EN21).
 * - Responses mix envelopes and bare bodies; normalize.ts owns that.
 */
import {
  infiniteQueryOptions,
  useInfiniteQuery,
  useMutation,
  useQuery,
  useQueryClient,
  type InfiniteData,
  type QueryClient,
} from '@tanstack/react-query'
import { createLimiter } from '@/features/observability/limiter'
import { ApiError, apiFetch, withQuery } from '@/lib/api/client'
import { flowAgents, type FlowDetail } from './activity'
import { unwrapMessagesPage, unwrapPending, unwrapSession, unwrapSessionsPage } from './normalize'
import { chatKeys } from './keys'
import { forgetSessionLookup } from './sessionLookup'
import { tuning } from './tuning'
import { signInGeneration } from '@/lib/session'
import type {
  ChatMessage,
  ChatSessionRow,
  CursorPage,
  HitlDto,
  MessagesPage,
  SaveMessageBody,
} from './types'

const SESSIONS = '/api/chat/sessions'
const LIST_PAGE = 50
const HISTORY_PAGE = 100

export { chatKeys }

const sessionsQuery = infiniteQueryOptions({
  queryKey: chatKeys.list,
  queryFn: async ({ pageParam, signal }) =>
    unwrapSessionsPage(
      await apiFetch(withQuery(SESSIONS, { limit: LIST_PAGE, cursor: pageParam ?? undefined }), {
        signal,
      }),
    ),
  initialPageParam: null as string | null,
  getNextPageParam: (last) => (last.has_more ? last.next_cursor : undefined),
  staleTime: 15_000,
  meta: { path: SESSIONS },
})

export function useChatSessions() {
  return useInfiniteQuery({ ...sessionsQuery, refetchOnWindowFocus: true })
}

const historyQuery = (sessionId: string) =>
  infiniteQueryOptions({
    queryKey: chatKeys.history(sessionId),
    queryFn: async ({ pageParam, signal }) =>
      unwrapMessagesPage(
        await apiFetch(
          withQuery(`${SESSIONS}/${encodeURIComponent(sessionId)}/messages`, {
            limit: HISTORY_PAGE,
            prev_cursor: pageParam ?? undefined,
          }),
          { signal },
        ),
      ),
    initialPageParam: null as string | null,
    // "Older" is the next page to load: the server's prev_cursor walks back in time.
    getNextPageParam: (last) => (last.has_more ? last.prev_cursor : undefined),
  })

/** The chat routes' prefetch (plan §8 Phase 3): the rail's first page, and the open chat's newest history page. */
export function prefetchChat(client: QueryClient, sessionId?: string) {
  void client.prefetchInfiniteQuery(sessionsQuery)
  if (sessionId) void client.prefetchInfiniteQuery(historyQuery(sessionId))
}

export function useChatHistory(sessionId: string | undefined) {
  return useInfiniteQuery({
    ...historyQuery(sessionId ?? ''),
    enabled: !!sessionId,
    refetchOnWindowFocus: true,
    // While a request is pending, poll so an expiry turns the card into a receipt (DS4).
    refetchInterval: (query) =>
      query.state.data?.pages.some((p) => p.hitl.some((h) => h.status === 'pending'))
        ? tuning.PENDING_POLL_MS
        : false,
    meta: { path: `${SESSIONS}/:id/messages` },
  })
}

/** All loaded pages, newest page first (the order useInfiniteQuery keeps). */
export const historyPages = (data: InfiniteData<MessagesPage> | undefined): MessagesPage[] =>
  data?.pages ?? []

export function sessionRows(
  data: InfiniteData<CursorPage<ChatSessionRow>> | undefined,
): ChatSessionRow[] {
  const seen = new Set<string>()
  const out: ChatSessionRow[] = []
  for (const p of data?.pages ?? [])
    for (const r of p.data)
      if (!seen.has(r.session_id)) {
        seen.add(r.session_id)
        out.push(r)
      }
  return out
}

/** Put a row at the top of the loaded list (a new chat appears before its first reply, NAS-432). */
export function upsertSessionRow(client: QueryClient, row: ChatSessionRow) {
  client.setQueryData<InfiniteData<CursorPage<ChatSessionRow>>>(chatKeys.list, (prev) => {
    if (!prev)
      return {
        pages: [{ data: [row], has_more: false, next_cursor: null, prev_cursor: null }],
        pageParams: [null],
      }
    const pages = prev.pages.map((p) => ({
      ...p,
      data: p.data.filter((r) => r.session_id !== row.session_id),
    }))
    pages[0] = { ...pages[0], data: [row, ...pages[0].data] }
    return { ...prev, pages }
  })
}

export function removeSessionRow(client: QueryClient, sessionId: string) {
  client.setQueryData<InfiniteData<CursorPage<ChatSessionRow>>>(
    chatKeys.list,
    (prev) =>
      prev && {
        ...prev,
        pages: prev.pages.map((p) => ({
          ...p,
          data: p.data.filter((r) => r.session_id !== sessionId),
        })),
      },
  )
}

export const createSession = async (
  body: { session_id: string; agent_id?: string; first_prompt: string },
  signal?: AbortSignal,
) =>
  unwrapSession(
    await apiFetch(SESSIONS, {
      method: 'POST',
      body: JSON.stringify(body),
      headers: { 'Content-Type': 'application/json' },
      signal,
    }),
  )

export const saveMessage = (sessionId: string, body: SaveMessageBody) =>
  apiFetch<ChatMessage>(`${SESSIONS}/${encodeURIComponent(sessionId)}/messages`, {
    method: 'POST',
    body: JSON.stringify(body),
    headers: { 'Content-Type': 'application/json' },
  })

export function useRenameChat(sessionId: string) {
  const client = useQueryClient()
  return useMutation({
    mutationFn: (title: string) =>
      apiFetch<unknown>(`${SESSIONS}/${encodeURIComponent(sessionId)}`, {
        method: 'PUT',
        body: JSON.stringify({ title }),
        headers: { 'Content-Type': 'application/json' },
      }).then(unwrapSession),
    // PUT returns a bare ChatSession (no agent_name, is_coding_agent or rollups): merge only the
    // title into the list row, or the rail would lose the agent and a recorded chat its badge.
    onSuccess: (row) => {
      const prev = sessionRows(client.getQueryData(chatKeys.list)).find(
        (r) => r.session_id === sessionId,
      )
      upsertSessionRow(
        client,
        prev ? { ...prev, title: row.title, updated_at: row.updated_at ?? prev.updated_at } : row,
      )
    },
  })
}

export function useDeleteChat(sessionId: string) {
  const client = useQueryClient()
  return useMutation({
    mutationFn: () =>
      apiFetch<unknown>(`${SESSIONS}/${encodeURIComponent(sessionId)}`, { method: 'DELETE' }),
    onSuccess: () => {
      removeSessionRow(client, sessionId)
      client.removeQueries({ queryKey: chatKeys.history(sessionId) })
      // A chat found past the rail must not come back from the lookup cache.
      forgetSessionLookup(sessionId)
      client.removeQueries({ queryKey: chatKeys.lookup(sessionId) })
      // Sessions and the trace page stop offering Open chat for it (v1c §5.10).
      client.removeQueries({ queryKey: chatKeys.probe(sessionId) })
    },
  })
}

export type ResolveBody =
  | { answer: string | string[]; custom_answer?: string }
  | { decision: 'approve' | 'reject'; scope?: 'once' | 'session' }
  | { auth_action: 'start' | 'confirm' }

export function useResolveRequest(sessionId: string) {
  const client = useQueryClient()
  return useMutation({
    mutationFn: ({ id, body }: { id: string; body: ResolveBody }) =>
      apiFetch<HitlDto>(`/api/hitl/${encodeURIComponent(id)}/resolve`, {
        method: 'POST',
        body: JSON.stringify(body),
        headers: { 'Content-Type': 'application/json' },
      }),
    // The Waiting queue drops an answered request at once (v1c §5.9).
    onSettled: () =>
      Promise.all([
        client.invalidateQueries({ queryKey: chatKeys.history(sessionId) }),
        client.invalidateQueries({ queryKey: chatKeys.pendingAll }),
      ]),
  })
}

export function useCancelRequest(sessionId: string) {
  const client = useQueryClient()
  return useMutation({
    mutationFn: (id: string) =>
      apiFetch<HitlDto>(`/api/hitl/${encodeURIComponent(id)}/cancel`, { method: 'POST' }),
    onSettled: () =>
      Promise.all([
        client.invalidateQueries({ queryKey: chatKeys.history(sessionId) }),
        client.invalidateQueries({ queryKey: chatKeys.pendingAll }),
      ]),
  })
}

// ─── Routed attribution fallback (v1b §5.8) ───────────────────────────────────

/** At most FLOWS_IN_FLIGHT flows requests at once, however many replies scroll into view (E-A6). */
const flowsLimit = createLimiter(tuning.FLOWS_IN_FLIGHT)

/**
 * The agents a saved routed reply used, from `GET /api/flows/{trace_id}` (names and order only,
 * G-2). Fetched only when `enabled` (the reply is on screen); a flow never changes once done.
 */
export function useFlowAgents(traceId: string | null | undefined, enabled: boolean) {
  const client = useQueryClient()
  return useQuery({
    queryKey: chatKeys.flows(traceId),
    enabled: enabled && !!traceId,
    staleTime: Infinity,
    // A fan-out through flowsLimit: a failing server gets one request per reply, not three.
    retry: false,
    queryFn: async ({ signal }) => {
      // `enabled` needs a trace id, and nothing refetches this query by hand.
      if (!traceId) throw new Error('[chat] flows query ran without a trace id')
      const detail = await flowsLimit(signal, () =>
        apiFetch<FlowDetail>(`/api/flows/${encodeURIComponent(traceId)}`, { signal }),
      )
      const cached = client
        .getQueryCache()
        .findAll({ queryKey: chatKeys.flowsAll })
        .filter((q) => q.state.status === 'success')
      // Oldest first out (NE-14).
      if (cached.length >= tuning.FLOWS_CACHE_MAX) {
        for (const q of cached
          .sort((a, b) => a.state.dataUpdatedAt - b.state.dataUpdatedAt)
          .slice(0, cached.length - tuning.FLOWS_CACHE_MAX + 1))
          client.removeQueries({ queryKey: q.queryKey, exact: true })
      }
      return flowAgents(detail.steps ?? [])
    },
  })
}

/** The one-row read `useChatProbe` makes; Open chat's Copy details quotes it. */
export const probePath = (sessionId: string) =>
  withQuery(`${SESSIONS}/${encodeURIComponent(sessionId)}/messages`, { limit: 1 })

export type ChatProbe =
  | { state: 'probing' }
  | { state: 'yours' }
  | { state: 'absent' }
  | { state: 'error'; error: unknown; retry(): void }

/**
 * Is this Sessions id one of my chats? (v1c §5.10, v1b G-7) A Sessions row id is a chat id (§2.4), and the
 * messages route answers 404 for a missing chat and for another user's (no superuser bypass, §2.2: `chat/routes.rs`
 * `list_messages` checks `user_id` only, cb3aaf0c), so a
 * one-row read tells them apart. Asked only when `enabled`, never for `weave_` ids; cached for the page's
 * life; a 404 is final (the client default never retries a 4xx). The 401 handling is the app's shared one.
 */
export function useChatProbe(sessionId: string, enabled: boolean): ChatProbe {
  const asked = enabled && !sessionId.startsWith('weave_')
  const q = useQuery({
    queryKey: chatKeys.probe(sessionId),
    queryFn: ({ signal }) => apiFetch<unknown>(probePath(sessionId), { signal }).then(() => true),
    enabled: asked,
    staleTime: Infinity,
    meta: { path: `${SESSIONS}/:id/messages` },
  })
  if (!asked) return { state: 'absent' }
  if (q.data) return { state: 'yours' }
  if (q.error instanceof ApiError && q.error.status === 404) return { state: 'absent' }
  if (q.isError) return { state: 'error', error: q.error, retry: () => void q.refetch() }
  return { state: 'probing' }
}

/** Backoff after failed pending polls (v1c §5.9, E6): PENDING_POLL_MS (30 s), then twice it, then four times. */
export const pendingInterval = (failures: number) =>
  tuning.PENDING_POLL_MS * [1, 2, 4][Math.min(failures, 2)]

/**
 * Consecutive failed polls per user. TanStack Query 5 resets `fetchFailureCount` when each fetch starts, so with
 * `retry: false` it never passes 1: the count lives here instead, and a success zeroes it.
 */
const pendingFailures = new Map<string, number>()

/** Cleared with the chat registry (sign out, user switch, test cleanup). */
export const resetPendingBackoff = () => pendingFailures.clear()

/**
 * The Waiting queue's poll (v1c §5.9, NE-13, E6): one query per user, mounted by the chat layout so it runs on
 * every chat route, polling only while the tab is visible. One poll is one request (`retry: false`); after
 * failures the interval backs off (`pendingFailures`), and a success resets it. It also refetches on focus
 * (an opt-in over the app default), after an answer or dismissal, and on a pause frame (the registry wiring).
 * Each result carries the sign-in generation it was fetched under; a stale one is ignored (E2).
 */
export function usePendingRequests(userId: string) {
  return useQuery({
    queryKey: chatKeys.pending(userId),
    queryFn: async ({ signal }) => {
      const generation = signInGeneration()
      try {
        const body = await apiFetch<unknown>('/api/hitl/pending', { signal })
        pendingFailures.set(userId, 0)
        return { items: unwrapPending(body), generation }
      } catch (err) {
        // A poll cancelled by a refresh (resolve, cancel, a new pause) isn't a failure: no backoff.
        if (!signal.aborted) pendingFailures.set(userId, (pendingFailures.get(userId) ?? 0) + 1)
        throw err
      }
    },
    retry: false,
    refetchInterval: () => pendingInterval(pendingFailures.get(userId) ?? 0),
    refetchIntervalInBackground: false,
    refetchOnWindowFocus: true,
    meta: { path: '/api/hitl/pending' },
  })
}
