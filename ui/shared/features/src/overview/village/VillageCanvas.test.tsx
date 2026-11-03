/**
 * The village on the Overview (plans/feat-village-overview.md) against the OV-1 mock, through the real router: it is
 * the default view; a place opens the insight panel on what it stands for and puts the selection in the URL, and a
 * click on open ground clears it; the town hall (governance), token tank, piggy bank and orchestrator have their own
 * panels; the Control center sorts what needs you and Take me there selects it; Kiko walks the same items; the Map is
 * one switch away (an old List link opens it); Build lights green plots and a plot opens the page where the thing is set
 * up; Mine reads the viewer's allowance.
 */
import { fireEvent, screen, waitFor, within } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { afterEach, beforeAll, describe, expect, it } from 'vitest'
import { newChatDraftKey, readDraft } from '@nasiko/features/chat/drafts'
import { configureMocks } from '@nasiko/mocks/handlers'
import { ADMIN_ID } from '@nasiko/mocks/seed-harness'
import { now, seed, setupPinnedSeed } from '@nasiko/testing/pinnedSeed'
import { renderApp } from '@nasiko/testing/renderApp'
import { copy as overviewCopy } from '../copy'
import { boardCopy } from '../fleet/copy'
import { villageCopy as copy } from './copy'
import { PREFS_KEY } from './prefs'

setupPinnedSeed()
// The canvas is a lazy chunk: load it once up front, so the first test's render doesn't pay its transform.
beforeAll(() => import('./VillageCanvas'), 60_000)
afterEach(() => configureMocks({ seed, now, loggedIn: true, superuser: null, variant: null }))

const village = () => screen.findByTestId('village-canvas', {}, { timeout: 15_000 })
const PLATFORM = '5eed0002-0001-4000-8000-000000000004'

describe('the village', () => {
  it('draws the HUD, the districts and the cars', async () => {
    renderApp('/?view=village')
    const v = await village()
    expect(within(v).getByRole('application', { name: copy.instructions })).toBeInTheDocument()
    // No fleet health any more; Build is the Control center's call to action.
    expect(within(v).queryByRole('button', { name: /^Fleet health/ })).toBeNull()
    // The System card sits top left in both views, folded to its counts; zoom is a tab on the top edge.
    expect(within(v).getByRole('button', { name: boardCopy.system.openLabel })).toBeInTheDocument()
    expect(within(v).getByRole('button', { name: copy.hud.fit })).toBeInTheDocument()
    expect(within(v).getByRole('button', { name: copy.hud.buildLabel })).toHaveTextContent(
      copy.hud.build,
    )
    // One scale for the meters (V57): each says its share of the budget.
    expect(
      within(v).getByRole('button', { name: /^Budget used: \$[\d,.]+, \d+% of the budget\./ }),
    ).toBeInTheDocument()
    expect(
      within(v).getByRole('button', { name: /^Ungoverned: \$[\d,.]+, \d+% of the budget\./ }),
    ).toBeInTheDocument()
    expect(within(v).getByRole('button', { name: /^Control center: 5 to fix/ })).toBeInTheDocument()
    expect(within(v).getByRole('button', { name: 'Platform team' })).toBeInTheDocument()
    // A broken piece says what its failed runs cost (V56); a failed deploy costs nothing.
    expect(
      within(v).getByRole('button', {
        name: /^Code Reviewer, Broken, \$[\d.]+ spent on failed runs$/,
      }),
    ).toBeInTheDocument()
    expect(within(v).getByRole('button', { name: 'Doc Writer, Broken' })).toBeInTheDocument()
    expect(within(v).getByRole('button', { name: copy.place.tank })).toBeInTheDocument()
    expect(within(v).getByRole('button', { name: /^Claude Code, \d+ seats$/ })).toBeInTheDocument()
    expect(
      within(v).getByRole('button', { name: `Platform operations ${copy.place.project}` }),
    ).toBeInTheDocument()
    expect(screen.getByRole('radio', { name: overviewCopy.views.village })).toHaveAttribute(
      'aria-checked',
      'true',
    )
    // No map, and none of the map's filters until one is set.
    expect(screen.queryByTestId('overview-canvas')).toBeNull()
  })

  it('a team plot opens the insight panel on its team; open ground clears it', async () => {
    const user = userEvent.setup()
    const { router } = renderApp('/?view=village')
    const v = await village()
    await user.click(within(v).getByRole('button', { name: 'Platform team' }))
    const p = await screen.findByTestId('insight-panel', {}, { timeout: 5000 })
    expect(within(p).getAllByText('Platform').length).toBeGreaterThan(0)
    expect(router.state.location.search).toMatchObject({ sel: `node:${PLATFORM}` })
    expect(within(v).getByRole('button', { name: 'Platform team' })).toHaveAttribute(
      'aria-pressed',
      'true',
    )
    // The bar at the bottom names it; Hide details slides the drawer away and keeps the selection.
    await user.click(within(v).getByRole('button', { name: copy.selection.hide }))
    expect(within(v).getByRole('button', { name: copy.selection.details })).toBeInTheDocument()
    fireEvent.click(v.querySelector('svg') as SVGSVGElement)
    await waitFor(() => expect(router.state.location.search).not.toHaveProperty('sel'))
  })

  it('the town hall is governance: allocation, policies and guardrails', async () => {
    const user = userEvent.setup()
    renderApp('/?sel=place:townhall')
    const p = await screen.findByTestId('village-place-panel', {}, { timeout: 10_000 })
    expect(within(p).getByText(copy.place.townhall)).toBeInTheDocument()
    expect(within(p).queryByRole('tab', { name: 'Budgets' })).toBeNull()
    expect(within(p).getByText(copy.panel.allocation)).toBeInTheDocument()
    await user.click(within(p).getByRole('tab', { name: copy.panel.tabs.policy }))
    expect(within(p).getByText(copy.panel.policyNote)).toBeInTheDocument()
    expect(within(p).getByRole('link', { name: copy.panel.routerPolicy })).toHaveAttribute(
      'href',
      '/router',
    )
    await user.click(within(p).getByRole('tab', { name: copy.panel.tabs.guardrails }))
    expect(within(p).getByText(copy.panel.guardrailsNote)).toBeInTheDocument()
  })

  it('the token tank holds the budgets: the org’s month, each team’s line and where to set them', async () => {
    renderApp('/?sel=place:tank')
    const p = await screen.findByTestId('village-place-panel', {}, { timeout: 10_000 })
    expect(within(p).getByText(copy.place.tank)).toBeInTheDocument()
    expect(within(p).getByText(copy.panel.orgBudget)).toBeInTheDocument()
    expect(within(p).getByText('Platform')).toBeInTheDocument()
    expect(within(p).getByRole('link', { name: copy.panel.setBudgets })).toHaveAttribute(
      'href',
      '/router#router-budgets',
    )
  })

  it('the meters open the token tank and the piggy bank; the piggy bank links to Optimization', async () => {
    const user = userEvent.setup()
    const { router } = renderApp('/?view=village')
    const v = await village()
    await user.click(within(v).getByRole('button', { name: /^Budget used: / }))
    expect(router.state.location.search).toMatchObject({ sel: 'place:tank' })
    await user.click(within(v).getByRole('button', { name: /^Saved: / }))
    const p = await screen.findByTestId('village-place-panel')
    expect(within(p).getByText(copy.place.piggy)).toBeInTheDocument()
    expect(within(p).getByRole('link', { name: copy.panel.openOptimization })).toHaveAttribute(
      'href',
      '/optimization',
    )
    expect(router.state.location.search).toMatchObject({ sel: 'place:piggy' })
    await user.click(within(p).getByRole('button', { name: copy.close }))
    await waitFor(() => expect(router.state.location.search).not.toHaveProperty('sel'))
  })

  it('the Control center sorts what needs you, and Take me there selects the problem', async () => {
    const user = userEvent.setup()
    const { router } = renderApp('/?view=village')
    const v = await village()
    await user.click(within(v).getByRole('button', { name: /^Control center/ }))
    const log = within(v).getByRole('region', { name: copy.hud.log })
    expect(within(log).getByText(/^Code Reviewer has crashed 3 times since/)).toBeInTheDocument()
    expect(within(log).getByRole('link', { name: 'Open the agent' })).toHaveAttribute(
      'href',
      expect.stringMatching(/^\/agents\//),
    )
    await user.click(within(log).getByRole('tab', { name: /Upgrades/ }))
    expect(within(log).getByText('Route simple traffic to a smaller model')).toBeInTheDocument()
    await user.click(within(log).getAllByRole('button', { name: copy.takeMe })[0] as HTMLElement)
    expect(router.state.location.search).toMatchObject({ sel: 'opp:ov-o1' })
    expect(within(v).queryByRole('region', { name: copy.hud.log })).toBeNull()
  })

  it('each Control center count opens the list on its own tab', async () => {
    const user = userEvent.setup()
    renderApp('/?view=village')
    const v = await village()
    await user.click(within(v).getByRole('button', { name: copy.hud.countLabel('watch', 9) }))
    const log = within(v).getByRole('region', { name: copy.hud.log })
    expect(within(log).getByRole('tab', { name: /Watch/ })).toHaveAttribute('aria-selected', 'true')
    expect(
      within(v).getByRole('button', { name: copy.hud.countLabel('watch', 9) }),
    ).toHaveAttribute('aria-pressed', 'true')
  })

  it('the chat box hands a question to a new Orchestrator chat, as its draft', async () => {
    const user = userEvent.setup()
    const { router } = renderApp('/?view=village')
    const v = await village()
    // The find box is the Map's (Go to); the village's bottom centre is a chat box.
    expect(within(v).queryByRole('combobox', { name: copy.query.label })).toBeNull()
    const ask = within(v).getByRole('form', { name: copy.ask.label })
    expect(within(ask).getByRole('button', { name: copy.ask.send })).toBeDisabled()
    await user.type(
      within(ask).getByRole('textbox', { name: copy.ask.label }),
      'Why is Platform over?',
    )
    await user.click(within(ask).getByRole('button', { name: copy.ask.send }))
    await waitFor(() => expect(router.state.location.pathname).toBe('/chat'))
    expect(router.state.location.search).toMatchObject({ auto: 1 })
    // The question waits in the new routed chat's draft, for the full composer to send.
    expect(readDraft(ADMIN_ID, newChatDraftKey({ routed: true }))).toBe('Why is Platform over?')
  })

  it('Kiko walks through what is broken first, one at a time', async () => {
    const user = userEvent.setup()
    renderApp('/?view=village')
    const v = await village()
    await user.click(within(v).getByRole('button', { name: /^Kiko, your guide/ }))
    expect(within(v).getByText(/^Kiko · 1 of \d+$/)).toBeInTheDocument()
    expect(within(v).getByText(/^Code Reviewer has crashed 3 times since/)).toBeInTheDocument()
    await user.click(within(v).getByRole('button', { name: copy.kiko.next }))
    expect(within(v).getByText(/^Kiko · 2 of \d+$/)).toBeInTheDocument()
    await user.click(within(v).getByRole('button', { name: copy.kiko.hide }))
    expect(within(v).queryByText(/^Kiko · /)).toBeNull()
  })

  it('an old List link opens the Map, whose System card lists every part and picks one', async () => {
    const user = userEvent.setup()
    const { router } = renderApp('/?view=list')
    const v = await village()
    expect(await screen.findByTestId('overview-canvas')).toBeInTheDocument()
    // Folded to its counts by default, in the top left.
    expect(within(v).queryByRole('region', { name: boardCopy.system.title })).toBeNull()
    await user.click(within(v).getByRole('button', { name: boardCopy.system.openLabel }))
    const system = within(v).getByRole('region', { name: boardCopy.system.title })
    await user.click(within(system).getByRole('radio', { name: /^Needs attention/ }))
    await user.type(within(system).getByRole('searchbox'), 'jira')
    await user.click(within(system).getByRole('button', { name: /^jira-cloud/ }))
    expect(router.state.location.search).toMatchObject({
      sel: 'node:5eed000e-0000-4000-8000-000000000007',
    })
    // Folded again, its counts bring it back.
    await user.click(within(system).getByRole('button', { name: boardCopy.system.hide }))
    expect(within(v).queryByRole('region', { name: boardCopy.system.title })).toBeNull()
    await user.click(within(v).getByRole('button', { name: boardCopy.system.openLabel }))
    expect(within(v).getByRole('region', { name: boardCopy.system.title })).toBeInTheDocument()
  })

  it('Map is one switch away in the header, and it switches back', async () => {
    const user = userEvent.setup()
    const { router } = renderApp('/?view=village')
    await village()
    await user.click(screen.getByRole('radio', { name: overviewCopy.views.map }))
    expect(await screen.findByTestId('overview-canvas')).toBeInTheDocument()
    expect(router.state.location.search).toMatchObject({ view: 'map' })
    await user.click(screen.getByRole('radio', { name: overviewCopy.views.village }))
    expect(await village()).toBeInTheDocument()
    expect(router.state.location.search).toMatchObject({ view: 'village' })
  })

  it('Build lights green plots for a kind, and a plot opens the page where it is set up', async () => {
    const user = userEvent.setup()
    const { router } = renderApp('/?view=village')
    const v = await village()
    await user.click(within(v).getByRole('button', { name: copy.hud.buildLabel }))
    let shop = await screen.findByRole('dialog', { name: copy.shop.title })
    // Every router has its gate on the free plan, so the gate card goes straight to its page.
    expect(within(shop).getByRole('link', { name: /^LLM router/ })).toHaveAttribute(
      'href',
      '/router',
    )
    expect(
      within(shop)
        .getByText(/Team house/)
        .closest('a, button'),
    ).toBeNull()
    await user.click(within(shop).getByRole('button', { name: /^Tool/ }))
    await waitFor(() => expect(screen.queryByRole('dialog', { name: copy.shop.title })).toBeNull())
    expect(within(v).getByRole('status')).toHaveTextContent(copy.placing.hint('tool'))
    const plots = within(v).getAllByRole('button', { name: /^Plot \d+ for a new tool$/ })
    expect(plots.length).toBeGreaterThan(0)
    await user.click(plots[0] as HTMLElement)
    await waitFor(() => expect(router.state.location.pathname).toBe('/mcp'))
    const stored = JSON.parse(localStorage.getItem(PREFS_KEY) ?? '{}') as {
      state?: { claims?: Record<string, { plot: number }> }
    }
    expect(stored.state?.claims?.tool?.plot).toBeTypeOf('number')
    // The guardrails are to come; the rate-limit tower opens Flow limits (a section of Settings here).
    await router.navigate({ to: '/', search: { view: 'village' } })
    const back = await village()
    await user.click(within(back).getByRole('button', { name: copy.hud.buildLabel }))
    shop = await screen.findByRole('dialog', { name: copy.shop.title })
    await user.click(within(shop).getByRole('tab', { name: copy.shop.tabs.guardrails }))
    expect(within(shop).getByRole('link', { name: /Rate-limit tower/ })).toHaveAttribute(
      'href',
      '/settings',
    )
    expect(
      within(shop)
        .getByText(/^Wall$/)
        .closest('a, button'),
    ).toBeNull()
  })

  it('a placing run is cancelled by Escape or Cancel', async () => {
    const user = userEvent.setup()
    renderApp('/?view=village')
    const v = await village()
    await user.click(within(v).getByRole('button', { name: copy.hud.buildLabel }))
    const shop = await screen.findByRole('dialog', { name: copy.shop.title })
    await user.click(within(shop).getByRole('button', { name: /^Tool/ }))
    await user.click(await within(v).findByRole('button', { name: copy.placing.cancel }))
    expect(within(v).queryByRole('button', { name: /^Plot \d+ for a new tool$/ })).toBeNull()
  })

  it('Mine reads the viewer’s allowance and leaves out the ungoverned meter', async () => {
    renderApp('/?view=village&scope=mine')
    const v = await village()
    expect(within(v).getByRole('button', { name: /^Allowance used: \$/ })).toBeInTheDocument()
    expect(within(v).queryByRole('button', { name: /^Ungoverned/ })).toBeNull()
  })
})
