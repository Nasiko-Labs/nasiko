/**
 * TokenOps attribution (F3) and cost vs performance (F5): search, CSV export, sort and the Workflows toggle, row and
 * card "View traces", the access-scope notice, the narrow-screen panel slot, and the chart/table toggles.
 */
import { screen, waitFor, within } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { http, HttpResponse } from 'msw'
import { describe, expect, it, onTestFinished, vi } from 'vitest'
import { withHarnessTurns } from '@nasiko/mocks/aggregate'
import { generateHarnessSeed } from '@nasiko/mocks/seed-harness'
import { FIXED, seed, section, setupPinnedSeed } from '@nasiko/testing/pinnedSeed'
import { renderApp } from '@nasiko/testing/renderApp'
import { server } from '@nasiko/testing/setup'

const liveAgent = seed.agents.find((a) => !a.deleted)!
// Base UI's Select shows its options a tick after the click.
const optionNames = async () => (await screen.findAllByRole('option')).map((o) => o.textContent)

setupPinnedSeed()

describe('attribution', () => {
  // Fixed 2026-10-22: KPIs, the agent table, the calendar and the timeline all scope to the caller's own
  // calls on agents they can access now (oss/server/src/observability, ee/server/src/finops_scope.rs) —
  // the agent view (the default) no longer needs this notice. Only the workflow table is still fleet-wide.
  it('a non-superuser sees no access-scope notice on the now-scoped agent view', async () => {
    server.use(
      http.get('/api/me', () =>
        HttpResponse.json({ sub: 'u1', username: 'viewer', is_superuser: false }),
      ),
    )
    renderApp('/tokenops?open=all')
    await screen.findAllByRole('row')
    expect(screen.queryByText(/fleet-wide/)).toBeNull()
  })

  it('a non-superuser still sees the access-scope notice on the workflow view', async () => {
    server.use(
      http.get('/api/me', () =>
        HttpResponse.json({ sub: 'u1', username: 'viewer', is_superuser: false }),
      ),
    )
    renderApp('/tokenops?open=all&view=workflow')
    expect(await screen.findByText(/workflow table is fleet-wide/)).toBeInTheDocument()
  })

  it('search with no match, CSV export of visible rows, row "View traces" scopes the drawer', async () => {
    const user = userEvent.setup()
    let blob: Blob | undefined
    const originalUrl = {
      createObjectURL: URL.createObjectURL,
      revokeObjectURL: URL.revokeObjectURL,
    }
    Object.assign(URL, {
      createObjectURL: vi.fn((b: Blob) => {
        blob = b
        return 'blob:x'
      }),
      revokeObjectURL: vi.fn(),
    })
    try {
      vi.spyOn(HTMLAnchorElement.prototype, 'click').mockImplementation(() => {})
      const { router } = renderApp('/tokenops?open=all')
      const attribution = await section('Who is driving cost')
      await within(attribution).findByRole('table')

      await user.type(within(attribution).getByRole('searchbox', { name: 'Search agents' }), 'zzz')
      expect(await within(attribution).findByText('No agents match “zzz”')).toBeInTheDocument()
      expect(within(attribution).getByRole('button', { name: /CSV/ })).toBeDisabled()
      await waitFor(() => expect(router.state.location.search).toMatchObject({ q: 'zzz' }))
      await user.clear(within(attribution).getByRole('searchbox', { name: 'Search agents' }))
      await waitFor(() => expect(router.state.location.search).not.toHaveProperty('q'))

      await user.click(within(attribution).getByRole('button', { name: /CSV/ }))
      const lines = (await blob!.text()).split('\r\n')
      expect(lines[0]).toMatch(/^"Agent","Spend \(USD\)","Share \(%\)",.*"Container hours"$/)
      // The table shows the top 10 until Show all; the CSV always has every visible row.
      const capped = within(within(attribution).getByRole('table')).getAllByRole('row')
      expect(capped).toHaveLength(11)
      await user.click(within(attribution).getByRole('button', { name: /^Show all \d+$/ }))
      const rows = within(within(attribution).getByRole('table')).getAllByRole('row')
      expect(lines).toHaveLength(rows.length)

      await user.click(
        within(attribution).getByRole('button', {
          name: `View traces for ${liveAgent.display_name}`,
        }),
      )
      await waitFor(() =>
        expect(router.state.location.search).toMatchObject({ agent: liveAgent.id, traces: true }),
      )
      const drawer = await screen.findByRole('dialog')
      expect(
        within(drawer).getByText(new RegExp(`^${liveAgent.display_name} · `)),
      ).toBeInTheDocument()
    } finally {
      Object.assign(URL, originalUrl)
    }
  })

  it('the sort dropdown sets ?sort (replace) and the Workflows toggle switches view and resets sort', async () => {
    const user = userEvent.setup()
    const { router } = renderApp('/tokenops?open=all&sort=name')
    const attribution = await section('Who is driving cost')
    const sort = within(attribution).getByRole('combobox', { name: 'Sort by' })
    await user.click(sort)
    expect(await optionNames()).toContain('Most container hours')
    await user.click(screen.getByRole('option', { name: 'Most tokens' }))
    await waitFor(() => expect(router.state.location.search).toMatchObject({ sort: 'tokens' }))
    expect(sort).toHaveTextContent('Most tokens')

    await user.click(within(attribution).getByRole('radio', { name: 'Workflows' }))
    await waitFor(() =>
      expect(router.state.location.search).toMatchObject({ view: 'workflow', sort: 'cost' }),
    )
    const wf = await section('Who is driving cost')
    await user.click(within(wf).getByRole('combobox', { name: 'Sort by' }))
    expect(await optionNames()).not.toContain('Most container hours')
  })

  it('workflow view coerces sort=hours to Highest spend and hides the hours option', async () => {
    renderApp('/tokenops?open=all&view=workflow&sort=hours')
    const attribution = await section('Who is driving cost')
    expect(within(attribution).getByRole('combobox', { name: 'Sort by' })).toHaveTextContent(
      'Highest spend',
    )
    expect(within(attribution).queryByRole('button', { name: /View traces for/ })).toBeNull()
  })

  it('mobile card "View traces" opens the drawer for that card\'s agent', async () => {
    const user = userEvent.setup()
    const { router } = renderApp('/tokenops?open=all')
    const attribution = await section('Who is driving cost')
    const cardButtons = await within(attribution).findAllByRole('button', { name: 'View traces' })
    const card = cardButtons[1].closest('li')!
    const name = card.querySelector('span')!.textContent
    // The attribution rows include the admin's harness agents (aggregate.ts withHarnessTurns).
    const agent = withHarnessTurns(seed, generateHarnessSeed({ anchor: FIXED })).agents.find(
      (a) => a.display_name === name,
    )!
    await user.click(cardButtons[1])
    await waitFor(() =>
      expect(router.state.location.search).toMatchObject({
        agent: agent.id,
        traces: true,
        view: 'agent',
      }),
    )
    expect(await screen.findByRole('dialog')).toBeInTheDocument()
  })
})

describe('narrow screens', () => {
  it('one panel slot; Cost vs performance row picks highlight the attribution row', async () => {
    vi.stubGlobal('matchMedia', (query: string) => ({
      matches: false,
      media: query,
      addEventListener: () => {},
      removeEventListener: () => {},
    }))
    // jsdom has no scrollIntoView: add it for this test only, so other tests still catch
    // a missing optional call.
    const proto = Element.prototype as { scrollIntoView?: () => void }
    const added = !proto.scrollIntoView
    if (added) proto.scrollIntoView = () => {}
    onTestFinished(() => {
      if (added) delete proto.scrollIntoView
    })
    const scroll = vi.spyOn(Element.prototype, 'scrollIntoView')
    const user = userEvent.setup()
    renderApp('/tokenops?open=all')
    await section('Who is driving cost')
    expect(screen.queryByRole('heading', { name: 'Cost vs performance' })).toBeNull()
    await user.click(screen.getByRole('radio', { name: 'Cost vs performance' }))
    const perf = await section('Cost vs performance')
    expect(screen.queryByRole('heading', { name: 'Who is driving cost' })).toBeNull()
    await user.click(within(perf).getByRole('button', { name: 'Table' }))
    const pick = within(within(perf).getByRole('table')).getAllByRole('button')[0]
    const name = pick.textContent!
    await user.click(pick)
    const attribution = await section('Who is driving cost')
    const row = within(within(attribution).getByRole('table')).getByText(name).closest('tr')!
    expect(row.className).toMatch(/bg-primary\/10/)
    expect(scroll).toHaveBeenCalled()
  })
})

// Regression: ISSUE-003 (/qa 2026-09-26, .gstack/qa-reports/qa-report-localhost-3000-2026-09-26.md): chart/table
// toggles read "Chart, pressed" while the table was showing.
describe('chart/table view toggles', () => {
  it.each([
    ['Spend over time', /Spend over time/],
    ['day panel', /hour by hour/],
    ['Cost vs performance', /Cost vs performance/],
  ])('%s: the button names the action and carries no pressed state', async (_, heading) => {
    const user = userEvent.setup()
    renderApp(`/tokenops?open=all&day=${seed.spikeDate}`)
    const panel = await section(heading)
    const toTable = await within(panel).findByRole('button', { name: 'Table' })
    expect(toTable).not.toHaveAttribute('aria-pressed')
    await user.click(toTable)
    const toChart = within(panel).getByRole('button', { name: 'Chart' })
    expect(toChart).not.toHaveAttribute('aria-pressed')
    expect(within(panel).getByRole('table')).toBeInTheDocument()
  })
})
