/** Ship-audit gap tests for `useGithubConnect` (plans/feat-deploy.md §4.2): how the popup poll ends. */
import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { act, renderHook } from '@testing-library/react'
import type { ReactNode } from 'react'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { ApiError } from '@/lib/api/client'
import { useGithubConnect } from './github'
import { GITHUB_POLL_MS, GITHUB_POLL_TRIES } from './tuning'

const api = vi.hoisted(() => ({
  login: vi.fn<() => Promise<string>>(),
  connected: vi.fn<() => Promise<boolean>>(),
}))
vi.mock('./api', () => ({
  fetchGithubLoginUrl: api.login,
  githubConnectedNow: api.connected,
  githubKeys: { user: ['deploy', 'github', 'user'], repos: ['deploy', 'github', 'repos'] },
}))

function setup(popup: { closed: boolean } | null) {
  vi.spyOn(window, 'open').mockReturnValue(popup as Window | null)
  const qc = new QueryClient()
  const invalidate = vi.spyOn(qc, 'invalidateQueries')
  const wrapper = ({ children }: { children: ReactNode }) => (
    <QueryClientProvider client={qc}>{children}</QueryClientProvider>
  )
  const hook = renderHook(() => useGithubConnect(), { wrapper })
  return { hook, invalidate }
}

const tick = async () =>
  act(async () => {
    await vi.advanceTimersByTimeAsync(GITHUB_POLL_MS)
  })

beforeEach(() => {
  vi.useFakeTimers()
  api.login.mockResolvedValue('https://github.com/login/oauth/authorize?x=1')
  api.connected.mockResolvedValue(false)
})
afterEach(() => {
  vi.useRealTimers()
  vi.restoreAllMocks()
  api.login.mockReset()
  api.connected.mockReset()
})

describe('useGithubConnect: the poll ends', () => {
  it('goes back to idle and stops polling when the popup closes without connecting', async () => {
    const popup = { closed: false }
    const { hook } = setup(popup)
    await act(async () => {
      await hook.result.current.connect()
    })
    expect(hook.result.current.state).toEqual({ kind: 'waiting' })
    await tick()
    expect(api.connected).toHaveBeenCalledTimes(1)
    popup.closed = true
    await tick()
    expect(hook.result.current.state).toEqual({ kind: 'idle' })
    const calls = api.connected.mock.calls.length
    await tick()
    await tick()
    expect(api.connected).toHaveBeenCalledTimes(calls)
  })

  it(`gives up after ${GITHUB_POLL_TRIES} tries`, async () => {
    const { hook } = setup({ closed: false })
    await act(async () => {
      await hook.result.current.connect()
    })
    await act(async () => {
      await vi.advanceTimersByTimeAsync(GITHUB_POLL_MS * (GITHUB_POLL_TRIES + 5))
    })
    expect(api.connected).toHaveBeenCalledTimes(GITHUB_POLL_TRIES)
    expect(hook.result.current.state).toEqual({ kind: 'idle' })
  })

  it("stops on a 401 from the token poll and re-checks ['me'] (the app's one expiry path)", async () => {
    api.connected.mockRejectedValue(
      new ApiError(401, 'missing or invalid token', '/api/auth/github/token', 'Unauthorized'),
    )
    const { hook, invalidate } = setup({ closed: false })
    await act(async () => {
      await hook.result.current.connect()
    })
    await tick()
    expect(hook.result.current.state).toEqual({ kind: 'idle' })
    expect(invalidate).toHaveBeenCalledWith({ queryKey: ['me'] })
    await tick()
    await tick()
    expect(api.connected).toHaveBeenCalledTimes(1)
  })
})
