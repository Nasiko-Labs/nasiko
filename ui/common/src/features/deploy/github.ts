/**
 * Connect GitHub (plans/feat-deploy.md §4.2): the OAuth consent opens in a popup (its callback lands on the legacy UI,
 * which is fine there; server gap D-9), and the page polls the connection until it's made or the popup closes.
 */
import { type QueryClient, useQueryClient } from '@tanstack/react-query'
import { useCallback, useEffect, useRef, useState } from 'react'
import { fetchGithubLoginUrl, githubConnectedNow, githubKeys } from './api'
import { ApiError } from '@/lib/api/client'
import { GITHUB_POLL_MS, GITHUB_POLL_TRIES, GITHUB_POPUP } from './tuning'
import type { GithubRepo } from './types'

export type ConnectState =
  { kind: 'idle' } | { kind: 'waiting' } | { kind: 'blocked'; url: string } | { kind: 'failed' }

/** These reads run outside a query: on a 401, re-check `me`, whose 401 sends the app to /login?expired=true. */
function expiredCheck(qc: QueryClient, err: unknown) {
  if (err instanceof ApiError && err.status === 401) void qc.invalidateQueries({ queryKey: ['me'] })
}

export function useGithubConnect() {
  const qc = useQueryClient()
  const [state, setState] = useState<ConnectState>({ kind: 'idle' })
  const stop = useRef<(() => void) | null>(null)
  const attempt = useRef(0)
  useEffect(
    () => () => {
      attempt.current += 1
      stop.current?.()
    },
    [],
  )

  const connect = useCallback(async () => {
    stop.current?.()
    // A second click while the login URL loads supersedes the first: only the newest attempt opens a popup and polls.
    const mine = ++attempt.current
    let url: string
    try {
      url = await fetchGithubLoginUrl()
    } catch (err) {
      if (mine !== attempt.current) return
      setState({ kind: 'failed' })
      expiredCheck(qc, err)
      return
    }
    if (mine !== attempt.current) return
    const popup = window.open(
      url,
      GITHUB_POPUP.name,
      `popup,width=${GITHUB_POPUP.width},height=${GITHUB_POPUP.height}`,
    )
    if (!popup) setState({ kind: 'blocked', url })
    else setState({ kind: 'waiting' })
    let tries = 0
    let timer: ReturnType<typeof setTimeout> | null = null
    let cancelled = false
    stop.current = () => {
      cancelled = true
      if (timer) clearTimeout(timer)
    }
    const done = () => {
      stop.current?.()
      setState({ kind: 'idle' })
      void qc.invalidateQueries({ queryKey: githubKeys.user })
      void qc.invalidateQueries({ queryKey: githubKeys.repos })
    }
    const tick = async () => {
      if (cancelled) return
      tries++
      let connected = false
      try {
        connected = await githubConnectedNow()
      } catch (err) {
        // A dead session stops the poll and takes the app's one expiry path; other errors just try again.
        if (err instanceof ApiError && err.status === 401) {
          stop.current?.()
          setState({ kind: 'idle' })
          expiredCheck(qc, err)
          return
        }
      }
      if (cancelled) return
      if (connected) return done()
      // The popup closed without connecting (checked once more above), or we ran out of tries.
      if ((popup && popup.closed) || tries >= GITHUB_POLL_TRIES) {
        stop.current?.()
        setState((s) => (s.kind === 'blocked' ? s : { kind: 'idle' }))
        return
      }
      timer = setTimeout(() => void tick(), GITHUB_POLL_MS)
    }
    timer = setTimeout(() => void tick(), GITHUB_POLL_MS)
  }, [qc])

  return { state, connect }
}

/** Repositories matching a search (name, full name or description), newest first as the server sends them. Pure. */
export function filterRepos(repos: readonly GithubRepo[], q: string | undefined): GithubRepo[] {
  const needle = (q ?? '').trim().toLowerCase()
  if (!needle) return [...repos]
  return repos.filter(
    (r) =>
      r.full_name.toLowerCase().includes(needle) ||
      (r.description ?? '').toLowerCase().includes(needle),
  )
}
