import { screen, waitFor } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { describe, expect, it } from 'vitest'
import { setupPinnedSeed } from '@/test/pinnedSeed'
import { renderApp } from '@/test/renderApp'
import { recordRequests } from '@/test/setup'

setupPinnedSeed()

describe('route loaders prefetch on intent (plan §8 Phase 3)', () => {
  it('hovering a nav link starts the target page’s first request; the page then reuses it', async () => {
    // A quiet page (the Overview at / fetches the dashboard itself and has its own TokenOps link).
    renderApp('/status')
    const link = await screen.findByRole('link', { name: 'TokenOps' })
    const reqs = recordRequests()
    try {
      await userEvent.hover(link)
      await waitFor(() =>
        expect(reqs.urls.map((u) => u.pathname)).toContain('/api/observability/finops/dashboard'),
      )
      const dashboards = () =>
        reqs.urls.filter(
          (u) =>
            u.pathname === '/api/observability/finops/dashboard' &&
            !u.searchParams.has('start_time'),
        ).length
      const before = dashboards()
      await userEvent.click(link)
      await screen.findByTestId('summary-narrative')
      expect(dashboards()).toBe(before)
    } finally {
      reqs.stop()
    }
  })
})
