/** The background follower (eng review R8, design review 8): polls, visibility, the open Build page, one toast. */
import { QueryClient } from '@tanstack/react-query'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import {
  clearFollower,
  follow,
  followedCount,
  importFinished,
  openRegistryView,
  pollOnce,
  setFollowerClock,
  setNotifier,
  setOpenBuild,
  setUploadsFetcher,
  type FollowNotice,
} from './follower'
import { FOLLOW_BACKOFF_MAX_MS, FOLLOW_MAX_MS, FOLLOW_POLL_MS } from './tuning'
import type { UploadStatus } from './types'

const row = (id: string, status: string, name = `agent-${id}`) =>
  ({ upload_id: id, status, agent_name: name }) as UploadStatus
const qc = new QueryClient()

let rows: UploadStatus[]
let reads: number
let notices: FollowNotice[]
let clock: number
let unregister: () => void

function setVisibility(state: 'visible' | 'hidden') {
  Object.defineProperty(document, 'visibilityState', { configurable: true, get: () => state })
  document.dispatchEvent(new Event('visibilitychange'))
}

beforeEach(() => {
  vi.useFakeTimers()
  rows = []
  reads = 0
  notices = []
  clock = 0
  setFollowerClock(() => clock)
  unregister = setNotifier((n) => notices.push(n))
  setVisibility('visible')
})
afterEach(() => {
  clearFollower()
  setUploadsFetcher(null)
  unregister()
  vi.restoreAllMocks()
  vi.useRealTimers()
})

const startFollowing = (...ids: string[]) => {
  for (const id of ids) follow(qc, id, `agent-${id}`)
  // After `follow` (which installs the default fetcher once), the test's fake read replaces it.
  setUploadsFetcher(async () => {
    reads++
    return rows
  })
}

describe('follower', () => {
  it('reads the uploads once per tick however many builds it follows, and stops when none are left', async () => {
    startFollowing('a', 'b', 'c')
    rows = [row('a', 'processing'), row('b', 'processing'), row('c', 'initiated')]
    await vi.advanceTimersByTimeAsync(FOLLOW_POLL_MS)
    expect(reads).toBe(1)
    rows = [row('a', 'completed'), row('b', 'failed'), row('c', 'completed')]
    await vi.advanceTimersByTimeAsync(FOLLOW_POLL_MS)
    expect(reads).toBe(2)
    expect(followedCount()).toBe(0)
    await vi.advanceTimersByTimeAsync(FOLLOW_POLL_MS * 3)
    expect(reads).toBe(2)
  })

  it('does not poll while the tab is hidden, and resumes when it shows', async () => {
    startFollowing('a')
    rows = [row('a', 'processing')]
    setVisibility('hidden')
    await vi.advanceTimersByTimeAsync(FOLLOW_POLL_MS * 4)
    expect(reads).toBe(0)
    setVisibility('visible')
    await vi.advanceTimersByTimeAsync(FOLLOW_POLL_MS)
    expect(reads).toBe(1)
  })

  it('toasts exactly once when a build finishes off its page, and refreshes the agents', async () => {
    const invalidate = vi.spyOn(qc, 'invalidateQueries')
    startFollowing('a', 'b')
    rows = [row('a', 'completed', 'support-bot'), row('b', 'failed', 'triage')]
    await pollOnce()
    await pollOnce()
    expect(notices).toEqual([
      { kind: 'running', buildId: 'a', agentId: null, name: 'support-bot' },
      { kind: 'failed', buildId: 'b', name: 'triage' },
    ])
    expect(invalidate).toHaveBeenCalledTimes(1)
    const { predicate } = invalidate.mock.calls[0]![0] as unknown as {
      predicate: (q: { queryKey: unknown[] }) => boolean
    }
    expect(predicate({ queryKey: ['agents', 'catalog'] })).toBe(true)
    expect(predicate({ queryKey: ['agents', 'watch', 'x'] })).toBe(false)
  })

  it('drops the open Build page’s build without a toast (that page streams it)', async () => {
    startFollowing('a')
    setOpenBuild('a')
    rows = [row('a', 'completed')]
    await pollOnce()
    expect(notices).toEqual([])
    expect(followedCount()).toBe(0)
  })

  it('gives up on a build never seen finishing, silently', async () => {
    startFollowing('a')
    clock = FOLLOW_MAX_MS
    await pollOnce()
    expect(followedCount()).toBe(0)
    expect(notices).toEqual([])
  })

  it('announces a finished import unless the Registry tab is open', () => {
    const close = openRegistryView()
    importFinished({ kind: 'notRunning', agentId: 'x', name: 'img' })
    expect(notices).toEqual([])
    close()
    importFinished({ kind: 'notRunning', agentId: 'x', name: 'img' })
    expect(notices).toHaveLength(1)
  })

  it('backs off while the read fails, keeps the build, and recovers', async () => {
    startFollowing('a')
    let fail = true
    setUploadsFetcher(async () => {
      reads++
      if (fail) throw new Error('502')
      return [row('a', 'completed')]
    })
    await vi.advanceTimersByTimeAsync(FOLLOW_POLL_MS)
    expect(reads).toBe(1)
    // Second try waits twice as long.
    await vi.advanceTimersByTimeAsync(FOLLOW_POLL_MS)
    expect(reads).toBe(1)
    await vi.advanceTimersByTimeAsync(FOLLOW_POLL_MS)
    expect(reads).toBe(2)
    expect(followedCount()).toBe(1)
    expect(notices).toEqual([])
    fail = false
    await vi.advanceTimersByTimeAsync(FOLLOW_BACKOFF_MAX_MS)
    expect(notices).toHaveLength(1)
    expect(followedCount()).toBe(0)
  })

  it('drops a build it could never read once it is too old, silently and without refreshing agents', async () => {
    const own = new QueryClient()
    const invalidate = vi.spyOn(own, 'invalidateQueries')
    follow(own, 'a', 'agent-a')
    setUploadsFetcher(async () => {
      throw new Error('route absent')
    })
    clock = FOLLOW_MAX_MS
    await pollOnce()
    expect(followedCount()).toBe(0)
    expect(notices).toEqual([])
    expect(invalidate).not.toHaveBeenCalled()
  })

  it('passes the agent id through when the upload knew it', async () => {
    follow(qc, 'z', 'bot', 'agent-1')
    setUploadsFetcher(async () => [row('z', 'completed', 'bot')])
    await pollOnce()
    expect(notices).toEqual([{ kind: 'running', buildId: 'z', agentId: 'agent-1', name: 'bot' }])
  })

  it('sign-out clears everything (R3)', async () => {
    startFollowing('a')
    clearFollower()
    expect(followedCount()).toBe(0)
    await vi.advanceTimersByTimeAsync(FOLLOW_POLL_MS * 2)
    expect(reads).toBe(0)
  })
})
