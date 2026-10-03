/**
 * Fixes from the /ship pre-landing review of the Overview (2026-09-29): each test pins one behaviour the specialists
 * or the red team found broken.
 */
import { render, screen, waitFor, within } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import {
  createMemoryHistory,
  createRootRoute,
  createRouter,
  RouterProvider,
} from '@tanstack/react-router'
import { delay, http, HttpResponse } from 'msw'
import type { ReactNode } from 'react'
import { afterEach, describe, expect, it, vi } from 'vitest'
import { readRailView } from '@/features/chat/rememberTarget'
import { configureMocks } from '@/mocks/handlers'
import { now, seed, setupPinnedSeed } from '@/test/pinnedSeed'
import { renderApp } from '@/test/renderApp'
import { recordRequests, server } from '@/test/setup'
import type { NeedsYou as NeedsYouData, Spend as SpendData } from './api'
import { NeedsYou } from './components/NeedsYou'
import { Spend } from './components/Spend'
import { copy } from './copy'
import { mergeNeeds } from './needs'

setupPinnedSeed()
afterEach(() =>
  configureMocks({ seed, now, loggedIn: true, superuser: null, variant: null, routerVariants: [] }),
)

const card = (id: string) => screen.findByTestId(id)
function renderInRouter(node: ReactNode) {
  const router = createRouter({
    routeTree: createRootRoute({ component: () => <>{node}</> }),
    history: createMemoryHistory({ initialEntries: ['/'] }),
  })
  return render(<RouterProvider router={router} />)
}

describe('a failed agent list', () => {
  it('leaves the other cards working and says agent health could not be checked', async () => {
    server.use(http.get('*/api/agents', () => new HttpResponse('boom', { status: 500 })))
    renderApp('/')
    expect(
      await within(await card('overview-health')).findByText(copy.couldntLoad(copy.health.what)),
    ).toBeInTheDocument()
    expect(
      await within(await card('overview-month')).findByTestId(
        'overview-mtd',
        {},
        { timeout: 5000 },
      ),
    ).toBeInTheDocument()
    expect(
      await within(await card('overview-needs')).findByText(
        copy.couldntCheck(copy.needs.source.agents),
        {},
        { timeout: 5000 },
      ),
    ).toBeInTheDocument()
    expect(screen.getByTestId('overview-headline')).not.toHaveTextContent(copy.headline.checking)
  })
})

describe('Needs you waits for the ratings', () => {
  it('keeps agent health loading until the rating dashboards answer, so nothing is called clear early', async () => {
    server.use(
      http.get('*/api/observability/finops/dashboard', async () => {
        await delay('infinite')
        return HttpResponse.json({})
      }),
    )
    renderApp('/')
    const needs = await card('overview-needs')
    // With rows, a labelled skeleton under them; with none (budgets hidden, so no seeded budget rows), the card skeleton.
    const loading = () =>
      within(needs).queryByLabelText(copy.loading) ?? within(needs).queryByText(copy.loading)
    await waitFor(() => expect(loading()).toBeInTheDocument())
    await new Promise((r) => setTimeout(r, 500))
    expect(within(needs).queryByTestId('needs-empty')).toBeNull()
    expect(loading()).toBeInTheDocument()
  })

  // Budgets hidden (no server support for /api/budgets yet, R-L10): un-skip with the commented budget code.

  it.skip('lists a stopped agent budget as the agent, never again as a budget row', async () => {
    renderApp('/')
    const needs = await card('overview-needs')
    await waitFor(
      () => expect(needs.querySelectorAll('[data-kind="budget"]').length).toBeGreaterThan(0),
      { timeout: 5000 },
    )
    const budgetRows = [...needs.querySelectorAll('[data-kind="budget"]')].map(
      (r) => r.textContent ?? '',
    )
    expect(budgetRows.some((t) => t.includes(copy.needs.yourBudget))).toBe(true)
    // The seed's busiest agent is stopped by its budget: that shows as a Needs-action agent, not "over its limit".
    expect(budgetRows.some((t) => /over its limit/.test(t))).toBe(false)
  })
})

describe('budgets on a server without them', () => {
  it('asks for budget status only after the budget list answered', async () => {
    server.use(http.get('*/api/budgets', () => new HttpResponse('', { status: 404 })))
    const rec = recordRequests()
    renderApp('/')
    const needs = await card('overview-needs')
    await waitFor(() => expect(needs.querySelectorAll('[data-kind]').length).toBeGreaterThan(0), {
      timeout: 5000,
    })
    rec.stop()
    expect(rec.urls.map((u) => u.pathname).filter((p) => p === '/api/budgets/status')).toEqual([])
  })
})

describe('the account request failing', () => {
  it('says so and links to server status instead of loading forever', async () => {
    server.use(http.get('*/api/me', () => new HttpResponse('upstream', { status: 503 })))
    renderApp('/')
    const msg = await screen.findByTestId('overview-me-error', {}, { timeout: 5000 })
    expect(msg).toHaveTextContent(copy.meFailed)
    expect(within(msg).getByRole('link', { name: copy.checkStatus })).toHaveAttribute(
      'href',
      '/status',
    )
  })
})

describe('Recent sessions with the server down', () => {
  it('does not blame the trace store for a bare proxy 502', async () => {
    server.use(
      http.get('*/api/observability/session/list', () => new HttpResponse('', { status: 502 })),
    )
    renderApp('/')
    const sessions = await card('overview-sessions')
    expect(
      await within(sessions).findByText(
        copy.couldntLoad(copy.sessions.what),
        {},
        { timeout: 5000 },
      ),
    ).toBeInTheDocument()
    expect(within(sessions).queryByText(copy.sessions.traceStore)).toBeNull()
  })
})

describe('Fleet health count badges', () => {
  it("keep the app focus ring on the Needs action count (not the badge variant's faint destructive ring)", async () => {
    renderApp('/')
    const health = await card('overview-health')
    const action = await within(health).findByRole(
      'link',
      { name: /^\d+ needs? action/ },
      { timeout: 5000 },
    )
    expect(action.className).toContain('focus-visible:ring-ring')
    expect(action.className).not.toContain('focus-visible:ring-destructive')
  })
})

describe('components', () => {
  const retries = () => ({
    requests: vi.fn(),
    agents: vi.fn(),
    budgets: vi.fn(),
    sessions: vi.fn(),
  })

  it('the outside-Chat row opens Chat on the Waiting view', async () => {
    const data: NeedsYouData = {
      needs: mergeNeeds({
        requests: { state: 'ok', value: { chats: [], outside: 2 } },
        agents: { state: 'ok', value: [] },
        budgets: { state: 'absent' },
        sessions: { state: 'ok', value: { failed: 0, checked: 25, agents: [] } },
      }),
      lastChecked: 0,
      retry: retries(),
      sessions: {} as NeedsYouData['sessions'],
    }
    renderInRouter(<NeedsYou data={data} now={0} userId="u1" />)
    await userEvent.click(await screen.findByRole('link', { name: copy.needs.openWaiting }))
    expect(readRailView('u1')).toBe('waiting')
  })

  it("sends an old spike to TokenOps' day panel, a recent one to Sessions", async () => {
    const at = Date.parse('2026-09-29T12:00:00Z')
    const spend = (date: string): SpendData => ({
      summary: {
        mtd: 120,
        elapsedDays: 29,
        daysInMonth: 30,
        show: false,
        low: null,
        high: null,
        lastMonthTotal: 0,
        lastMonthSameDays: 0,
        vsLastMonthPct: null,
      },
      isPending: false,
      error: null,
      rangeDays: 30,
      totals: { spend: 120, runs: 10, previous: null },
      totalsError: null,
      days: [],
      stack: null,
      stackPending: false,
      spike: { date, spend: 100, factor: 5 },
      drivers: [],
      other: null,
      driversError: null,
      unpriced: false,
      retry: vi.fn(),
    })
    const { unmount } = renderInRouter(<Spend spend={spend('2026-09-15')} now={at} />)
    expect(
      (await screen.findByRole('link', { name: new RegExp(copy.spend.seeDay) })).getAttribute(
        'href',
      ),
    ).toMatch(/^\/tokenops\?.*day=2026-09-15/)
    unmount()
    renderInRouter(<Spend spend={spend('2026-09-27')} now={at} />)
    expect(
      (await screen.findByRole('link', { name: new RegExp(copy.spend.seeSessions) })).getAttribute(
        'href',
      ),
    ).toMatch(/^\/sessions\?.*day=2026-09-27/)
  })
})
