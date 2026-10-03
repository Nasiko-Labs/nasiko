/**
 * TokenOps filters: the provider, model and agent chips (Radix Select), the KPI "More metrics" toggle, and the
 * breadcrumb and chips that clear a drill-down. (jsdom's missing pointer-capture and scrolling APIs, which Radix
 * Select calls, are stubbed in src/test/setup.ts.)
 */
import { screen, waitFor, within } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { describe, expect, it } from 'vitest'
import { SEED_MODELS } from '@/mocks/seed'
import { seed, section, setupPinnedSeed } from '@/test/pinnedSeed'
import { renderApp } from '@/test/renderApp'

const liveAgent = seed.agents.find((a) => !a.deleted)!
const deletedAgent = seed.agents.find((a) => a.deleted)!

setupPinnedSeed()

const optionNames = () => screen.getAllByRole('option').map((o) => o.textContent)

async function choose(
  user: ReturnType<typeof userEvent.setup>,
  combobox: HTMLElement,
  option: string,
) {
  await user.click(combobox)
  await user.click(await screen.findByRole('option', { name: option }))
}

describe('filter chips (Radix Select)', () => {
  it('provider change clears the model; the model list is narrowed to that provider', async () => {
    const user = userEvent.setup()
    const { router } = renderApp('/tokenops?open=all&model=gpt-4o')
    await screen.findByLabelText(/^Key metrics/)
    await choose(user, screen.getByRole('combobox', { name: 'Filter by provider' }), 'anthropic')
    await waitFor(() =>
      expect(router.state.location.search).toMatchObject({ provider: 'anthropic' }),
    )
    expect(router.state.location.search).not.toHaveProperty('model')

    await user.click(screen.getByRole('combobox', { name: 'Filter by model' }))
    const anthropic = SEED_MODELS.filter((m) => m.provider === 'anthropic').map((m) => m.model)
    expect(optionNames()).toEqual(['All', ...anthropic])
    await user.click(screen.getByRole('option', { name: anthropic[0] }))
    await waitFor(() =>
      expect(router.state.location.search).toMatchObject({
        provider: 'anthropic',
        model: anthropic[0],
      }),
    )

    // "All" clears the filter.
    await choose(user, screen.getByRole('combobox', { name: 'Filter by provider' }), 'All')
    await waitFor(() => expect(router.state.location.search).not.toHaveProperty('provider'))
  })

  it('a URL value not in the list still appears as an option', async () => {
    const user = userEvent.setup()
    renderApp('/tokenops?open=all&model=not-a-model')
    await screen.findByLabelText(/^Key metrics/)
    await user.click(screen.getByRole('combobox', { name: 'Filter by model' }))
    expect(optionNames()).toEqual([
      'All',
      'not-a-model',
      ...[...new Set(SEED_MODELS.map((m) => m.model))].sort(),
    ])
  })

  it('agent filter lists display names (no deleted agent) and sets ?agent=<uuid>, closing traces', async () => {
    const user = userEvent.setup()
    const { router } = renderApp('/tokenops?open=all')
    await section('Who is driving cost')
    await user.click(screen.getByRole('combobox', { name: 'Filter by agent' }))
    const names = optionNames()
    expect(names).toContain(liveAgent.display_name)
    expect(names).not.toContain(deletedAgent.display_name)
    expect(names).not.toContain(liveAgent.name)
    await user.click(screen.getByRole('option', { name: liveAgent.display_name }))
    await waitFor(() => expect(router.state.location.search).toMatchObject({ agent: liveAgent.id }))
    expect(screen.getByRole('combobox', { name: 'Filter by agent' })).toHaveTextContent(
      liveAgent.display_name,
    )
  })
})

describe('drill-down chips and metrics', () => {
  it('KPI "More metrics" toggles (replace), breadcrumb and filter chips clear the drill-down', async () => {
    const user = userEvent.setup()
    const { router } = renderApp(`/tokenops?open=all&day=${seed.spikeDate}&agent=${liveAgent.id}`)
    const kpis = await screen.findByLabelText(/^Key metrics/)
    const depth = router.history.length
    await user.click(within(kpis).getByRole('button', { name: /More metrics/ }))
    expect(within(kpis).getByText('Active agents')).toBeInTheDocument()
    await waitFor(() => expect(router.state.location.search).toMatchObject({ more: true }))
    expect(router.history.length).toBe(depth)
    await user.click(within(kpis).getByRole('button', { name: /Fewer metrics/ }))
    expect(within(kpis).queryByText('Active agents')).toBeNull()

    const crumbs = screen.getByRole('navigation', { name: 'Drill-down' })
    expect(within(crumbs).getByText(liveAgent.display_name)).toHaveAttribute(
      'aria-current',
      'location',
    )
    await user.click(screen.getByRole('button', { name: 'Remove agent filter' }))
    await waitFor(() => expect(router.state.location.search).not.toHaveProperty('agent'))
    await user.click(
      within(screen.getByRole('navigation', { name: 'Drill-down' })).getByRole('button', {
        name: 'Last 30 days',
      }),
    )
    await waitFor(() => expect(router.state.location.search).not.toHaveProperty('day'))
    expect(screen.queryByRole('navigation', { name: 'Drill-down' })).toBeNull()
  })
})
