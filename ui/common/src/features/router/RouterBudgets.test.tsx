/**
 * R2 budgets on the router page (plans/feat-llm-router.md §5, §5.1; contract R-L10): the table and its states, the
 * Alerts list, the budget sheet (limit, threshold chips, Stop-calls consequence inline, inline remove), Switch to Alert
 * only, the empty, failed and missing-endpoint states, the TokenOps link, focus, and axe.
 */
import { screen, waitFor, within } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import axe from 'axe-core'
import { http, HttpResponse } from 'msw'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { budgetMockState, configureMocks } from '@/mocks/handlers'
import { now, seed, setupPinnedSeed } from '@/test/pinnedSeed'
import { renderApp } from '@/test/renderApp'
import { recordRequestBodies, server } from '@/test/setup'
import { copy } from './copy'

setupPinnedSeed()
// These tests act on agents that follow the default: show that group (it folds by default).
beforeEach(() => localStorage.setItem('openruntime.router.defaultsOpen', 'open'))
afterEach(() => configureMocks({ seed, now, loggedIn: true, routerVariants: [] }))

const table = () => screen.findByRole('table', { name: copy.budgetsTitle })
const ready = async () => {
  await table()
  await waitFor(() => expect(document.querySelectorAll('[data-state]').length).toBeGreaterThan(0))
}
const announcer = () => screen.getByTestId('router-announcer')
const owner = () => budgetMockState().budgets.find((b) => b.scope === 'owner')!
const stopped = () => budgetMockState().budgets.find((b) => b.action === 'stop')!
const row = (id: string) => document.querySelector(`[data-budget="${id}"]`) as HTMLElement
const raise = (id: string) =>
  userEvent.click(within(row(id)).getByRole('button', { name: new RegExp(`^${copy.raiseLimit}`) }))
const ownerTitle = copy.budgetEdit(null)
const money = (v: number) => `$${v.toFixed(2)}`
const esc = (t: string) => t.replace(/[.$]/g, (c) => `\\${c}`)

// Budgets hidden (no server support for /api/budgets yet, R-L10): un-skip with RouterPage's commented budget code.
describe.skip('Budgets section', () => {
  it('shows you first, then agents; used of limit, forecast, state and the action at 100%', async () => {
    renderApp('/router')
    await ready()
    const rows = within(await table())
      .getAllByRole('row')
      .slice(1)
    expect(rows[0]).toHaveTextContent(copy.budgetOwner)
    expect(rows[0]).toHaveTextContent(copy.budgetOwnerHint)
    expect(rows[0]!.querySelector('[data-state]')).toHaveTextContent(copy.budgetWarning(80))
    expect(rows[0]).toHaveTextContent(new RegExp(`\\$[\\d.,]+ of ${esc(money(owner().limit_usd))}`))
    // The owner's month-end range passes the limit.
    expect(rows[0]).toHaveTextContent(/\$[\d.,]+ – \$[\d.,]+/)
    expect(rows[0]).toHaveTextContent(copy.forecastOver)
    const x = row(stopped().id)
    expect(x.querySelector('[data-state]')).toHaveTextContent(copy.budgetExceeded)
    expect(x).toHaveTextContent(copy.actionStop)
    expect(x).toHaveTextContent(copy.stoppedUntil('Apr 1'))
    expect(x).toHaveTextContent(copy.forecastStopped)
    expect(rows.some((r) => r.querySelector('[data-state="ok"]'))).toBe(true)
    expect(screen.getByText(copy.budgetsSubtitle('Apr 1'))).toBeInTheDocument()
    expect(screen.getByText(copy.forecastNote)).toBeInTheDocument()
  })

  it('shows how many calls had no price', async () => {
    renderApp('/router')
    await ready()
    expect(row(owner().id)).toHaveTextContent(/[\d,]+ calls with no price count as \$0/)
  })

  it('the forecast says "Too early" in the first days of the month', async () => {
    const early = new Date('2026-03-02T06:00:00Z').getTime()
    vi.setSystemTime(early)
    configureMocks({ now: () => early })
    try {
      renderApp('/router')
      await ready()
      const ok = budgetMockState().budgets.find((b) => b.action === 'alert' && b.scope === 'agent')!
      expect(row(ok.id)).toHaveTextContent(copy.forecastTooEarly)
    } finally {
      vi.setSystemTime(now())
    }
  })

  it('lists alerts newest first, and View budget opens that budget', async () => {
    renderApp('/router')
    await ready()
    const list = await screen.findByRole('list', { name: copy.alertsTitle })
    const items = within(list).getAllByRole('listitem')
    expect(items[0]).toHaveTextContent(`${copy.alertOwner} crossed 80%`)
    expect(list).toHaveTextContent('calls stopped')
    await userEvent.click(
      within(items[0]!).getByRole('button', { name: new RegExp(copy.viewBudget) }),
    )
    expect(await screen.findByRole('dialog', { name: ownerTitle })).toBeInTheDocument()
  })

  it('an empty alerts list says so', async () => {
    server.use(
      http.get('/api/budgets/alerts', () =>
        HttpResponse.json({ data: [], status_code: 200, message: 'ok' }),
      ),
    )
    renderApp('/router')
    await ready()
    expect(await screen.findByText(copy.noAlerts)).toBeInTheDocument()
  })

  it('a failed alerts read shows its own error with Retry', async () => {
    server.use(
      http.get('/api/budgets/alerts', () => new HttpResponse('internal error', { status: 500 })),
    )
    renderApp('/router')
    await ready()
    const alerts = screen.getByRole('heading', { name: copy.alertsTitle }).closest('section')!
    expect(await within(alerts).findByRole('button', { name: copy.retry })).toBeInTheDocument()
  })

  it('a failed status read keeps the rows with their limits and offers Retry', async () => {
    server.use(
      http.get('/api/budgets/status', () => new HttpResponse('internal error', { status: 500 })),
    )
    renderApp('/router')
    await table()
    expect(await screen.findByText(copy.budgetStatusFailed)).toBeInTheDocument()
    expect(row(owner().id)).toHaveTextContent(`— of ${money(owner().limit_usd)}`)
  })

  it('Switch to Alert only sends the budget with action alert and its updated_at, announces, and keeps focus in the row', async () => {
    const rec = recordRequestBodies()
    renderApp('/router')
    await ready()
    const b = stopped()
    const before = {
      limit_usd: b.limit_usd,
      thresholds: [...b.thresholds],
      expected_updated_at: b.updated_at,
    }
    await userEvent.click(
      within(row(b.id)).getByRole('button', { name: new RegExp(`^${copy.switchToAlert}`) }),
    )
    await waitFor(() => expect(announcer()).toHaveTextContent(/is on Alert only/))
    await rec.flush()
    expect(rec.requests.find((r) => r.method === 'PUT')!.body).toEqual({
      ...before,
      action: 'alert',
    })
    await waitFor(() => expect(row(b.id)).not.toHaveTextContent(copy.stoppedUntil('Apr 1')))
    expect(document.activeElement).toBe(
      within(row(b.id)).getByRole('button', { name: new RegExp(`^${copy.raiseLimit}`) }),
    )
  })

  it('a failed switch announces why and the row keeps Stop calls', async () => {
    server.use(
      http.put('/api/budgets/:id', () => new HttpResponse('internal error', { status: 500 })),
    )
    renderApp('/router')
    await ready()
    const b = stopped()
    await userEvent.click(
      within(row(b.id)).getByRole('button', { name: new RegExp(`^${copy.switchToAlert}`) }),
    )
    await waitFor(() => expect(announcer()).toHaveTextContent(/OpenRuntime said: internal error/))
    expect(row(b.id)).toHaveTextContent(copy.actionStop)
  })

  it('a failed budgets read shows the section error and the rest of the page works', async () => {
    configureMocks({ routerVariants: ['router-budgets-fail'] })
    renderApp('/router')
    expect(await screen.findByText(copy.budgetsFailed)).toBeInTheDocument()
    expect(await screen.findByRole('table', { name: copy.agentsTitle })).toBeInTheDocument()
  })

  it('a server without the budget endpoints (bare 404) says it needs a newer server', async () => {
    server.use(http.get('/api/budgets', () => new HttpResponse(null, { status: 404 })))
    renderApp('/router')
    expect(await screen.findByText(copy.budgetsMissing)).toBeInTheDocument()
    expect(screen.queryByRole('button', { name: copy.createFirstBudget })).toBeNull()
  })
})

describe.skip('budget sheet', () => {
  it('Raise limit opens the sheet on the limit and saves a guarded full replace', async () => {
    const rec = recordRequestBodies()
    renderApp('/router')
    await ready()
    const b = owner()
    const readAt = b.updated_at
    await raise(b.id)
    const sheet = await screen.findByRole('dialog', { name: ownerTitle })
    const limit = within(sheet).getByLabelText(copy.monthlyLimit)
    await waitFor(() => expect(limit).toHaveFocus())
    expect(limit).toHaveValue(b.limit_usd.toFixed(2))
    expect(sheet).toHaveTextContent(
      new RegExp(`used of ${esc(money(b.limit_usd))} this month · resets Apr 1 · router-metered`),
    )
    await userEvent.clear(limit)
    await userEvent.type(limit, '150')
    await userEvent.click(within(sheet).getByRole('button', { name: copy.save }))
    await waitFor(() => expect(sheet).not.toBeInTheDocument())
    await rec.flush()
    expect(rec.requests.find((r) => r.method === 'PUT')!.body).toEqual({
      limit_usd: 150,
      thresholds: [50, 80, 100],
      action: 'alert',
      expected_updated_at: readAt,
    })
    await waitFor(() => expect(row(b.id)).toHaveTextContent('of $150.00'))
    await waitFor(() => expect(announcer()).toHaveTextContent(copy.budgetSaved(null)))
  })

  it('a budget changed elsewhere since the sheet opened is refused, and the sheet says so', async () => {
    renderApp('/router')
    await ready()
    const b = owner()
    await raise(b.id)
    const sheet = await screen.findByRole('dialog', { name: ownerTitle })
    b.updated_at = new Date(now() + 60_000).toISOString()
    await userEvent.click(within(sheet).getByRole('button', { name: copy.save }))
    expect(await within(sheet).findByRole('alert')).toHaveTextContent(copy.budgetChanged)
  })

  it('threshold chips: remove, add, no duplicates, a typed mark kept on Save; Stop calls locks 100% and states the consequence', async () => {
    const rec = recordRequestBodies()
    renderApp('/router')
    await ready()
    await raise(owner().id)
    const sheet = await screen.findByRole('dialog', { name: ownerTitle })
    await userEvent.click(within(sheet).getByRole('button', { name: copy.removeThreshold(50) }))
    await userEvent.click(within(sheet).getByRole('button', { name: copy.removeThreshold(100) }))
    const add = within(sheet).getByRole('textbox', { name: copy.addThreshold })
    await userEvent.type(add, '80')
    expect(within(sheet).getByRole('button', { name: copy.addThreshold })).toBeDisabled()
    await userEvent.clear(add)
    expect(within(sheet).queryByTestId('stop-consequence')).toBeNull()
    const stop = within(sheet).getByRole('radio', { name: new RegExp(`^${copy.actionStop}`) })
    await userEvent.click(stop)
    // 100% comes back and can't be removed while Stop calls is on.
    expect(within(sheet).getByText('100%')).toBeInTheDocument()
    expect(within(sheet).queryByRole('button', { name: copy.removeThreshold(100) })).toBeNull()
    expect(within(sheet).getByText(copy.stopKeeps100)).toBeInTheDocument()
    const consequence = within(sheet).getByTestId('stop-consequence')
    expect(consequence).toHaveTextContent(copy.stopConsequenceOwner('Apr 1'))
    expect(stop.getAttribute('aria-describedby')).toBe(consequence.id)
    // Typed but not added: Save keeps it.
    await userEvent.type(add, '90')
    await userEvent.click(within(sheet).getByRole('button', { name: copy.save }))
    await waitFor(() => expect(sheet).not.toBeInTheDocument())
    await rec.flush()
    expect(rec.requests.find((r) => r.method === 'PUT')!.body).toMatchObject({
      thresholds: [80, 90, 100],
      action: 'stop',
    })
  })

  it('an agent budget’s Stop-calls consequence names the agent and the reset date', async () => {
    renderApp('/router')
    await ready()
    const b = stopped()
    const name = row(b.id).querySelector('td')!.textContent!
    await raise(b.id)
    const sheet = await screen.findByRole('dialog', { name: copy.budgetEdit(name) })
    expect(within(sheet).getByTestId('stop-consequence')).toHaveTextContent(
      copy.stopConsequenceAgent(name, 'Apr 1'),
    )
  })

  it('removing a budget confirms inline (focus moves into it, and back on Cancel), then deletes it', async () => {
    const rec = recordRequestBodies()
    renderApp('/router')
    await ready()
    const b = stopped()
    await raise(b.id)
    const sheet = await screen.findByRole('dialog')
    await userEvent.click(within(sheet).getByRole('button', { name: copy.deleteBudget }))
    expect(screen.queryByRole('alertdialog')).toBeNull()
    await waitFor(() => expect(within(sheet).getByText(/^Remove the budget for /)).toHaveFocus())
    const confirmBox = within(sheet).getByText(/^Remove the budget for /).parentElement!
    await userEvent.click(within(confirmBox).getByRole('button', { name: copy.cancel }))
    await waitFor(() =>
      expect(within(sheet).getByRole('button', { name: copy.deleteBudget })).toHaveFocus(),
    )
    await userEvent.click(within(sheet).getByRole('button', { name: copy.deleteBudget }))
    await userEvent.click(within(sheet).getAllByRole('button', { name: copy.deleteBudget }).at(-1)!)
    await waitFor(() => expect(sheet).not.toBeInTheDocument())
    await rec.flush()
    expect(
      rec.requests.some((r) => r.method === 'DELETE' && r.url.pathname === `/api/budgets/${b.id}`),
    ).toBe(true)
    await waitFor(() => expect(row(b.id)).toBeNull())
  })

  it('a failed remove keeps the sheet open with the error and the row in place', async () => {
    server.use(
      http.delete('/api/budgets/:id', () => new HttpResponse('budget not found', { status: 404 })),
    )
    renderApp('/router')
    await ready()
    const b = stopped()
    await raise(b.id)
    const sheet = await screen.findByRole('dialog')
    await userEvent.click(within(sheet).getByRole('button', { name: copy.deleteBudget }))
    await userEvent.click(within(sheet).getAllByRole('button', { name: copy.deleteBudget }).at(-1)!)
    expect(await within(sheet).findByRole('alert')).toBeInTheDocument()
    expect(row(b.id)).not.toBeNull()
  })

  it('an invalid limit is flagged and nothing is sent', async () => {
    const rec = recordRequestBodies()
    renderApp('/router')
    await ready()
    await raise(owner().id)
    const sheet = await screen.findByRole('dialog')
    const limit = within(sheet).getByLabelText(copy.monthlyLimit)
    await userEvent.clear(limit)
    await userEvent.type(limit, '0.004')
    await userEvent.click(within(sheet).getByRole('button', { name: copy.save }))
    expect(within(sheet).getByText(copy.limitPositive)).toBeInTheDocument()
    expect(limit).toHaveAttribute('aria-invalid', 'true')
    await rec.flush()
    expect(rec.requests.filter((r) => r.method === 'PUT')).toHaveLength(0)
  })
})

describe.skip('new budgets', () => {
  it('the empty state creates one with the defaults: you first, Alert only, 50/80/100', async () => {
    configureMocks({ routerVariants: ['router-budgets-empty'] })
    const rec = recordRequestBodies()
    renderApp('/router')
    await userEvent.click(await screen.findByRole('button', { name: copy.createFirstBudget }))
    const sheet = await screen.findByRole('dialog', { name: copy.budgetNew })
    expect(within(sheet).getByLabelText(copy.budgetScope)).toHaveTextContent(copy.budgetOwnerOption)
    await userEvent.type(within(sheet).getByLabelText(copy.monthlyLimit), '120')
    await userEvent.click(within(sheet).getByRole('button', { name: copy.save }))
    await waitFor(() => expect(sheet).not.toBeInTheDocument())
    await rec.flush()
    expect(
      rec.requests.find((r) => r.method === 'POST' && r.url.pathname === '/api/budgets')!.body,
    ).toEqual({
      scope: 'owner',
      agent_id: null,
      limit_usd: 120,
      thresholds: [50, 80, 100],
      action: 'alert',
    })
    await ready()
    expect(within(await table()).getAllByRole('row')[1]).toHaveTextContent('of $120.00')
    await waitFor(() => expect(announcer()).toHaveTextContent(copy.budgetSaved(null)))
  })

  it('an agent budget: only open scopes are offered, the consequence names the agent, and the POST carries agent_id', async () => {
    const rec = recordRequestBodies()
    renderApp('/router')
    await ready()
    await userEvent.click(screen.getByRole('button', { name: copy.newBudget }))
    const sheet = await screen.findByRole('dialog', { name: copy.budgetNew })
    // The scope is a shadcn Select: read its options from the open listbox, by name (agent names come from the table).
    await userEvent.click(within(sheet).getByLabelText(copy.budgetScope))
    const options = (await screen.findAllByRole('option')).map((o) => o.textContent!)
    await userEvent.keyboard('{Escape}')
    const agentName = (id: string | null) =>
      document.querySelector(`[data-agent="${id}"] a`)!.textContent!
    const idOf = (name: string) =>
      [...document.querySelectorAll('[data-agent]')]
        .find((r) => r.querySelector('a')!.textContent === name)!
        .getAttribute('data-agent')
    expect(options).not.toContain(copy.budgetOwnerOption)
    for (const b of budgetMockState().budgets.filter((x) => x.scope === 'agent'))
      expect(options).not.toContain(agentName(b.agent_id))
    const pick = { textContent: options[0]!, value: idOf(options[0]!) }
    expect(within(sheet).getByText(copy.budgetScopeAgentHint)).toBeInTheDocument()
    await userEvent.type(within(sheet).getByLabelText(copy.monthlyLimit), '10')
    await userEvent.click(
      within(sheet).getByRole('radio', { name: new RegExp(`^${copy.actionStop}`) }),
    )
    expect(within(sheet).getByTestId('stop-consequence')).toHaveTextContent(
      copy.stopConsequenceAgent(pick.textContent!, 'Apr 1'),
    )
    await userEvent.click(within(sheet).getByRole('button', { name: copy.save }))
    await waitFor(() => expect(sheet).not.toBeInTheDocument())
    await rec.flush()
    expect(rec.requests.find((r) => r.method === 'POST')!.body).toMatchObject({
      scope: 'agent',
      agent_id: pick.value,
      action: 'stop',
      thresholds: [50, 80, 100],
    })
  })

  it('with every scope taken, the sheet says so and Save is off', async () => {
    const { queryClient } = renderApp('/router')
    await ready()
    await screen.findByRole('table', { name: copy.agentsTitle })
    // Give every owned agent a budget in the mock, then re-read the list.
    const s = budgetMockState()
    const b0 = s.budgets[0]!
    for (const el of document.querySelectorAll('[data-agent]')) {
      const id = el.getAttribute('data-agent')!
      if (!s.budgets.some((b) => b.agent_id === id))
        s.budgets.push({ ...b0, id: `x-${id}`, scope: 'agent', agent_id: id })
    }
    await queryClient.refetchQueries({ queryKey: ['router', 'budgets'] })
    await waitFor(() =>
      expect(document.querySelectorAll('[data-budget]').length).toBe(s.budgets.length),
    )
    await userEvent.click(screen.getByRole('button', { name: copy.newBudget }))
    const sheet = await screen.findByRole('dialog', { name: copy.budgetNew })
    expect(within(sheet).getByText(copy.budgetNoScopes)).toBeInTheDocument()
    expect(within(sheet).getByRole('button', { name: copy.save })).toBeDisabled()
  })
})

describe.skip('TokenOps link, deep link and a11y', () => {
  it('TokenOps links to the router page’s budgets', async () => {
    renderApp('/tokenops')
    expect(
      await screen.findByRole('link', { name: 'Budgets are on the LLM router page' }),
    ).toHaveAttribute('href', '/router#router-budgets')
  })

  it('arriving at #router-budgets scrolls to the section once it has loaded', async () => {
    const spy = vi.spyOn(Element.prototype, 'scrollIntoView')
    window.history.replaceState(null, '', '/router#router-budgets')
    try {
      renderApp('/router#router-budgets')
      await table()
      await waitFor(() =>
        expect(spy.mock.contexts.some((el) => (el as Element).id === 'router-budgets')).toBe(true),
      )
    } finally {
      window.history.replaceState(null, '', '/')
    }
  })

  const check = async () => {
    const result = await axe.run(document.body, {
      rules: { 'color-contrast': { enabled: false }, region: { enabled: false } },
    })
    expect(
      result.violations.map((v) => `${v.id}: ${v.nodes.map((n) => n.target.join(' ')).join(', ')}`),
    ).toEqual([])
  }
  it('axe: the Budgets and Alerts sections, and the budget sheet with Stop calls on', async () => {
    renderApp('/router')
    await ready()
    await screen.findByRole('list', { name: copy.alertsTitle })
    await check()
    await raise(stopped().id)
    await screen.findByTestId('stop-consequence')
    await check()
  })
})

// Regression: ISSUE-201..202 (/qa 2026-09-28, .gstack/qa-reports/qa-report-localhost-2026-09-28-r2.md): spend
// jumping after Switch to Alert only; the TokenOps deep link landing above the Budgets section.
describe.skip('switching and deep-link regressions', () => {
  it('switching a stopped budget to Alert only doesn’t count the calls it refused', async () => {
    renderApp('/router')
    await screen.findByRole('table', { name: copy.budgetsTitle })
    const b = budgetMockState().budgets.find((x) => x.action === 'stop')!
    const row = () => document.querySelector(`[data-budget="${b.id}"]`) as HTMLElement
    await waitFor(() => expect(row()).toHaveTextContent(copy.stoppedUntil('Apr 1')))
    const usedBefore = /\$([\d.,]+) of/.exec(row().textContent!)![1]
    await userEvent.click(
      within(row()).getByRole('button', { name: new RegExp(`^${copy.switchToAlert}`) }),
    )
    await waitFor(() => expect(row()).not.toHaveTextContent(copy.stoppedUntil('Apr 1')))
    expect(/\$([\d.,]+) of/.exec(row().textContent!)![1]).toBe(usedBefore)
  })

  it('#router-budgets is scrolled to again once the agents rows above it settle (they change its position)', async () => {
    let release!: () => void
    const gate = new Promise<void>((r) => {
      release = r
    })
    server.use(
      http.get('/api/agents/:id/llm-config', async () => {
        await gate
      }),
    )
    const spy = vi.spyOn(Element.prototype, 'scrollIntoView')
    window.history.replaceState(null, '', '/router#router-budgets')
    try {
      renderApp('/router#router-budgets')
      await screen.findByRole('table', { name: copy.budgetsTitle })
      // Real wait, kept: the router's own hash scroll may or may not run, so there is no signal to wait on; this lets
      // it land before `before` is counted, so the later increase can only be the settle scroll.
      await new Promise((r) => setTimeout(r, 50))
      // The router's own hash scroll may already have run; count only the scrolls after the rows settle.
      const toBudgets = () =>
        spy.mock.contexts.filter((el) => (el as Element).id === 'router-budgets').length
      const before = toBudgets()
      release()
      await waitFor(() => expect(toBudgets()).toBeGreaterThan(before))
    } finally {
      window.history.replaceState(null, '', '/')
    }
  })
})
