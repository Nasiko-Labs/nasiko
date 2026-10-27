/**
 * TokenOps filters: the provider, model and agent chips (Select), the always-visible metrics, and the
 * breadcrumb and chips that clear a drill-down. (jsdom's missing pointer-capture and scrolling APIs, which a
 * Select may call, are stubbed in shared/testing/src/setup.ts.)
 */
import { screen, waitFor, within } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { describe, expect, it } from 'vitest'
import { SEED_MODELS } from '@nasiko/mocks/seed'
import { seed, section, setupPinnedSeed } from '@nasiko/testing/pinnedSeed'
import { renderApp } from '@nasiko/testing/renderApp'

const liveAgent = seed.agents.find((a) => !a.deleted)!
const deletedAgent = seed.agents.find((a) => a.deleted)!

setupPinnedSeed()

// Base UI's Select shows its options a tick after the click.
const optionNames = async () => (await screen.findAllByRole('option')).map((o) => o.textContent)

async function choose(
  user: ReturnType<typeof userEvent.setup>,
  combobox: HTMLElement,
  option: string,
) {
  await user.click(combobox)
  await user.click(await screen.findByRole('option', { name: option }))
}

describe('filter chips (Select)', () => {
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
    expect(await optionNames()).toEqual(['All', ...anthropic])
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
    expect(await optionNames()).toEqual([
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
    const names = await optionNames()
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
  it('every metric is on the page at once; breadcrumb and filter chips clear the drill-down', async () => {
    const user = userEvent.setup()
    const { router } = renderApp(`/tokenops?open=all&day=${seed.spikeDate}&agent=${liveAgent.id}`)
    const kpis = await screen.findByLabelText(/^Key metrics/)
    // No "More metrics" expander any more: the six tiles and the four secondary figures always show.
    for (const label of ['Total spend', 'Operations', 'p95 latency', 'Active agents'])
      expect(within(kpis).getByText(label)).toBeInTheDocument()
    const more = screen.getByLabelText(/^More metrics/)
    for (const label of ['Tokens', 'Tool calls', 'p50 latency', 'p99 latency'])
      expect(within(more).getByText(label)).toBeInTheDocument()

    // The drill path is the top bar's breadcrumb: TokenOps › day › agent.
    const crumbs = () => screen.getByRole('navigation', { name: 'Breadcrumb' })
    expect(within(crumbs()).getByText(liveAgent.display_name)).toHaveAttribute(
      'aria-current',
      'page',
    )
    await user.click(screen.getByRole('button', { name: 'Remove agent filter' }))
    await waitFor(() => expect(router.state.location.search).not.toHaveProperty('agent'))
    await user.click(within(crumbs()).getByRole('button', { name: 'TokenOps' }))
    await waitFor(() => expect(router.state.location.search).not.toHaveProperty('day'))
    // Undrilled, the shell's breadcrumb is back: TokenOps is the current page, not a button.
    expect(within(crumbs()).queryByRole('button', { name: 'TokenOps' })).toBeNull()
    expect(router.state.location.search).toMatchObject({ preset: '30d' })
  })
})
