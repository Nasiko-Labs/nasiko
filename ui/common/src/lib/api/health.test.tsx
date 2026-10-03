/** The /health polling: the sidebar's slow check everywhere, plus the Status page's own while it is open (at /status). */
import { screen } from '@testing-library/react'
import { describe, expect, it, vi } from 'vitest'
import { setupPinnedSeed } from '@/test/pinnedSeed'
import { renderApp } from '@/test/renderApp'
import { HEALTH_INTERVAL_MS, SIDEBAR_HEALTH_INTERVAL_MS } from './health'

setupPinnedSeed()
describe('sidebar health check', () => {
  it('polls every 60 s without a focus refetch on ordinary pages', async () => {
    const { queryClient } = renderApp('/agents')
    await screen.findByTestId('status-row')
    const observers = queryClient.getQueryCache().find({ queryKey: ['health'] })!.observers
    expect(observers).toHaveLength(1)
    expect(observers[0]!.options.refetchInterval).toBe(SIDEBAR_HEALTH_INTERVAL_MS)
    expect(observers[0]!.options.refetchOnWindowFocus).toBe(false)
  })

  it('on the Overview (/) only the sidebar polls: no 15 s page check (overview eng R6)', async () => {
    const { queryClient } = renderApp('/')
    await screen.findByRole('heading', { level: 1, name: 'Overview' })
    await screen.findByTestId('status-row')
    const observers = queryClient.getQueryCache().find({ queryKey: ['health'] })!.observers
    expect(observers).toHaveLength(1)
    expect(observers[0]!.options.refetchInterval).toBe(SIDEBAR_HEALTH_INTERVAL_MS)
  })

  it('on the Status page both poll, and neither refetches on focus (the client default)', async () => {
    const { queryClient } = renderApp('/status')
    await screen.findByTestId('status-row')
    await vi.waitFor(() =>
      expect(queryClient.getQueryCache().find({ queryKey: ['health'] })!.observers).toHaveLength(2),
    )
    const observers = queryClient.getQueryCache().find({ queryKey: ['health'] })!.observers
    expect(observers.map((o) => o.options.refetchInterval).sort()).toEqual(
      [HEALTH_INTERVAL_MS, SIDEBAR_HEALTH_INTERVAL_MS].sort(),
    )
    // Unset means the client default (queryClient.ts: false), which TanStack resolves per observer.
    for (const o of observers) expect(o.options.refetchOnWindowFocus ?? false).toBe(false)
  })
})
