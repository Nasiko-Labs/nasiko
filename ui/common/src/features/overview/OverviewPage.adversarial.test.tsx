/**
 * Fixes from the /ship adversarial review of the Overview (2026-09-29): the paths that let the page claim an
 * all-clear it hadn't checked, and the smaller staleness and labelling gaps.
 */
import { render, screen, waitFor, within } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import {
  createMemoryHistory,
  createRootRoute,
  createRouter,
  RouterProvider,
} from '@tanstack/react-router'
import { http, HttpResponse } from 'msw'
import { afterEach, describe, expect, it, vi } from 'vitest'
import { budgetMockState, configureMocks } from '@/mocks/handlers'
import { agentsList } from '@/mocks/observability'
import { now, seed, setupPinnedSeed } from '@/test/pinnedSeed'
import { renderApp } from '@/test/renderApp'
import { server } from '@/test/setup'
import type { NeedsYou as NeedsYouData } from './api'
import { NeedsYou } from './components/NeedsYou'
import { copy } from './copy'
import { mergeNeeds } from './needs'

setupPinnedSeed()
afterEach(() =>
  configureMocks({ seed, now, loggedIn: true, superuser: null, variant: null, routerVariants: [] }),
)

const card = (id: string) => screen.findByTestId(id)
const healthCounts = async () => {
  const health = await card('overview-health')
  await waitFor(
    () =>
      expect(
        within(health).getAllByRole('link', { name: /^\d+ (healthy|watch|needs? action|unknown)/ }),
      ).toHaveLength(4),
    { timeout: 5000 },
  )
  return Object.fromEntries(
    within(health)
      .getAllByRole('link', { name: /^\d+ (healthy|watch|needs? action|unknown)/ })
      .map((l) => {
        const [, n, what] = l.textContent!.match(/^(\d+) (.*)$/)!
        return [what!, Number(n)]
      }),
  )
}

describe('a failed cost input', () => {
  it('spend series down: Needs you keeps its rows but never says "Nothing needs you"', async () => {
    server.use(
      http.get(
        '*/api/observability/finops/spend-timeseries',
        () => new HttpResponse('boom', { status: 500 }),
      ),
    )
    renderApp('/')
    const needs = await card('overview-needs')
    expect(
      await within(needs).findByText(
        copy.couldntCheck(copy.needs.source.agents),
        {},
        { timeout: 5000 },
      ),
    ).toBeInTheDocument()
    expect(within(needs).queryByTestId('needs-empty')).toBeNull()
    expect(screen.getByTestId('overview-headline')).not.toHaveTextContent(copy.needs.nothing)
    // Known Needs-action agents (crashed, failed) still show.
    expect(needs.querySelectorAll('[data-kind="agent"]').length).toBeGreaterThan(0)
  })

  it('Retry on the agents line refetches the failed cost input, not just the agent list', async () => {
    let fail = true
    server.use(
      http.get('*/api/observability/finops/spend-timeseries', () =>
        fail ? new HttpResponse('boom', { status: 500 }) : undefined,
      ),
    )
    renderApp('/')
    const needs = await card('overview-needs')
    const line = await within(needs).findByText(
      copy.couldntCheck(copy.needs.source.agents),
      {},
      { timeout: 5000 },
    )
    fail = false
    await userEvent.click(
      within(line.closest('div.flex')! as HTMLElement).getByRole('button', { name: copy.retry }),
    )
    await waitFor(
      () =>
        expect(within(needs).queryByText(copy.couldntCheck(copy.needs.source.agents))).toBeNull(),
      { timeout: 5000 },
    )
  })

  it('previous week down: no agent is called Healthy on missing comparisons', async () => {
    let calls = 0
    server.use(
      http.get('*/api/observability/finops/dashboard', ({ request }) => {
        // The previous-week window is the only call with explicit bounds 7 days back; fail every call with start_time.
        if (new URL(request.url).searchParams.get('start_time')) {
          calls++
          return new HttpResponse('boom', { status: 500 })
        }
        return undefined
      }),
    )
    renderApp('/')
    const counts = await healthCounts()
    expect(calls).toBeGreaterThan(0)
    expect(counts.healthy).toBe(0)
    expect(
      within(await card('overview-health')).getByText(copy.couldntCheck(copy.health.costWhat)),
    ).toBeInTheDocument()
  })
})

describe('a stopped budget on an agent that is not rated', () => {
  // Budgets hidden (no server support for /api/budgets yet, R-L10): un-skip with the commented budget code.
  it.skip('still shows the budget in Needs you', async () => {
    const stopped = budgetMockState().budgets.find((b) => b.agent_id && b.action === 'stop')!
    server.use(
      http.get('*/api/agents', ({ request }) => {
        const u = new URL(request.url)
        const rows = agentsList(seed, {
          limit: u.searchParams.get('limit'),
          offset: u.searchParams.get('offset'),
        })
        return HttpResponse.json(
          rows.map((a) => (a.id === stopped.agent_id ? { ...a, status: 'stopped' } : a)),
        )
      }),
    )
    renderApp('/')
    const needs = await card('overview-needs')
    await waitFor(
      () =>
        expect(
          [...needs.querySelectorAll('[data-kind="budget"]')].some((r) =>
            /over its limit/.test(r.textContent ?? ''),
          ),
        ).toBe(true),
      { timeout: 5000 },
    )
  })
})

describe('the Agents health filter with a failed agent list', () => {
  it('shows the error with Retry, not "no results"', async () => {
    server.use(
      http.get('*/api/agents', ({ request }) => {
        // The catalog pages (limit=50, the catalog query) succeed; the directory's full list fails.
        return new URL(request.url).searchParams.get('limit') === '100'
          ? new HttpResponse('boom', { status: 500 })
          : undefined
      }),
    )
    renderApp('/agents?health=watch')
    expect(
      await screen.findByRole('button', { name: /retry/i }, { timeout: 5000 }),
    ).toBeInTheDocument()
    expect(screen.queryByText(/No agents match/i)).toBeNull()
  })
})

describe('Needs you on OSS (budgets absent)', () => {
  it('shows one card error when every available source failed', async () => {
    const data: NeedsYouData = {
      needs: mergeNeeds({
        requests: { state: 'failed' },
        agents: { state: 'failed' },
        budgets: { state: 'absent' },
        sessions: { state: 'failed' },
      }),
      lastChecked: 0,
      retry: { requests: vi.fn(), agents: vi.fn(), budgets: vi.fn(), sessions: vi.fn() },
      sessions: {} as NeedsYouData['sessions'],
    }
    const router = createRouter({
      routeTree: createRootRoute({ component: () => <NeedsYou data={data} now={0} userId="u1" /> }),
      history: createMemoryHistory({ initialEntries: ['/'] }),
    })
    render(<RouterProvider router={router} />)
    expect(await screen.findByText(copy.couldntLoad(copy.needs.what))).toBeInTheDocument()
  })
})
