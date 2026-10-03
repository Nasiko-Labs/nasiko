/** The lifecycle watch store ends with the session, like the query cache it replaced (Phase 6). */
import { QueryClientProvider, useQuery, type QueryClient } from '@tanstack/react-query'
import type { AnyRouter } from '@tanstack/react-router'
import { act, renderHook, waitFor } from '@testing-library/react'
import type { ReactNode } from 'react'
import { describe, expect, it } from 'vitest'
import { ApiError } from '@/lib/api/client'
import { createQueryClient } from '@/lib/queryClient'
import { seed, setupPinnedSeed } from '@/test/pinnedSeed'
import { useDeleteAgent, useWatch } from './api'

setupPinnedSeed()

const router = {
  state: { location: { pathname: '/agents/mine', href: '/agents/mine' } },
  navigate: async () => {},
} as unknown as AnyRouter

function setup(id = 'a1') {
  const qc = createQueryClient(() => router, { retry: false })
  const wrapper = ({ children }: { children: ReactNode }) => (
    <QueryClientProvider client={qc}>{children}</QueryClientProvider>
  )
  const hook = renderHook(() => useWatch(id), { wrapper })
  act(() => hook.result.current.begin('restart'))
  expect(hook.result.current.watching).toBe(true)
  return { qc, hook, wrapper }
}

describe('watch store lifetime', () => {
  it('a new query client starts with no watches', () => {
    const first = setup()
    setup('a2')
    expect(first.hook.result.current.watch).toBeNull()
  })

  it.each<[string, (qc: QueryClient) => unknown]>([
    ['clear() (sign-out, sign-in, another tab signing out)', (qc) => qc.clear()],
    ['an unfiltered resetQueries() (another account signed in)', (qc) => qc.resetQueries()],
  ])('%s drops every watch', (_name, reset) => {
    const { qc, hook } = setup()
    act(() => void reset(qc))
    expect(hook.result.current.watch).toBeNull()
  })

  it('a filtered resetQueries() keeps the watches', () => {
    const { qc, hook } = setup()
    act(() => void qc.resetQueries({ queryKey: ['tokenops'] }))
    expect(hook.result.current.watching).toBe(true)
  })

  it('a 401 expiry drops every watch', async () => {
    const { hook, wrapper } = setup()
    renderHook(
      () =>
        useQuery({
          queryKey: ['x'],
          queryFn: () => Promise.reject(new ApiError(401, 'token expired', '/api/x', '401')),
        }),
      { wrapper },
    )
    await waitFor(() => expect(hook.result.current.watch).toBeNull())
  })

  it('deleting an agent drops its watch only', async () => {
    const id = seed.agents[0]!.id
    const { hook, wrapper } = setup(id)
    const other = renderHook(() => useWatch('a9'), { wrapper })
    act(() => other.result.current.begin('start'))
    const del = renderHook(() => useDeleteAgent(id), { wrapper })
    await act(() => del.result.current.mutateAsync())
    expect(hook.result.current.watch).toBeNull()
    expect(other.result.current.watching).toBe(true)
  })
})
