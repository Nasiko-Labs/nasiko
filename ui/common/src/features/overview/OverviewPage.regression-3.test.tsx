// Regression: ISSUE-004 — with no agent deployed, Fleet health blamed "a week of activity" for having no ratings
// Found by /qa on 2026-09-29 (live, EE seed: every agent registered)
// Report: .gstack/qa-reports/qa-report-localhost-2026-09-29.md
import { screen, waitFor, within } from '@testing-library/react'
import { http, HttpResponse } from 'msw'
import { agentsList } from '@/mocks/observability'
import { describe, expect, it } from 'vitest'
import { seed, setupPinnedSeed } from '@/test/pinnedSeed'
import { renderApp } from '@/test/renderApp'
import { server } from '@/test/setup'
import { copy } from './copy'

setupPinnedSeed()

describe('Fleet health with nothing deployed (ISSUE-004)', () => {
  it('says no agent is deployed, not that agents need more activity', async () => {
    server.use(
      http.get('*/api/agents', ({ request }) => {
        const u = new URL(request.url)
        const rows = agentsList(seed, {
          limit: u.searchParams.get('limit'),
          offset: u.searchParams.get('offset'),
        })
        return HttpResponse.json(rows.map((a) => ({ ...a, status: 'registered' })))
      }),
    )
    renderApp('/')
    const health = await screen.findByTestId('overview-health')
    expect(
      await within(health).findByTestId('health-none-deployed', {}, { timeout: 5000 }),
    ).toHaveTextContent(copy.health.noneDeployed)
    await waitFor(() => expect(health).not.toHaveTextContent(/week of activity/))
  })

  it('shows no such line when agents are deployed', async () => {
    renderApp('/')
    const health = await screen.findByTestId('overview-health')
    await waitFor(() => expect(within(health).getAllByRole('link').length).toBeGreaterThan(4))
    expect(within(health).queryByTestId('health-none-deployed')).toBeNull()
  })
})
