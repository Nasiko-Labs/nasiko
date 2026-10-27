/**
 * The Flows list (plans/feat-flows.md §3, §2a; T5) through the real router and the flows mock: the window, Kind and
 * Status filters, search, the sample-labelled charts and Duration table, the empty, filtered, older-server and error
 * states, the sidebar item and "← Flows" back to the list as it was left (F20).
 */
import { screen, waitFor, within } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { http, HttpResponse } from 'msw'
import { afterEach, describe, expect, it } from 'vitest'
import { configureMocks } from '@nasiko/mocks/handlers'
import { now, seed, setupPinnedSeed } from '@nasiko/testing/pinnedSeed'
import { renderApp } from '@nasiko/testing/renderApp'
import { recordRequests, server } from '@nasiko/testing/setup'
import { copy } from './copy'
import { SEARCH_DEBOUNCE_MS } from './tuning'

setupPinnedSeed()
afterEach(() => configureMocks({ seed, now, loggedIn: true, variant: null }))

const table = async () =>
  screen.findByRole('table', { name: copy.list.tableLabel }, { timeout: 8000 })
const titles = async () =>
  within(await table())
    .getAllByRole('link')
    .map((a) => a.textContent)

describe('Flows list', () => {
  it('lists the last 7 days, newest first, with the sample-labelled charts', async () => {
    renderApp('/flows')
    const t = await titles()
    expect(t).toContain('Compare 2026 pricing for three vendors and draft a summary')
    expect(t).not.toContain('Someone else’s question')
    const requests = (await screen.findByRole('heading', { name: copy.list.requests })).closest(
      'section',
    )!
    expect(within(requests).getByText(/^From your latest \d+ flows$/)).toBeInTheDocument()
    // Under 100 finished flows: no p99, and the panel says when it will show.
    const duration = screen.getByRole('heading', { name: copy.list.duration }).closest('section')!
    expect(within(duration).getByText(copy.list.p99Hidden(100))).toBeInTheDocument()
    await userEvent.click(within(duration).getByRole('radio', { name: copy.list.table }))
    expect(within(duration).getByRole('cell', { name: 'p50' })).toBeInTheDocument()
    expect(within(duration).queryByRole('cell', { name: 'p99' })).toBeNull()
  })

  it('filters Kind on the page and Status on the server', async () => {
    const rec = recordRequests()
    renderApp('/flows')
    await table()
    const pick = async (filter: string, option: string) => {
      await userEvent.click(screen.getByRole('combobox', { name: filter }))
      await userEvent.click(await screen.findByRole('option', { name: option }))
    }
    await pick(copy.list.kind, copy.list.kinds.workflow)
    await waitFor(async () =>
      expect(await titles()).toEqual(
        expect.arrayContaining(['Quarterly report: draft the revenue section']),
      ),
    )
    expect(await titles()).not.toContain(
      'Compare 2026 pricing for three vendors and draft a summary',
    )
    await pick(copy.list.statusLabel, copy.list.statuses.failed)
    await waitFor(() =>
      expect(
        rec.urls.some(
          (u) => u.pathname === '/api/flows' && u.searchParams.get('status') === 'failed',
        ),
      ).toBe(true),
    )
    rec.stop()
  })

  it('searches after a pause, and Clear filters brings everything back', async () => {
    renderApp('/flows')
    await table()
    await userEvent.type(screen.getByRole('searchbox', { name: copy.list.search }), 'pricing')
    await waitFor(async () =>
      expect(await titles()).toEqual([
        'Compare 2026 pricing for three vendors and draft a summary',
      ]),
    )
    await userEvent.clear(screen.getByRole('searchbox', { name: copy.list.search }))
    await userEvent.type(screen.getByRole('searchbox', { name: copy.list.search }), 'zzz-nothing')
    expect(
      await screen.findByText(copy.list.filteredEmpty(null, 'zzz-nothing'), {}, { timeout: 8000 }),
    ).toBeInTheDocument()
    await userEvent.click(screen.getByRole('button', { name: copy.list.clear }))
    await waitFor(async () => expect((await titles()).length).toBeGreaterThan(5))
    expect(screen.getByRole('searchbox', { name: copy.list.search })).toHaveValue('')
  })

  it('keys typed while its own commit reaches the URL stay in the box (review fix)', async () => {
    renderApp('/flows')
    await table()
    const box = screen.getByRole('searchbox', { name: copy.list.search })
    await userEvent.type(box, 'pri')
    // Keep typing around the moment the first commit lands in the URL.
    await new Promise((r) => setTimeout(r, SEARCH_DEBOUNCE_MS))
    await userEvent.type(box, 'cing')
    expect(box).toHaveValue('pricing')
    await waitFor(async () =>
      expect(await titles()).toEqual([
        'Compare 2026 pricing for three vendors and draft a summary',
      ]),
    )
    expect(box).toHaveValue('pricing')
  })

  it('first run: no flows yet, with Open Chat', async () => {
    configureMocks({ variant: 'flows-empty' })
    renderApp('/flows')
    expect(await screen.findByText(copy.list.firstRun, {}, { timeout: 8000 })).toBeInTheDocument()
    expect(screen.getByRole('link', { name: copy.list.openChat })).toHaveAttribute('href', '/chat')
  })

  it('a server without /api/flows says it needs a newer one', async () => {
    configureMocks({ variant: 'flows-absent' })
    renderApp('/flows')
    expect(await screen.findByText(copy.list.absent, {}, { timeout: 8000 })).toBeInTheDocument()
  })

  it('a failing read shows the error with Retry', async () => {
    server.use(http.get('/api/flows', () => new HttpResponse('internal error', { status: 500 })))
    renderApp('/flows')
    expect(
      await screen.findByRole('button', { name: /retry/i }, { timeout: 8000 }),
    ).toBeInTheDocument()
  })

  it('is in the sidebar, and "← Flows" returns to the list as it was left (F20)', async () => {
    renderApp('/flows?kind=orchestrated')
    await table()
    expect(screen.getAllByRole('link', { name: 'Flows' }).length).toBeGreaterThan(0)
    await userEvent.click(
      within(await table()).getByRole('link', {
        name: 'Compare 2026 pricing for three vendors and draft a summary',
      }),
    )
    const back = await screen.findByRole('link', { name: copy.backLabel }, { timeout: 8000 })
    expect(back.getAttribute('href')).toContain('kind=orchestrated')
  })
})
