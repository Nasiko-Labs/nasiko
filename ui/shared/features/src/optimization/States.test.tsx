/**
 * The /optimization states, one test per row of plans/feat-optimization-page.md §7 not already covered by the block
 * tests (T6: R2B, R2C, R2E). The Workspace row comes with T7.
 */
import { screen, waitFor, within } from '@testing-library/react'
import { delay, http, HttpResponse } from 'msw'
import { afterEach, describe, expect, it } from 'vitest'
import { configureMocks } from '@nasiko/mocks/handlers'
import { now, seed, setupPinnedSeed } from '@nasiko/testing/pinnedSeed'
import { renderApp } from '@nasiko/testing/renderApp'
import { server } from '@nasiko/testing/setup'
import { copy } from './copy'

setupPinnedSeed()
afterEach(() => configureMocks({ seed, now, loggedIn: true, variant: null, superuser: null }))

const T = { timeout: 12_000 }
const SAVINGS = '/api/observability/finops/context-savings'
const block = async (title: string) =>
  (await screen.findByRole('heading', { level: 2, name: title }, T)).closest(
    'section',
  ) as HTMLElement

describe('loading', () => {
  it('the lead is busy with a heading skeleton until its read lands (§7 Lead)', async () => {
    server.use(
      http.get(SAVINGS, async () => {
        await delay(400)
      }),
    )
    renderApp('/optimization')
    const h = await screen.findByRole('heading', { level: 2, name: copy.lead.chart.loading }, T)
    expect(h.closest('section')).toHaveAttribute('aria-busy', 'true')
    expect(await screen.findByRole('heading', { level: 2, name: /^Saving/ }, T)).toBeInTheDocument()
    expect(h.isConnected).toBe(false)
  })
})

describe('first visit (R2E)', () => {
  it('requests ran but none reported: the lead says so and offers a chat; the blocks say there is nothing yet', async () => {
    configureMocks({ variant: 'optimization-no-reports' })
    renderApp('/optimization')
    expect(
      await screen.findByRole('heading', { level: 2, name: copy.lead.heading.noReports }, T),
    ).toBeInTheDocument()
    expect(screen.getByText(copy.lead.noReportsLine)).toBeInTheDocument()
    expect(
      await within(await block(copy.byAgent.title)).findByText(copy.byAgent.empty, {}, T),
    ).toBeInTheDocument()
    expect(
      await within(await block(copy.senders.title)).findByText(copy.senders.empty, {}, T),
    ).toBeInTheDocument()
    // Needs attention works as usual (it reads the agent lists, not savings).
    expect(await screen.findByRole('region', { name: copy.attention.title }, T)).toBeInTheDocument()
  })

  it('no accessible agents: the lead says so and By agent and Biggest senders show their outlines', async () => {
    server.use(http.get('/api/agents', () => HttpResponse.json([])))
    renderApp('/optimization')
    expect(
      await screen.findByRole('heading', { level: 2, name: copy.lead.heading.noAgents }, T),
    ).toBeInTheDocument()
    expect(
      await within(await block(copy.byAgent.title)).findByText(copy.byAgent.outline, {}, T),
    ).toBeInTheDocument()
    expect(
      await within(await block(copy.senders.title)).findByText(copy.senders.outline, {}, T),
    ).toBeInTheDocument()
    // Nothing to fix, so no strip; the lead says why compression is off.
    expect(await screen.findByText(copy.lead.compression['no-agents'], {}, T)).toBeInTheDocument()
    expect(screen.queryByRole('region', { name: copy.attention.title })).toBeNull()
  })
})

describe('errors (R2C)', () => {
  it('every savings read failing fails each block alone; Needs attention and your settings still work', async () => {
    configureMocks({ variant: 'optimization-down' })
    renderApp('/optimization')
    expect(
      await screen.findByRole('heading', { level: 2, name: copy.lead.heading.error }, T),
    ).toBeInTheDocument()
    for (const title of [copy.byAgent.title, copy.senders.title]) {
      const s = await block(title)
      expect(await within(s).findByRole('button', { name: /retry/i }, T)).toBeInTheDocument()
    }
    expect(await screen.findByRole('region', { name: copy.attention.title }, T)).toBeInTheDocument()
    expect(await screen.findByRole('button', { name: copy.save }, T)).toBeInTheDocument()
  })

  it('a failed Biggest senders read keeps the rest of the page', async () => {
    server.use(
      http.get(
        `${SAVINGS}/top-requests`,
        () => new HttpResponse('internal error', { status: 500 }),
      ),
    )
    renderApp('/optimization')
    const s = await block(copy.senders.title)
    expect(await within(s).findByRole('button', { name: /retry/i }, T)).toBeInTheDocument()
    const agents = await block(copy.byAgent.title)
    await waitFor(() => expect(within(agents).getAllByRole('row').length).toBeGreaterThan(1), T)
  })

  it('your account failing to load is one error with Retry, never endless skeletons', async () => {
    server.use(http.get('/api/me', () => new HttpResponse('internal error', { status: 500 })))
    renderApp('/optimization')
    expect(await screen.findByText(/your account/i, {}, T)).toBeInTheDocument()
    expect(screen.getAllByRole('button', { name: /retry/i }).length).toBeGreaterThan(0)
    expect(screen.queryByRole('heading', { level: 2, name: copy.lead.chart.loading })).toBeNull()
  })
})

describe("today's server (R2B)", () => {
  it('Needs attention and your settings work as usual', async () => {
    configureMocks({ variant: 'optimization-classic' })
    renderApp('/optimization')
    expect(await screen.findByRole('region', { name: copy.attention.title }, T)).toBeInTheDocument()
    expect(await screen.findByText(copy.subAlwaysOn, {}, T)).toBeInTheDocument()
    expect(await screen.findByRole('button', { name: copy.save }, T)).toBeInTheDocument()
  })
})
