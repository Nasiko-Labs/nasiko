/**
 * Overview cards rendered on their own with hand-built data (ship coverage audit): the states and links the page tests
 * can't reach deterministically from the seed (plans/feat-overview.md §6-§10).
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
import type {
  BudgetCard,
  BudgetLine,
  HarnessSummary,
  NeedsYou as NeedsYouData,
  Spend as SpendData,
} from './api'
import { Budget } from './components/Budget'
import { Harnesses, HarnessesLine } from './components/Harnesses'
import { Headline } from './components/Headline'
import { NeedsYou } from './components/NeedsYou'
import { MonthBar } from './components/MonthBar'
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
              { id: 'b1', label: copy.needs.yourBudget, crossed: 80, state: 'warning' },
              { id: 'b2', label: 'Helper', crossed: null, state: 'exceeded' },
            ],
          },
          sessions: {
            state: 'ok',
            value: { failed: 3, checked: 25, agents: ['Crashy', 'Unknown agent'] },
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
    expect(href(copy.needs.budgetsLabel('Spender'))).toBe('/router#router-budgets')
    expect(within(list).getByText('Your monthly budget is at 80%')).toBeInTheDocument()
    expect(within(list).getByText('Helper budget is over its limit')).toBeInTheDocument()
    expect(href(copy.needs.budgetsLabel('Helper'))).toBe('/router#router-budgets')
    expect(within(list).getByText(copy.needs.sessions(3, 25))).toBeInTheDocument()
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
  p95: null,
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
  days: [day('2026-09-28', 20), day('2026-09-29', 100)],
  stack: null,
  stackPending: false,
  spike: null,
  drivers: [],
  other: null,
  driversError: null,
  unpriced: false,
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

  it('keys each driver to its colour, and the Table view lists the stacked days by keyboard', async () => {
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

describe('Budgets', () => {
  const line = (over: Partial<BudgetLine> = {}): BudgetLine => ({
    id: 'b1',
    name: null,
    limit: 100,
    used: 85,
    pct: 85,
    state: 'warning',
    stopped: false,
    thresholds: [50, 80, 100],
    action: 'alert',
    forecast: { show: true, hidden: null, low: 110, high: 120, overLimit: true },
    daysLeft: 1,
    ...over,
  })
  const card = (over: Partial<BudgetCard> = {}): BudgetCard => ({
    absent: false,
    isPending: false,
    error: null,
    own: line(),
    agents: [],
    retry: vi.fn(),
    ...over,
  })

  it('shows the own budget in words, with the forecast past the limit flagged', async () => {
    renderInRouter(<Budget data={card()} month="September 2026" />)
    const c = await screen.findByTestId('overview-budget')
    expect(c).toHaveTextContent('$85.00 of $100.00')
    expect(c).toHaveTextContent(copy.budget.state('warning', false))
    expect(c).toHaveTextContent('85.0% used')
    expect(c).toHaveTextContent('1 day left')
    expect(c).toHaveTextContent(`$110.00–$120.00, ${copy.budget.overLimit}`)
    expect(c).toHaveTextContent('50%, 80%, 100%')
    expect(c).toHaveTextContent(copy.budget.alertOnly)
    expect(within(c).getByRole('link', { name: /Budgets/ })).toHaveAttribute(
      'href',
      '/router#router-budgets',
    )
  })

  it('lists agent budgets with a stopped one said as a word, and offers to set one when there are none', async () => {
    const { unmount } = renderInRouter(
      <Budget
        data={card({
          own: null,
          agents: [line({ id: 'b2', name: 'Code Reviewer', state: 'exceeded', stopped: true })],
        })}
        month="September 2026"
      />,
    )
    const c = await screen.findByTestId('overview-budget')
    expect(c).toHaveTextContent(
      `Code Reviewer$85.00 of $100.00${copy.budget.state('exceeded', true)}`,
    )
    expect(c).not.toHaveTextContent(copy.budget.forecast)
    unmount()
    renderInRouter(<Budget data={card({ own: null })} month="September 2026" />)
    expect(await screen.findByRole('link', { name: copy.budget.set })).toHaveAttribute(
      'href',
      '/router#router-budgets',
    )
  })

  it('fails as a whole card with Retry', async () => {
    const retry = vi.fn()
    renderInRouter(
      <Budget data={card({ error: new Error('boom'), retry })} month="September 2026" />,
    )
    await userEvent.click(await screen.findByRole('button', { name: copy.retry }))
    expect(retry).toHaveBeenCalledOnce()
  })
})

describe('Harnesses', () => {
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
  it('keeps the first run line to one sentence per state, with only priced cost', async () => {
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
})
