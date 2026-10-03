/**
 * Live pulse for fleet mode.
 * - Mock mode: replays today's sessions. If Live is running when the page opens, the newest
 *   REPLAY_HOLD of today's sessions are held back and revealed one every REPLAY_STEP_MS
 *   (deterministic; nothing is invented).
 * - Live mode: polls the first page (LIVE_PAGE_SIZE) every LIVE_POLL_MS, only while Live is
 *   on and the tab is visible; rows not in the loaded list are arrivals.
 * Arrivals wait behind a "N new sessions" pill while the user interacts with the list
 * (pointer over it, or scrolled down) and insert on click or after IDLE_MS of no
 * interaction. State only changes in timer and event callbacks, never during render.
 *
 * The pulse's client state (tab visibility, the replay position, which arrivals are in the list)
 * is a vanilla zustand store: visibility is written outside React, by the document's event, and
 * every update is a new immutable snapshot the page reads through selectors.
 */
import { useQuery, useQueryClient } from '@tanstack/react-query'
import { useCallback, useEffect, useMemo, useRef, useState } from 'react'
import { createStore, useStore } from 'zustand'
import { apiFetch, withQuery } from '@/lib/api/client'
import {
  IDLE_MS,
  LIVE_PAGE_SIZE,
  LIVE_POLL_MS,
  REPLAY_STEP_MS,
} from '@/features/observability/tuning'
import type { SessionListResponse, SessionSummary } from '@/features/observability/types'

export const REPLAY_HOLD = 5
const SCROLLED = 120

interface LiveState {
  /** The tab is visible: replay and polling run only then. */
  visible: boolean
  /** Replayed (held-back) sessions revealed so far. */
  revealed: number
  /** Arrivals already inserted into the list. */
  inserted: ReadonlySet<string>
  /** Ids inserted by the last flush. */
  fresh: ReadonlySet<string>
}

const isVisible = () => typeof document === 'undefined' || document.visibilityState !== 'hidden'
const initial = (): LiveState => ({
  visible: isVisible(),
  revealed: 0,
  inserted: new Set(),
  fresh: new Set(),
})

// ponytail: one store for the one Sessions page; it resets when the page unmounts, so each visit
// starts fresh as before. Key it by page instance if two feeds ever mount at once.
const liveStore = createStore<LiveState>()(initial)
if (typeof document !== 'undefined')
  document.addEventListener('visibilitychange', () => liveStore.setState({ visible: isVisible() }))

export interface LiveFeed {
  rows: SessionSummary[]
  /** The last live poll failed: the list may be stale. */
  pollFailed: boolean
  queued: number
  flush: () => void
  /** Attach to the list container. */
  listProps: { onPointerEnter: () => void; onPointerLeave: () => void; onPointerMove: () => void }
  /** Ids inserted by the last flush (float-up highlight, polite announcement). */
  fresh: ReadonlySet<string>
}

/** Live mode keeps at most this many polled sessions (arrivals plus updated rows). */
const LIVE_KEEP = 500

export function useLiveFeed({
  base,
  mode,
  running,
  today,
  windowStart,
  windowEndMs,
}: {
  base: readonly SessionSummary[]
  mode: 'mock' | 'live'
  /** Live on, not paused, fleet mode. */
  running: boolean
  /** UTC date of "today" (mock replay only replays today). */
  today: string
  windowStart: Date
  /** Rows after this are outside the window (a closed past window); null = open-ended. */
  windowEndMs: number | null
}): LiveFeed {
  const visible = useStore(liveStore, (s) => s.visible)
  const revealed = useStore(liveStore, (s) => s.revealed)
  const inserted = useStore(liveStore, (s) => s.inserted)
  const fresh = useStore(liveStore, (s) => s.fresh)
  useEffect(() => () => liveStore.setState(initial()), [])
  const client = useQueryClient()
  // Replay only if Live was running when the page opened (a paused start shows every row).
  const [replay] = useState(mode === 'mock' && running)
  const held = useMemo(() => {
    if (!replay) return []
    return base
      .filter((s) => s.start_time?.startsWith(today))
      .sort((a, b) => Date.parse(a.start_time ?? '') - Date.parse(b.start_time ?? ''))
      .slice(-REPLAY_HOLD)
      .map((s) => s.session_id)
  }, [replay, base, today])

  useEffect(() => {
    if (!replay || !running || !visible || revealed >= held.length) return
    const t = setInterval(
      () => liveStore.setState((s) => ({ revealed: Math.min(held.length, s.revealed + 1) })),
      REPLAY_STEP_MS,
    )
    return () => clearInterval(t)
  }, [replay, running, visible, revealed, held.length])

  const path = withQuery('/api/observability/session/list', {
    start_time: windowStart.toISOString(),
    limit: LIVE_PAGE_SIZE,
    offset: 0,
  })
  const liveKey = ['sessions', 'live-head', windowStart.toISOString()] as const
  const poll = useQuery({
    queryKey: liveKey,
    // Accumulate: merge each head page into what earlier polls saw (newest values win), so
    // an arrival stays listed after 25 newer sessions push it off the polled page.
    queryFn: async ({ signal }) => {
      const head = await apiFetch<SessionListResponse>(path, { signal }).then(
        (b) => b.data.sessions,
      )
      const byId = new Map(
        (client.getQueryData<SessionSummary[]>(liveKey) ?? []).map((r) => [r.session_id, r]),
      )
      for (const r of head) byId.set(r.session_id, r)
      return [...byId.values()]
        .sort(
          (a, b) => (Date.parse(b.start_time ?? '') || 0) - (Date.parse(a.start_time ?? '') || 0),
        )
        .slice(0, LIVE_KEEP)
    },
    enabled: mode === 'live' && running && visible,
    refetchInterval: mode === 'live' && running && visible ? LIVE_POLL_MS : false,
    // A poll: the next one is the retry.
    retry: false,
    meta: { path },
  })

  const baseIds = useMemo(() => new Set(base.map((s) => s.session_id)), [base])
  const inWindow = useMemo(
    () =>
      (poll.data ?? []).filter(
        (s) => windowEndMs === null || !s.start_time || Date.parse(s.start_time) <= windowEndMs,
      ),
    [poll.data, windowEndMs],
  )
  const polled = useMemo(
    () => inWindow.filter((s) => !baseIds.has(s.session_id)),
    [inWindow, baseIds],
  )
  // Rows already listed take the newest polled values (cost, duration, last output).
  const latest = useMemo(
    () => new Map(inWindow.filter((s) => baseIds.has(s.session_id)).map((s) => [s.session_id, s])),
    [inWindow, baseIds],
  )
  // Everything that has "arrived": replayed rows and polled rows, oldest first.
  const arrivedIds = useMemo(
    () => [...held.slice(0, revealed), ...polled.map((s) => s.session_id).reverse()],
    [held, revealed, polled],
  )

  const pending = useMemo(
    () => arrivedIds.filter((id) => !inserted.has(id)),
    [arrivedIds, inserted],
  )
  const interacting = useRef(false)
  const lastMove = useRef(0)

  const insert = useCallback((ids: string[]) => {
    if (!ids.length) return
    liveStore.setState((s) => ({ inserted: new Set([...s.inserted, ...ids]), fresh: new Set(ids) }))
  }, [])

  // Pending rows insert once the user is idle (checked every second).
  useEffect(() => {
    if (!pending.length) return
    const t = setInterval(() => {
      const idle =
        !interacting.current &&
        Date.now() - lastMove.current > IDLE_MS &&
        window.scrollY <= SCROLLED
      if (idle) insert(pending)
    }, 1000)
    return () => clearInterval(t)
  }, [pending, insert])

  const flush = useCallback(() => {
    insert(pending)
    window.scrollTo?.({ top: 0 })
  }, [insert, pending])

  const rows = useMemo(() => {
    const hidden = new Set([...held.filter((id) => !inserted.has(id))])
    const arrivals = polled.filter((s) => inserted.has(s.session_id))
    return [...arrivals, ...base.map((s) => latest.get(s.session_id) ?? s)].filter(
      (s) => !hidden.has(s.session_id),
    )
  }, [held, inserted, polled, base, latest])

  const listProps = useMemo(
    () => ({
      onPointerEnter: () => {
        interacting.current = true
      },
      onPointerLeave: () => {
        interacting.current = false
        lastMove.current = Date.now()
      },
      onPointerMove: () => {
        lastMove.current = Date.now()
      },
    }),
    [],
  )

  return {
    rows,
    pollFailed: running && poll.isError,
    queued: pending.length,
    flush,
    listProps,
    fresh,
  }
}
