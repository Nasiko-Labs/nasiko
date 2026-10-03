/**
 * The Overview page's failure and absence paths (ship coverage audit; plans/feat-overview.md §5-§8, design 5A):
 * each card fails on its own, and a budgets endpoint the server doesn't have is absent, never failed.
 */
import { screen, waitFor, within } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { http, HttpResponse } from 'msw'
import { afterEach, describe, expect, it } from 'vitest'
import { configureMocks } from '@/mocks/handlers'
import { now, seed, setupPinnedSeed } from '@/test/pinnedSeed'
import { renderApp } from '@/test/renderApp'
import { recordRequests, server } from '@/test/setup'
import { copy } from './copy'

setupPinnedSeed()
afterEach(() => configureMocks({ seed, now, variant: null, routerVariants: [] }))

const card = async (id: string) => screen.findByTestId(id)

describe('Overview failures', () => {
  it('shows the Fleet health error with Retry when the agent list fails, keeping focus on the card', async () => {
    server.use(http.get('*/api/agents', () => new HttpResponse('boom', { status: 500 })))
    renderApp('/')
    const health = await card('overview-health')
    await within(health).findByText(copy.couldntLoad(copy.health.what))
    await userEvent.click(within(health).getByRole('button', { name: copy.retry }))
    expect(document.activeElement).toBe(within(health).getByRole('heading', { level: 2 }))
  })

  it('keeps each failure in its own card: cost data, harness usage and recent sessions', async () => {
    configureMocks({ variant: 'usage-500' })
    server.use(
      http.get(
        '*/api/observability/finops/dashboard',
        () => new HttpResponse('boom', { status: 500 }),
      ),
      http.get(
        '*/api/observability/session/list',
        () => new HttpResponse('bad start_time', { status: 400 }),
      ),
    )
    renderApp('/')
    const health = await card('overview-health')
    expect(
      await within(health).findByText(
        copy.couldntCheck(copy.health.costWhat),
        {},
        { timeout: 5000 },
      ),
    ).toBeInTheDocument()
    // Ratings still show (Unknown), with the four count links.
    expect(
      within(health).getAllByRole('link', { name: /^\d+ (healthy|watch|needs? action|unknown)/ }),
    ).toHaveLength(4)
    expect(
      await within(await card('overview-spend')).findByText(
        copy.couldntCheck(copy.spend.driversWhat),
      ),
    ).toBeInTheDocument()
    expect(
      await within(await card('overview-harnesses')).findByText(
        copy.couldntCheck(copy.harnesses.what),
      ),
    ).toBeInTheDocument()
    // A non-trace-store error is a plain card error, not "needs the trace store".
    const sessions = await card('overview-sessions')
    expect(
      await within(sessions).findByText(copy.couldntLoad(copy.sessions.what)),
    ).toBeInTheDocument()
    expect(within(sessions).queryByText(copy.sessions.traceStore)).toBeNull()
    expect(
      await within(await card('overview-needs')).findByText(
        copy.couldntCheck(copy.needs.source.sessions),
      ),
    ).toBeInTheDocument()
  })

  // Budgets hidden (no server support for /api/budgets yet, R-L10): un-skip with the commented budget code.

  it.skip('treats a bare 404 from /api/budgets as absent: no failed line and no budget rows', async () => {
    server.use(http.get('/api/budgets', () => new HttpResponse('Not Found', { status: 404 })))
    const rec = recordRequests()
    renderApp('/')
    const needs = await card('overview-needs')
    await waitFor(() => expect(within(needs).queryByLabelText(copy.loading)).toBeNull(), {
      timeout: 5000,
    })
    await waitFor(() => expect(within(needs).getByText(/^Checked /)).toBeInTheDocument())
    expect(within(needs).queryByText(copy.couldntCheck(copy.needs.source.budgets))).toBeNull()
    expect(needs.querySelector('[data-kind="budget"]')).toBeNull()
    rec.stop()
    expect(rec.urls.map((u) => u.pathname)).toContain('/api/budgets')
  })

  // Budgets hidden (no server support for /api/budgets yet, R-L10): un-skip with the commented budget code.

  it.skip("says it couldn't check budgets when they fail, and never claims an all-clear", async () => {
    configureMocks({ routerVariants: ['router-budgets-fail'] })
    renderApp('/')
    const needs = await card('overview-needs')
    expect(
      await within(needs).findByText(
        copy.couldntCheck(copy.needs.source.budgets),
        {},
        { timeout: 5000 },
      ),
    ).toBeInTheDocument()
    expect(within(needs).queryByTestId('needs-empty')).toBeNull()
  })
})
