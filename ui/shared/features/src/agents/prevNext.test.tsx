/** Previous/Next on the agent page (docs/superpowers/plans/2026-10-06-prev-next.md). */
import { screen, waitFor, within } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { http, HttpResponse } from 'msw'
import { afterEach, describe, expect, it } from 'vitest'
import { buildAgentsState } from '@nasiko/mocks/agents'
import { configureMocks } from '@nasiko/mocks/handlers'
import { generateHarnessSeed } from '@nasiko/mocks/seed-harness'
import { FIXED, seed, setupPinnedSeed } from '@nasiko/testing/pinnedSeed'
import { renderApp } from '@nasiko/testing/renderApp'
import { server } from '@nasiko/testing/setup'

setupPinnedSeed()
afterEach(() => configureMocks({ persona: null }))

const live = seed.agents.filter((a) => !a.deleted)
const cards = () =>
  screen
    .getAllByRole('link')
    .filter((l) => /^\/agents\/[0-9a-f-]{36}$/.test(l.getAttribute('href') ?? ''))
const position = (n: number, of: number) =>
  screen.getByRole('group', { name: `agent ${n} of ${of}` })

describe('agent Previous/Next', () => {
  it('steps through the catalog in its order with the buttons and J/K, replacing history', async () => {
    const { router } = renderApp('/agents')
    await screen.findByRole('link', { name: new RegExp(live[0]!.display_name) })
    const hrefs = cards().map((l) => l.getAttribute('href'))
    await userEvent.click(cards()[0]!)
    const next = await screen.findByRole('button', { name: 'Next agent' })
    expect(position(1, hrefs.length)).toBeInTheDocument()
    expect(screen.getByRole('button', { name: 'Previous agent' })).toHaveAttribute(
      'aria-disabled',
      'true',
    )
    await userEvent.click(next)
    await waitFor(() => expect(router.state.location.pathname).toBe(hrefs[1]))
    await waitFor(() => expect(position(2, hrefs.length)).toBeInTheDocument())
    // The new page's arrow has focus again after the remount.
    await waitFor(() => expect(screen.getByRole('button', { name: 'Next agent' })).toHaveFocus())
    await userEvent.keyboard('k')
    await waitFor(() => expect(router.state.location.pathname).toBe(hrefs[0]))
    await userEvent.keyboard('j')
    await waitFor(() => expect(router.state.location.pathname).toBe(hrefs[1]))
    // Replaced, not pushed: Back is the catalog.
    router.history.back()
    await waitFor(() => expect(router.state.location.pathname).toBe('/agents'))
  })

  it('keeps the list across a tab switch', async () => {
    renderApp('/agents')
    await screen.findByRole('link', { name: new RegExp(live[0]!.display_name) })
    const total = cards().length
    await userEvent.click(cards()[0]!)
    await screen.findByRole('button', { name: 'Next agent' })
    await userEvent.click(screen.getByRole('tab', { name: 'Activity' }))
    await waitFor(() =>
      expect(screen.getByRole('tab', { name: 'Activity' })).toHaveAttribute(
        'aria-selected',
        'true',
      ),
    )
    expect(screen.getByRole('button', { name: 'Next agent' })).toBeInTheDocument()
    expect(position(1, total)).toBeInTheDocument()
  })

  it('walks the list as filtered when the card was clicked', async () => {
    renderApp('/agents')
    await screen.findByRole('link', { name: new RegExp(live[0]!.display_name) })
    await userEvent.type(
      screen.getByRole('searchbox', { name: 'Search agents' }),
      live[1]!.display_name,
    )
    await waitFor(() => expect(cards()).toHaveLength(1))
    await userEvent.click(cards()[0]!)
    expect(await screen.findByRole('group', { name: 'agent 1 of 1' })).toBeInTheDocument()
    expect(screen.getByRole('button', { name: 'Next agent' })).toHaveAttribute(
      'aria-disabled',
      'true',
    )
  })

  it('steps through Your agents too', async () => {
    const { router } = renderApp('/agents/mine')
    const table = await screen.findByRole('table')
    // One name link per row (the actions menu's "Open agent" is closed).
    const hrefs = (await within(table).findAllByRole('link')).map((l) => l.getAttribute('href'))
    await userEvent.click(within(table).getAllByRole('link')[0]!)
    await screen.findByRole('group', { name: `agent 1 of ${hrefs.length}` })
    await userEvent.click(screen.getByRole('button', { name: 'Next agent' }))
    await waitFor(() => expect(router.state.location.pathname).toBe(hrefs[1]))
  })

  it('falls back to Overview when the next agent lacks the tab, keeping the list', async () => {
    // A member manages only their own agents, so Settings is theirs alone: sign in as the owner of a seed
    // agent that a non-admin owns (the mock seeds a few, each with a different owner).
    const hs = generateHarnessSeed({ anchor: FIXED })
    const agents = buildAgentsState(seed, hs, FIXED.getTime()).agents
    const owner = hs.users.find((u) =>
      agents.some((a) => a.owner_id === u.id && !a.harness && !u.is_superuser),
    )!
    configureMocks({ persona: owner.username })
    const owned = new Set(agents.filter((a) => a.owner_id === owner.id).map((a) => a.id))
    const { router } = renderApp('/agents')
    await screen.findByRole('link', { name: new RegExp(live[0]!.display_name) })
    const ids = cards().map((l) => l.getAttribute('href')!.split('/').pop()!)
    const at = ids.findIndex((id, i) => owned.has(id) && ids[i + 1] && !owned.has(ids[i + 1]!))
    expect(at).toBeGreaterThanOrEqual(0)
    await userEvent.click(cards()[at]!)
    await userEvent.click(await screen.findByRole('tab', { name: 'Settings' }))
    await waitFor(() => expect(router.state.location.search).toEqual({ tab: 'settings' }))
    await userEvent.click(screen.getByRole('button', { name: 'Next agent' }))
    await waitFor(() => expect(router.state.location.pathname).toBe(`/agents/${ids[at + 1]}`))
    await waitFor(() => expect(router.state.location.search).toEqual({}))
    expect(screen.getByRole('tab', { name: 'Overview' })).toHaveAttribute('aria-selected', 'true')
    expect(position(at + 2, ids.length)).toBeInTheDocument()
  })

  it('asks before leaving unsaved settings, and J waits behind the prompt', async () => {
    const { router } = renderApp('/agents')
    await screen.findByRole('link', { name: new RegExp(live[0]!.display_name) })
    const first = cards()[0]!.getAttribute('href')
    await userEvent.click(cards()[0]!)
    await userEvent.click(await screen.findByRole('tab', { name: 'Settings' }))
    await userEvent.type(await screen.findByLabelText('Display name'), ' edited')
    await userEvent.click(screen.getByRole('button', { name: 'Next agent' }))
    const dialog = await screen.findByRole('alertdialog', { name: 'Leave without saving?' })
    expect(router.state.location.pathname).toBe(first)
    await userEvent.keyboard('j')
    expect(router.state.location.pathname).toBe(first)
    await userEvent.click(within(dialog).getByRole('button', { name: 'Stay' }))
    await waitFor(() => expect(screen.queryByRole('alertdialog')).toBeNull())
    expect(router.state.location.pathname).toBe(first)
  })

  it('keeps the arrows on a neighbour that fails to load', async () => {
    const { router } = renderApp('/agents')
    await screen.findByRole('link', { name: new RegExp(live[0]!.display_name) })
    const hrefs = cards().map((l) => l.getAttribute('href')!)
    server.use(
      http.get(
        `/api/agents/${hrefs[1]!.split('/').pop()}`,
        () => new HttpResponse('boom', { status: 500 }),
      ),
    )
    await userEvent.click(cards()[0]!)
    await userEvent.click(await screen.findByRole('button', { name: 'Next agent' }))
    expect(await screen.findByRole('button', { name: /Retry/ })).toBeInTheDocument()
    expect(position(2, hrefs.length)).toBeInTheDocument()
    await userEvent.click(screen.getByRole('button', { name: 'Next agent' }))
    await waitFor(() => expect(router.state.location.pathname).toBe(hrefs[2]))
  })

  it('keeps the arrows on a neighbour that is gone', async () => {
    const { router } = renderApp('/agents')
    await screen.findByRole('link', { name: new RegExp(live[0]!.display_name) })
    const hrefs = cards().map((l) => l.getAttribute('href')!)
    server.use(
      http.get(
        `/api/agents/${hrefs[1]!.split('/').pop()}`,
        () => new HttpResponse(null, { status: 404 }),
      ),
    )
    await userEvent.click(cards()[0]!)
    await userEvent.click(await screen.findByRole('button', { name: 'Next agent' }))
    await screen.findByText('Agent not found or not visible.')
    expect(position(2, hrefs.length)).toBeInTheDocument()
    await userEvent.click(screen.getByRole('button', { name: 'Next agent' }))
    await waitFor(() => expect(router.state.location.pathname).toBe(hrefs[2]))
  })

  it('shows no arrows on a deep link', async () => {
    renderApp(`/agents/${live[0]!.id}`)
    await screen.findByRole('heading', { level: 1 })
    expect(screen.queryByRole('button', { name: 'Next agent' })).toBeNull()
  })
})
