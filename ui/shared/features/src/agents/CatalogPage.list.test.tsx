/**
 * The catalog's List view: the Grid / List switch, a row per agent with the card's facts and one link,
 * search, filters and the health reason in the table, and the choice kept in `?view=` and per viewer.
 */
import { screen, waitFor, within } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { http, HttpResponse } from 'msw'
import { describe, expect, it } from 'vitest'
import { DRAFT_PREFIX } from '@nasiko/app-core/draftKeys'
import { ADMIN_ID } from '@nasiko/mocks/seed-harness'
import { seed, setupPinnedSeed } from '@nasiko/testing/pinnedSeed'
import { renderApp } from '@nasiko/testing/renderApp'
import { server } from '@nasiko/testing/setup'
import { copy } from './copy'

setupPinnedSeed()

const live = seed.agents.filter((a) => !a.deleted)
const KEY = `${DRAFT_PREFIX}${ADMIN_ID}:meta:agentsView`
const table = () => screen.findByRole('table', { name: copy.sections.all })
/** Body rows: each holds one agent link. */
const rows = () =>
  within(screen.getByRole('table', { name: copy.sections.all }))
    .getAllByRole('row')
    .filter((r) => within(r).queryByRole('link'))
const radio = (name: string) => screen.getByRole('radio', { name })
const firstCard = () => screen.findByRole('link', { name: new RegExp(live[0]!.display_name) })

describe('catalog list view', () => {
  it('Grid is the default; List shows a row per agent with the card facts, linking to the agent', async () => {
    const { router } = renderApp('/agents')
    await firstCard()
    expect(radio(copy.viewGrid)).toHaveAttribute('aria-checked', 'true')
    expect(screen.queryByRole('table')).toBeNull()

    await userEvent.click(radio(copy.viewList))
    await table()
    expect(router.state.location.search).toMatchObject({ view: 'list' })
    expect(rows()).toHaveLength(live.length)

    // One real link per row, so open-in-new-tab works.
    const a = live[0]!
    const row = rows().find((r) => within(r).queryByText(a.display_name))!
    const link = within(row).getByRole('link', { name: a.display_name })
    expect(within(row).getAllByRole('link')).toHaveLength(1)
    expect(link).toHaveAttribute('href', `/agents/${a.id}`)
    await userEvent.click(link)
    await waitFor(() => expect(router.state.location.pathname).toBe(`/agents/${a.id}`))
  })

  it("a row shows the card's facts: name, slug, status, description, skills and owner", async () => {
    const base = {
      version: '1.0.0',
      tags: [],
      metadata: null,
      created_at: '2026-03-01T00:00:00Z',
      updated_at: '2026-03-01T00:00:00Z',
    }
    const skills = ['Triage', 'Refunds', 'Escalate', 'Summarize'].map((name, i) => ({
      id: `s${i}`,
      name,
      description: '',
    }))
    server.use(
      http.get('/api/agents', ({ request }) =>
        HttpResponse.json(
          new URL(request.url).searchParams.get('offset') === '0'
            ? [
                {
                  ...base,
                  id: '0000beef-0000-4000-8000-000000000001',
                  name: 'billing-bot',
                  display_name: 'Billing Bot',
                  description: 'Reconciles invoices.',
                  status: 'stopped',
                  owner_id: ADMIN_ID,
                  skills,
                },
                {
                  ...base,
                  id: '0000beef-0000-4000-8000-000000000002',
                  name: 'helper',
                  display_name: '',
                  description: '',
                  status: 'running',
                  owner_id: '0000beef-0000-4000-8000-0000000000ff',
                  skills: [],
                },
              ]
            : [],
        ),
      ),
    )
    renderApp('/agents?view=list')
    await table()
    const [mine, other] = rows()
    expect(within(mine!).getByRole('link', { name: 'Billing Bot' })).toBeInTheDocument()
    expect(within(mine!).getByText('billing-bot')).toBeInTheDocument()
    expect(mine!.querySelector('[data-status="stopped"]')).not.toBeNull()
    expect(within(mine!).getAllByText('Reconciles invoices.').length).toBeGreaterThan(0)
    // One chip and "+3" below 1440 px, two and "+2" from it (CSS picks; jsdom keeps both counts).
    for (const s of ['Triage', 'Refunds', '+3', '+2'])
      expect(within(mine!).getByText(s)).toBeInTheDocument()
    for (const s of ['Escalate', 'Summarize']) expect(within(mine!).queryByText(s)).toBeNull()
    expect(within(mine!).getAllByText(copy.yours).length).toBeGreaterThan(0)
    // No display name: the raw name stands in, as on the card.
    expect(within(other!).getByRole('link', { name: 'helper' })).toBeInTheDocument()
    expect(other!.querySelector('[data-status="running"]')).not.toBeNull()
    expect(within(other!).getAllByText(copy.noDescription).length).toBeGreaterThan(0)
    expect(within(other!).getAllByText(copy.availableToYou).length).toBeGreaterThan(0)
  })

  it('search, the Owner and Tag filters and the health reason apply to the rows', async () => {
    renderApp('/agents?view=list')
    await table()
    expect(rows()).toHaveLength(live.length)
    const box = screen.getByRole('searchbox', { name: 'Search agents' })
    await userEvent.type(box, live[0]!.display_name)
    await waitFor(() => expect(rows()).toHaveLength(1))
    await userEvent.clear(box)
    await waitFor(() => expect(rows()).toHaveLength(live.length))

    const pick = async (filter: string, option: string) => {
      await userEvent.click(screen.getByRole('combobox', { name: filter }))
      await userEvent.click(await screen.findByRole('option', { name: option }))
    }
    await pick(copy.ownerFilter, copy.yours)
    await waitFor(() => expect(rows().length).toBeLessThan(live.length))
    expect(rows().length).toBeGreaterThan(0)
    for (const r of rows()) expect(within(r).queryByText(copy.availableToYou)).toBeNull()
    await pick(copy.ownerFilter, copy.anyone)
    await pick(copy.tagFilter, 'ops')
    await waitFor(() => expect(rows().length).toBeLessThan(live.length))
    expect(rows().length).toBeGreaterThan(0)
  })

  it("the health filter's reason shows on every row", async () => {
    renderApp('/agents?health=action&view=list')
    await table()
    await waitFor(() => expect(rows().length).toBeGreaterThan(0))
    for (const r of rows())
      expect(within(r).getByTestId('health-reason').textContent).toMatch(/^Why: \S/)
  })

  it('?view=list opens the list without changing the remembered view; a pick is remembered', async () => {
    const first = renderApp('/agents?view=list')
    await table()
    expect(radio(copy.viewList)).toHaveAttribute('aria-checked', 'true')
    expect(localStorage.getItem(KEY)).toBeNull()
    await userEvent.click(radio(copy.viewGrid))
    await waitFor(() => expect(screen.queryByRole('table')).toBeNull())
    expect(first.router.state.location.search).not.toHaveProperty('view')
    expect(localStorage.getItem(KEY)).toBe('grid')
    await userEvent.click(radio(copy.viewList))
    await table()
    expect(localStorage.getItem(KEY)).toBe('list')
  })

  it('a bare /agents opens the remembered list; ?view=grid overrides it', async () => {
    localStorage.setItem(KEY, 'list')
    const { router } = renderApp('/agents')
    await table()
    expect(router.state.location.search).not.toHaveProperty('view')
    await router.navigate({ to: '/agents', search: { view: 'grid' } })
    await waitFor(() => expect(screen.queryByRole('table')).toBeNull())
    expect(radio(copy.viewGrid)).toHaveAttribute('aria-checked', 'true')
  })

  it('blocked storage still switches views and opens the cards', async () => {
    const fail = () => {
      throw new Error('blocked')
    }
    Object.defineProperty(globalThis, 'localStorage', {
      configurable: true,
      writable: true,
      value: { length: 0, clear: fail, getItem: fail, key: fail, removeItem: fail, setItem: fail },
    })
    renderApp('/agents')
    await firstCard()
    expect(screen.queryByRole('table')).toBeNull()
    await userEvent.click(radio(copy.viewList))
    await table()
  })

  it('the switch is a labelled radio group that arrow keys operate', async () => {
    renderApp('/agents')
    await firstCard()
    expect(screen.getByRole('radiogroup', { name: copy.viewLabel })).toBeInTheDocument()
    radio(copy.viewGrid).focus()
    await userEvent.keyboard('{ArrowRight}')
    await table()
    expect(radio(copy.viewList)).toHaveFocus()
  })
})
