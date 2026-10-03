/**
 * The agent routing sheet and the agent detail card (plans/feat-llm-router.md §4.5, §8): the choices, the request plan
 * on the wire, step-2 failure and Pin again, closing mid-plan, the ordering test, the harness row, the owner gate on
 * the agent page, superuser labels, 401 with a sheet open, and focus back to the opener.
 */
import { screen, waitFor, within } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { http } from 'msw'
import { afterEach, beforeEach, describe, expect, it } from 'vitest'
import { configureMocks, routerMockState } from '@/mocks/handlers'
import { configId } from '@/mocks/router'
import { now, seed, setupPinnedSeed } from '@/test/pinnedSeed'
import { renderApp } from '@/test/renderApp'
import { recordRequestBodies, recordRequests, server } from '@/test/setup'
import { copy } from './copy'

setupPinnedSeed()
// These tests act on agents that follow the default: show that group (it folds by default).
beforeEach(() => localStorage.setItem('openruntime.router.defaultsOpen', 'open'))
afterEach(() => configureMocks({ seed, now, loggedIn: true, superuser: null, routerVariants: [] }))

const agentNamed = (display: string) => seed.agents.find((a) => a.display_name === display)!
const ready = async () => {
  await screen.findByRole('table', { name: copy.agentsTitle })
  await waitFor(() => expect(screen.queryAllByText(copy.readingRouting)).toHaveLength(0))
}
const rowFor = (name: string) => screen.getByRole('link', { name }).closest('tr') as HTMLElement
const announcer = () => screen.getByTestId('router-announcer')
/** Picks an option from a shadcn Select (it opens a listbox; there is no native select to target). */
const choose = async (combobox: HTMLElement, option: string) => {
  await userEvent.click(combobox)
  await userEvent.click(await screen.findByRole('option', { name: option }))
}
const openSheet = async (name: string) => {
  await userEvent.click(screen.getByRole('button', { name: `${copy.changeRouting}: ${name}` }))
  return screen.findByRole('dialog', { name: copy.routingTitle(name) })
}
const routingPatches = (rec: ReturnType<typeof recordRequestBodies>) =>
  rec.requests
    .filter((r) => r.method === 'PATCH' && /\/api\/agents\/[^/]+\/llm-config$/.test(r.url.pathname))
    .map((r) => r.body as Record<string, unknown>)

/** Hold the next routing PATCH that matches until `release()`; the rest go straight to the mock. */
function holdPatch(match: (body: Record<string, unknown>) => boolean) {
  let release!: () => void
  const gate = new Promise<void>((r) => {
    release = r
  })
  let held = false
  server.use(
    http.patch('/api/agents/:id/llm-config', async ({ request }) => {
      const body = (await request.clone().json()) as Record<string, unknown>
      if (held || !match(body)) return
      held = true
      await gate
    }),
  )
  return { release, isHeld: () => held }
}

describe('choices', () => {
  it('shows the current routing, and a default switch drops an override the default can’t serve', async () => {
    const rec = recordRequestBodies()
    renderApp('/router')
    await ready()
    const sheet = await openSheet('Research Agent')
    expect(sheet).toHaveTextContent(copy.overridden('gpt-4o'))
    expect(
      within(sheet).getByRole('radio', { name: new RegExp(`^${copy.useAConfig}`) }),
    ).toBeChecked()
    await userEvent.click(within(sheet).getByRole('radio', { name: copy.useMyDefault }))
    expect(within(sheet).getByText(copy.overrideWarning)).toBeInTheDocument()
    const keep = within(sheet).getByRole('checkbox', { name: copy.keepOverride })
    expect(keep).toBeDisabled()
    expect(within(sheet).getByText(copy.keepNotOffered('gpt-4o', 'anthropic'))).toBeInTheDocument()
    await userEvent.click(within(sheet).getByRole('button', { name: copy.save }))
    expect(await within(sheet).findByText(copy.savedPlan('Research Agent'))).toBeInTheDocument()
    await rec.flush()
    expect(routingPatches(rec)).toEqual([{ llm_config_id: null }])
    // Follow-through: timing hint, the honest limit, and the sessions link.
    expect(within(sheet).getByText(copy.cantConfirm)).toBeInTheDocument()
    expect(within(sheet).getByRole('link', { name: copy.seeSessions })).toHaveAttribute(
      'href',
      `/sessions?agent=${agentNamed('Research Agent').name}`,
    )
    await waitFor(() => expect(announcer()).toHaveTextContent(copy.savedPlan('Research Agent')))
    await userEvent.click(within(sheet).getByRole('button', { name: copy.done }))
    await waitFor(() => expect(rowFor('Research Agent')).toHaveTextContent(copy.source.default))
    expect(rowFor('Research Agent')).toHaveTextContent(
      /Updated \d{1,2}:\d\d.* · applies by about \d{1,2}:\d\d/,
    )
  })

  it('switching config with Keep sends attach, then the pin: never both in one request', async () => {
    const rec = recordRequestBodies()
    renderApp('/router')
    await ready()
    const sheet = await openSheet('Research Agent')
    await choose(within(sheet).getByRole('combobox', { name: copy.chooseConfig }), 'fast-openai')
    expect(within(sheet).getByRole('checkbox', { name: copy.keepOverride })).toBeChecked()
    await userEvent.click(within(sheet).getByRole('button', { name: copy.save }))
    await within(sheet).findByText(copy.savedPlan('Research Agent'))
    await rec.flush()
    const patches = routingPatches(rec)
    expect(patches).toEqual([{ llm_config_id: configId(3) }, { pinned_model: 'gpt-4o' }])
    for (const p of patches) expect('llm_config_id' in p && 'pinned_model' in p).toBe(false)
    expect(routerMockState().routing.get(agentNamed('Research Agent').id)).toMatchObject({
      llm_config_id: configId(3),
      pinned_model: 'gpt-4o',
    })
  })

  it('setting and removing an override are single pin requests', async () => {
    const rec = recordRequestBodies()
    renderApp('/router')
    await ready()
    let sheet = await openSheet('Doc Writer')
    await userEvent.type(within(sheet).getByLabelText(copy.overrideModel), 'claude-sonnet-4')
    await userEvent.click(within(sheet).getByRole('button', { name: copy.save }))
    await within(sheet).findByText(copy.savedPlan('Doc Writer'))
    await userEvent.click(within(sheet).getByRole('button', { name: copy.done }))
    await waitFor(() =>
      expect(rowFor('Doc Writer')).toHaveTextContent(copy.overridden('claude-sonnet-4')),
    )
    sheet = await openSheet('Doc Writer')
    await userEvent.click(within(sheet).getByRole('button', { name: copy.removeOverride }))
    await userEvent.click(within(sheet).getByRole('button', { name: copy.save }))
    await within(sheet).findByText(copy.savedPlan('Doc Writer'))
    await rec.flush()
    expect(routingPatches(rec)).toEqual([
      { pinned_model: 'claude-sonnet-4' },
      { pinned_model: null },
    ])
  })

  it('a harness row attaches a config like any agent', async () => {
    const rec = recordRequestBodies()
    renderApp('/router')
    await ready()
    const sheet = await openSheet('Claude Code (admin@example.com)')
    await choose(within(sheet).getByRole('combobox', { name: copy.chooseConfig }), 'research-tiers')
    await userEvent.click(within(sheet).getByRole('button', { name: copy.save }))
    await within(sheet).findByText(copy.savedPlan('Claude Code (admin@example.com)'))
    await rec.flush()
    expect(routingPatches(rec)).toEqual([{ llm_config_id: configId(2) }])
  })

  it('closing the sheet returns focus to its opener', async () => {
    renderApp('/router')
    await ready()
    const opener = screen.getByRole('button', { name: `${copy.changeRouting}: Doc Writer` })
    const sheet = await openSheet('Doc Writer')
    await userEvent.click(within(sheet).getByRole('button', { name: copy.cancel }))
    await waitFor(() => expect(opener).toHaveFocus())
  })
})

describe('partial and interrupted plans', () => {
  it('a failed re-pin keeps the attach, says the override is gone and offers Pin again', async () => {
    configureMocks({ routerVariants: ['router-repin-fail'] })
    const rec = recordRequestBodies()
    renderApp('/router')
    await ready()
    const sheet = await openSheet('Research Agent')
    await choose(within(sheet).getByRole('combobox', { name: copy.chooseConfig }), 'fast-openai')
    await userEvent.click(within(sheet).getByRole('button', { name: copy.save }))
    expect(await within(sheet).findByText(copy.partialOverride('gpt-4o'))).toBeInTheDocument()
    expect(routerMockState().routing.get(agentNamed('Research Agent').id)).toMatchObject({
      llm_config_id: configId(3),
      pinned_model: null,
    })
    // The server recovers; Pin again plans from the saved state, so it sends the pin only.
    configureMocks({ routerVariants: [] })
    routerMockState().routing.set(agentNamed('Research Agent').id, {
      llm_config_id: configId(3),
      pinned_model: null,
      inbound_format: 'openai',
    })
    await userEvent.click(within(sheet).getByRole('button', { name: copy.pinAgain }))
    expect(await within(sheet).findByText(copy.savedPlan('Research Agent'))).toBeInTheDocument()
    await rec.flush()
    expect(routingPatches(rec).at(-1)).toEqual({ pinned_model: 'gpt-4o' })
  })

  it('closing the sheet mid-plan still runs step 2 and announces the outcome', async () => {
    const rec = recordRequestBodies()
    const hold = holdPatch((b) => 'pinned_model' in b)
    renderApp('/router')
    await ready()
    const sheet = await openSheet('Research Agent')
    await choose(within(sheet).getByRole('combobox', { name: copy.chooseConfig }), 'fast-openai')
    await userEvent.click(within(sheet).getByRole('button', { name: copy.save }))
    await waitFor(() => expect(hold.isHeld()).toBe(true))
    await userEvent.keyboard('{Escape}')
    await waitFor(() => expect(screen.queryByRole('dialog')).toBeNull())
    hold.release()
    await waitFor(() => expect(announcer()).toHaveTextContent(copy.savedPlan('Research Agent')))
    await rec.flush()
    expect(routingPatches(rec)).toEqual([
      { llm_config_id: configId(3) },
      { pinned_model: 'gpt-4o' },
    ])
    await waitFor(() =>
      expect(rowFor('Research Agent')).toHaveTextContent(copy.overridden('gpt-4o')),
    )
  })

  it('ordering: a change made on the server during the plan is what the row shows after it settles', async () => {
    const hold = holdPatch((b) => 'pinned_model' in b)
    renderApp('/router')
    await ready()
    const sheet = await openSheet('Research Agent')
    await choose(within(sheet).getByRole('combobox', { name: copy.chooseConfig }), 'fast-openai')
    await userEvent.click(within(sheet).getByRole('button', { name: copy.save }))
    await waitFor(() => expect(hold.isHeld()).toBe(true))
    // Step 1 attached fast-openai (platform key); someone re-attaches research-tiers (OPENAI_API_KEY) meanwhile.
    const id = agentNamed('Research Agent').id
    routerMockState().routing.set(id, {
      llm_config_id: configId(2),
      pinned_model: null,
      inbound_format: 'openai',
    })
    hold.release()
    await within(sheet).findByText(copy.savedPlan('Research Agent'))
    const final = routerMockState().routing.get(id)!
    expect(final).toMatchObject({ llm_config_id: configId(2), pinned_model: 'gpt-4o' })
    await userEvent.click(within(sheet).getByRole('button', { name: copy.done }))
    await waitFor(() =>
      expect(rowFor('Research Agent')).toHaveTextContent(copy.yourKey('OPENAI_API_KEY')),
    )
    expect(rowFor('Research Agent')).toHaveTextContent(copy.overridden('gpt-4o'))
  })
})

describe('agent detail card', () => {
  it('the owner sees the card and can change routing from it', async () => {
    renderApp(`/agents/${agentNamed('Research Agent').id}`)
    const card = (await screen.findByRole('heading', { name: copy.cardTitle })).closest('section')!
    await waitFor(() => expect(card).toHaveTextContent(copy.overridden('gpt-4o')))
    await userEvent.click(within(card).getByRole('button', { name: copy.changeRouting }))
    expect(
      await screen.findByRole('dialog', { name: copy.routingTitle('Research Agent') }),
    ).toBeInTheDocument()
  })

  it('a non-owner who is not a superuser never reads the routing', async () => {
    configureMocks({ superuser: false })
    const other = seed.agents[13]!
    const calls = recordRequests()
    renderApp(`/agents/${other.id}`)
    await screen.findByRole('heading', { level: 1 })
    await screen.findByRole('tab', { name: 'Overview' })
    expect(screen.queryByRole('heading', { name: copy.cardTitle })).toBeNull()
    expect(calls.urls.filter((u) => u.pathname.endsWith('/llm-config'))).toEqual([])
    calls.stop()
  })

  it('a superuser on another owner’s agent sees owner labels and can’t pick a config', async () => {
    const other = seed.agents[13]!
    renderApp(`/agents/${other.id}`)
    const card = (await screen.findByRole('heading', { name: copy.cardTitle })).closest('section')!
    await waitFor(() => expect(card).toHaveTextContent(copy.source.ownerDefault))
    await userEvent.click(within(card).getByRole('button', { name: copy.changeRouting }))
    const sheet = await screen.findByRole('dialog', { name: copy.routingTitle(other.display_name) })
    expect(within(sheet).getByRole('radio', { name: copy.useOwnersDefault })).toBeChecked()
    expect(
      within(sheet).getByRole('radio', { name: new RegExp(`^${copy.useAConfig}`) }),
    ).toBeDisabled()
    expect(within(sheet).getByText(copy.cantSeeOwnerConfigs)).toBeInTheDocument()
  })

  it('a 403 hides the card and keeps the session', async () => {
    server.use(
      http.get(
        '/api/agents/:id/llm-config',
        () => new Response('not the agent owner', { status: 403 }),
      ),
    )
    const { router } = renderApp(`/agents/${agentNamed('Research Agent').id}`)
    await screen.findByRole('tab', { name: 'Overview' })
    await waitFor(() => expect(screen.queryByRole('heading', { name: copy.cardTitle })).toBeNull())
    expect(router.state.location.pathname).toBe(`/agents/${agentNamed('Research Agent').id}`)
  })
})

describe('session', () => {
  it('a 401 while a sheet is open goes to /login?expired=true', async () => {
    const { router } = renderApp('/router')
    await ready()
    const sheet = await openSheet('Doc Writer')
    server.use(
      http.patch(
        '/api/agents/:id/llm-config',
        () => new Response('session expired', { status: 401 }),
      ),
    )
    await userEvent.type(within(sheet).getByLabelText(copy.overrideModel), 'gpt-4o')
    await userEvent.click(within(sheet).getByRole('button', { name: copy.save }))
    await waitFor(() => expect(router.state.location.pathname).toBe('/login'))
    expect(router.state.location.search).toMatchObject({ expired: true })
  })
})

// Regression: ISSUE-003 (/qa 2026-09-28, .gstack/qa-reports/qa-report-localhost-2026-09-28.md): the "Use a config"
// radio's name included the chosen config.
describe('radio names', () => {
  it('the config radio is named by its label only', async () => {
    renderApp('/router')
    await ready()
    await userEvent.click(
      screen.getByRole('button', { name: `${copy.changeRouting}: Research Agent` }),
    )
    const sheet = await screen.findByRole('dialog', { name: copy.routingTitle('Research Agent') })
    expect(await within(sheet).findByRole('radio', { name: copy.useAConfig })).toBeChecked()
  })
})
