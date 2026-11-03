/**
 * Needs you rendered on its own with hand-built data (ship coverage audit): the states and links the page tests can't
 * reach deterministically from the seed (plans/feat-overview.md §6-§10). The home (PR2) keeps this card; the Overview's
 * other cards were retired with the Overview page (plans/feat-agent-centric.md PR2a).
 */
import { render, screen, within } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import {
  createMemoryHistory,
  createRootRoute,
  createRouter,
  RouterProvider,
} from '@tanstack/react-router'
import type { ReactNode } from 'react'
import { describe, expect, it, vi } from 'vitest'
import { readRailView } from '@nasiko/features/chat/rememberTarget'
import type { HarnessSummary, NeedsYou as NeedsYouData, Spend as SpendData } from './api'
import { Harnesses, HarnessesLine } from './components/Harnesses'
import { HarnessCell } from './components/SetUpNow'
import { Headline } from './components/Headline'
import { MonthBar } from './components/MonthBar'
import { NeedsYou } from './components/NeedsYou'
import { Spend } from './components/Spend'
import { copy } from './copy'
import { mergeNeeds, type NeedsInput } from './needs'

function renderInRouter(node: ReactNode) {
  const router = createRouter({
    routeTree: createRootRoute({ component: () => <>{node}</> }),
    history: createMemoryHistory({ initialEntries: ['/'] }),
  })
  return render(<RouterProvider router={router} />)
}

const retries = () => ({ requests: vi.fn(), agents: vi.fn(), budgets: vi.fn(), sessions: vi.fn() })
const needsData = (input: NeedsInput, retry = retries()): NeedsYouData => ({
  needs: mergeNeeds(input),
  lastChecked: 0,
  retry,
  sessions: {} as NeedsYouData['sessions'],
  waiting: {} as NeedsYouData['waiting'],
})

describe('Needs you rows', () => {
  it('links every row kind to the page that resolves it', async () => {
    renderInRouter(
      <NeedsYou
        userId="u1"
        now={Date.parse('2026-09-29T12:00:00Z')}
        data={needsData({
          requests: {
            state: 'ok',
            value: {
              chats: [
                {
                  sessionId: 's1',
                  firstId: 'r1',
                  kind: 'input_required',
                  count: 2,
                  chatTitle: 'Deploy chat',
                  at: '2026-09-29T11:00:00Z',
                },
              ],
              outside: 1,
            },
          },
          agents: {
            state: 'ok',
            value: [
              { id: 'a1', name: 'Crashy', reason: 'crashed', budget: false },
              { id: 'a2', name: 'Spender', reason: copy.reason.budgetStopped, budget: true },
            ],
          },
          budgets: {
            state: 'ok',
            value: [
              {
                id: 'b1',
                label: copy.needs.yourBudget,
                agentId: null,
                crossed: 80,
                state: 'warning',
                over: null,
                stopped: false,
                stops: false,
                runsOutOn: null,
                resetsAt: null,
                period: 'month' as const,
                covers: 'routed_calls' as const,
              },
              {
                id: 'b2',
                label: 'Helper',
                agentId: 'a3',
                crossed: null,
                state: 'exceeded',
                over: null,
                stopped: false,
                stops: false,
                runsOutOn: null,
                resetsAt: null,
                period: 'month' as const,
                covers: 'routed_calls' as const,
              },
            ],
          },
          sessions: {
            state: 'ok',
            value: { failed: 3, agents: ['Crashy', 'Unknown agent'] },
          },
        })}
      />,
    )
    const list = await screen.findByRole('list', { name: copy.needs.title })
    const href = (name: string | RegExp) =>
      within(list).getByRole('link', { name }).getAttribute('href')
    expect(within(list).getByText('Input needed (+1 more)')).toBeInTheDocument()
    expect(href(copy.needs.reviewLabel('Deploy chat'))).toBe('/chat/s1')
    expect(href(copy.needs.openWaiting)).toBe('/chat')
    expect(href(copy.needs.openAgentLabel('Crashy'))).toBe('/agents/a1')
    // A stopped agent budget opens the agent's Budget section on its hub (B5).
    expect(href(copy.needs.agentBudgetLabel('Spender'))).toBe('/agents/a2#hub-budget')
    expect(within(list).getByText('Your monthly budget is at 80%')).toBeInTheDocument()
    expect(within(list).getByText('Helper budget is over its limit')).toBeInTheDocument()
    expect(href(copy.needs.agentBudgetLabel('Helper'))).toBe('/agents/a3#hub-budget')
    expect(within(list).getByText(copy.needs.sessions(3))).toBeInTheDocument()
    expect(href(copy.needs.seeFailing)).toMatch(/^\/sessions\?(?=.*preset=7d)(?=.*lane=failing)/)
    // Severity is said by an icon with a label, most urgent sources first.
    expect(
      within(list)
        .getAllByRole('img')
        .map((i) => i.getAttribute('aria-label')),
    ).toEqual([
      copy.needs.waiting,
      copy.needs.waiting,
      copy.rating.action,
      copy.rating.action,
      copy.rating.watch,
      copy.rating.watch,
      copy.rating.action,
    ])
  })

  it('shows one card error when every source failed, and Retry retries all four', async () => {
    const retry = retries()
    renderInRouter(
      <NeedsYou
        userId="u1"
        now={0}
        data={needsData(
          {
            requests: { state: 'failed' },
            agents: { state: 'failed' },
            budgets: { state: 'failed' },
            sessions: { state: 'failed' },
          },
          retry,
        )}
      />,
    )
    expect(await screen.findByText(copy.couldntLoad(copy.needs.what))).toBeInTheDocument()
    expect(screen.queryByText(copy.couldntCheck(copy.needs.source.requests))).toBeNull()
    await userEvent.click(screen.getByRole('button', { name: copy.retry }))
    for (const fn of Object.values(retry)) expect(fn).toHaveBeenCalledOnce()
    expect(document.activeElement).toBe(
      screen.getByRole('heading', { level: 2, name: copy.needs.title }),
    )
  })
})

describe('shadcn conversion review (2026-09-29)', () => {
  it('a failed card is not a live region, so a server outage does not fire five alerts at once', async () => {
    renderInRouter(
      <NeedsYou
        userId="u1"
        now={0}
        data={needsData({
          requests: { state: 'failed' },
          agents: { state: 'failed' },
          budgets: { state: 'failed' },
          sessions: { state: 'failed' },
        })}
      />,
    )
    expect(await screen.findByText(copy.couldntLoad(copy.needs.what))).toBeInTheDocument()
    expect(screen.queryByRole('alert')).toBeNull()
  })
})

// Moved from the retired Overview page tests (plans/feat-agent-centric.md PR2a; /review of PR2a: testing).
describe('Needs you on its own', () => {
  it('the outside-Chat row opens Chat on the Waiting view', async () => {
    renderInRouter(
      <NeedsYou
        userId="u1"
        now={0}
        data={needsData({
          requests: { state: 'ok', value: { chats: [], outside: 2 } },
          agents: { state: 'ok', value: [] },
          budgets: { state: 'absent' },
          sessions: { state: 'ok', value: { failed: 0, agents: [] } },
        })}
      />,
    )
    await userEvent.click(await screen.findByRole('link', { name: copy.needs.openWaiting }))
    expect(readRailView('u1')).toBe('waiting')
  })

  it('on OSS (budgets absent) shows one card error when every available source failed', async () => {
    renderInRouter(
      <NeedsYou
        userId="u1"
        now={0}
        data={needsData({
          requests: { state: 'failed' },
          agents: { state: 'failed' },
          budgets: { state: 'absent' },
          sessions: { state: 'failed' },
        })}
      />,
    )
    expect(await screen.findByText(copy.couldntLoad(copy.needs.what))).toBeInTheDocument()
  })
})

// The cards Weave still renders (genui/catalog/views/overview.tsx) after the Overview page itself retired.
describe('Headline', () => {
  it('says it is checking until either sentence is known', () => {
    render(<Headline narrative={{ money: null, attention: null }} nothing={false} />)
    expect(screen.getByTestId('overview-headline')).toHaveTextContent(copy.headline.checking)
    expect(screen.queryByTestId('headline-money')).toBeNull()
  })
})

const day = (date: string, spend: number) => ({
  iso: `${date}T00:00:00.000Z`,
  label: date.slice(5),
  spend,
  operations: 0,
  p50: null,
  p95: null,
  p99: null,
  topAgent: null,
  topAgentSpend: null,
})

const spendData = (over: Partial<SpendData> = {}): SpendData => ({
  summary: {
    mtd: 120,
    elapsedDays: 3,
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
  tokens: null,
  days: [day('2026-09-28', 20), day('2026-09-29', 100)],
  stack: null,
  stackPending: false,
  spike: null,
  drivers: [],
  other: null,
  driversError: null,
  unpriced: false,
  latency: null,
  retry: vi.fn(),
  ...over,
})

describe('Spend', () => {
  it('says "no spend yet" at $0 with no earlier spend, and the forecast day before the forecast shows', async () => {
    const empty = spendData({
      summary: { ...spendData().summary!, mtd: 0 },
      totals: { spend: 0, runs: 0, previous: null },
      days: [day('2026-09-29', 0)],
    })
    const { unmount } = renderInRouter(
      <>
        <MonthBar spend={empty} />
        <Spend now={Date.parse('2026-03-20T15:00:00Z')} spend={empty} />
      </>,
    )
    expect(await screen.findByText(copy.spend.noSpendYet)).toBeInTheDocument()
    expect(screen.queryByRole('figure')).toBeNull()
    expect(screen.getByText(copy.spend.noDrivers(30))).toBeInTheDocument()
    unmount()
    renderInRouter(<MonthBar spend={spendData()} />)
    expect(await screen.findByTestId('overview-forecast')).toHaveTextContent(
      /^Forecast from day \d+$/,
    )
  })

  it("links the spike to that day's costliest sessions, and a failed driver read offers Retry", async () => {
    const retry = vi.fn()
    renderInRouter(
      <Spend
        now={Date.parse('2026-03-20T15:00:00Z')}
        spend={spendData({
          spike: { date: '2026-09-29', spend: 100, factor: 5 },
          driversError: new Error('boom'),
          retry,
        })}
      />,
    )
    const link = await screen.findByRole('link', { name: new RegExp(copy.spend.seeSessions) })
    expect(link.getAttribute('href')).toMatch(/^\/sessions\?(?=.*day=2026-09-29)(?=.*sort=cost)/)
    expect(screen.getByText(copy.couldntCheck(copy.spend.driversWhat))).toBeInTheDocument()
    await userEvent.click(screen.getByRole('button', { name: copy.retry }))
    expect(retry).toHaveBeenCalledOnce()
  })
})

describe('Spend chart', () => {
  const stacked = (over: Partial<SpendData> = {}) =>
    spendData({
      drivers: [
        {
          id: 'a1',
          name: 'Code Reviewer',
          kind: 'agent',
          cost: 60,
          sharePct: 60,
          deltaPct: null,
          deltaKind: 'unavailable',
        } as SpendData['drivers'][number],
      ],
      stack: {
        series: [{ key: 's0', id: 'a1', name: 'Code Reviewer' }],
        rows: [
          { iso: '2026-09-28T00:00:00.000Z', label: '09-28', total: 20, s0: 5, other: 15 },
          { iso: '2026-09-29T00:00:00.000Z', label: '09-29', total: 100, s0: 55, other: 45 },
        ],
      },
      ...over,
    })

  it('waits for the drivers rather than redraw as a stack', async () => {
    renderInRouter(
      <Spend now={Date.parse('2026-09-30T12:00:00Z')} spend={stacked({ stackPending: true })} />,
    )
    await screen.findByText('$120.00')
    expect(screen.queryByRole('figure')).toBeNull()
  })

  it('keys each driver to its color, and the Table view lists the stacked days by keyboard', async () => {
    renderInRouter(<Spend now={Date.parse('2026-09-30T12:00:00Z')} spend={stacked()} />)
    const fig = await screen.findByRole('figure')
    expect(fig).toHaveAccessibleName(/\$120\.00 in all, peaking on 09-29 at \$100\.00/)
    expect(fig).toHaveTextContent(copy.spend.otherNote)
    const toggle = within(fig).getByRole('button', { name: copy.spend.showTable })
    toggle.focus()
    await userEvent.keyboard('{Enter}')
    const table = within(fig).getByRole('table')
    expect(
      within(table)
        .getAllByRole('columnheader')
        .map((h) => h.textContent),
    ).toEqual([copy.spend.colDay, 'Code Reviewer', copy.spend.other, copy.spend.colTotal])
    expect(within(table).getByRole('row', { name: /09-29/ })).toHaveTextContent(
      '$55.00$45.00$100.00',
    )
    await userEvent.click(within(fig).getByRole('button', { name: copy.spend.showChart }))
    expect(within(fig).queryByRole('table')).toBeNull()
  })

  it('draws the fleet total alone when a driver series is missing', async () => {
    renderInRouter(
      <Spend now={Date.parse('2026-09-30T12:00:00Z')} spend={stacked({ stack: null })} />,
    )
    const fig = await screen.findByRole('figure')
    expect(fig).not.toHaveTextContent(copy.spend.otherNote)
    await userEvent.click(within(fig).getByRole('button', { name: copy.spend.showTable }))
    expect(within(fig).getAllByRole('columnheader')).toHaveLength(2)
  })
  it("lists Other, and a row's eye hides its series but never the last one", async () => {
    renderInRouter(
      <Spend
        now={Date.parse('2026-09-30T12:00:00Z')}
        spend={stacked({
          other: { count: 9, cost: 40, sharePct: 40, deltaPct: 10, unavailable: false },
        })}
      />,
    )
    const rows = await screen.findAllByTestId('spend-driver')
    expect(rows).toHaveLength(2)
    expect(rows[1]).toHaveTextContent(copy.spend.otherAgents(9))
    const eye = within(rows[0]!).getByRole('button', { name: copy.spend.show('Code Reviewer') })
    expect(eye).toHaveAttribute('aria-pressed', 'true')
    await userEvent.click(eye)
    expect(eye).toHaveAttribute('aria-pressed', 'false')
    // Other is now the only series drawn: it can't be hidden too.
    expect(within(rows[1]!).getByRole('button')).toBeDisabled()
  })
})

describe('Coding harnesses', () => {
  const data = (over: Partial<HarnessSummary>): HarnessSummary => ({
    isPending: false,
    error: null,
    ownOnly: false,
    notVisible: false,
    connected: 0,
    activeDevs: null,
    harnesses: [],
    retry: vi.fn(),
    ...over,
  })

  it('offers to connect one when none is connected, says so when nothing is visible, and marks unpriced cost', async () => {
    const { unmount } = renderInRouter(<Harnesses data={data({})} days={30} />)
    expect(await screen.findByRole('link', { name: copy.harnesses.connect })).toHaveAttribute(
      'href',
      '/harnesses',
    )
    unmount()
    const second = renderInRouter(<Harnesses data={data({ notVisible: true })} days={30} />)
    expect(await screen.findByText(copy.harnesses.notVisible)).toBeInTheDocument()
    expect(screen.queryByRole('link', { name: copy.harnesses.connect })).toBeNull()
    second.unmount()
    renderInRouter(
      <Harnesses
        data={data({
          connected: 2,
          activeDevs: 1,
          harnesses: [
            { id: 'claude-code', cost: 12, unpriced: false },
            { id: 'mystery', cost: 3, unpriced: true },
          ],
        })}
        days={30}
      />,
    )
    const tile = await screen.findByTestId('overview-harnesses')
    expect(tile).toHaveTextContent(`2 ${copy.kpi.connected}`)
    expect(tile).toHaveTextContent(copy.kpi.devs(1))
    // The unpriced harness has no share of the priced cost.
    expect(tile).toHaveTextContent('100.0%$12.00')
    const rows = screen.getAllByRole('listitem')
    expect(rows[1]).toHaveTextContent(`mystery${copy.harnesses.unpriced}`)
  })
  it('keeps the one-line Harnesses (Weave’s view) to one sentence per state, with only priced cost', async () => {
    const { unmount } = renderInRouter(<HarnessesLine data={data({})} days={30} />)
    const line = await screen.findByTestId('overview-harnesses')
    expect(line).toHaveTextContent(`${copy.harnesses.noneYet}·${copy.harnesses.connect}.`)
    unmount()
    const second = renderInRouter(
      <HarnessesLine
        data={data({
          connected: 2,
          ownOnly: true,
          harnesses: [
            { id: 'claude-code', cost: 12, unpriced: false },
            { id: 'mystery', cost: 3, unpriced: true },
          ],
        })}
        days={30}
      />,
    )
    expect(await screen.findByTestId('overview-harnesses')).toHaveTextContent(
      `${copy.harnesses.connected(2)}(${copy.kpi.yourUsage})·${copy.harnesses.lineCost('$12.00', 30)}·${copy.harnesses.open}`,
    )
    second.unmount()
    const retry = vi.fn()
    renderInRouter(<HarnessesLine data={data({ error: new Error('x'), retry })} days={30} />)
    await userEvent.click(await screen.findByRole('button', { name: copy.retry }))
    expect(retry).toHaveBeenCalled()
  })
  it('gives the first run harness cell one status per state, with only priced cost', async () => {
    const { unmount } = renderInRouter(<HarnessCell data={data({})} days={30} />)
    const cell = await screen.findByTestId('overview-harnesses')
    expect(cell).toHaveTextContent(`${copy.harnesses.title}${copy.setUpNow.noHarnesses}`)
    expect(within(cell).getByRole('link')).toHaveAttribute('href', '/harnesses')
    unmount()
    const second = renderInRouter(
      <HarnessCell
        data={data({
          connected: 2,
          ownOnly: true,
          harnesses: [
            { id: 'claude-code', cost: 12, unpriced: false },
            { id: 'mystery', cost: 3, unpriced: true },
          ],
        })}
        days={30}
      />,
    )
    expect(await screen.findByTestId('overview-harnesses')).toHaveTextContent(
      `${copy.harnesses.connected(2)} (${copy.kpi.yourUsage}) · ${copy.harnesses.lineCost('$12.00', 30)}`,
    )
    second.unmount()
    const retry = vi.fn()
    renderInRouter(<HarnessCell data={data({ error: new Error('x'), retry })} days={30} />)
    await userEvent.click(await screen.findByRole('button', { name: copy.retry }))
    expect(retry).toHaveBeenCalled()
  })
})
