/**
 * The Overview's map (`?view=map`, the Org / Fleet Runtime / TokenOps board on the village's canvas; the village is the
 * default view, village/VillageCanvas.test.tsx) and pins against the OV-1..OV-3 mocks: the board's cards inside the
 * village's HUD, a picked part filling, lighting what it touches and explaining itself in the canvas's drawer with its
 * problems' actions (I'll take it writes through OV-2), a picked card (lit, the rest faded, its insights and pages in
 * the drawer; nothing on the board navigates), the bill (its peak days open in the drawer, hovering it turns the lines
 * it pays for green), the month's budget, the savings and the opportunities on the board (a router lights them), the
 * tools as marks with one turned off lately, Go to, a selection from the URL, the scope switch, and a server without
 * the estate.
 */
import { cleanup, screen, waitFor, within } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { afterEach, beforeAll, describe, expect, it } from 'vitest'
import { configureMocks } from '@nasiko/mocks/handlers'
import { ADMIN_ID } from '@nasiko/mocks/seed-harness'
import { now, seed, setupPinnedSeed } from '@nasiko/testing/pinnedSeed'
import { renderApp } from '@nasiko/testing/renderApp'
import { recordRequestBodies, recordRequests } from '@nasiko/testing/setup'
import { copy } from './copy'
import { estateCopy } from './estate/copy'
import { boardCopy } from './fleet/copy'
import { villageCopy } from './village/copy'

setupPinnedSeed()
// The canvas is a lazy chunk: load it once up front, so the first test's render doesn't pay its transform.
beforeAll(() => import('./village/VillageCanvas'), 60_000)
afterEach(() => configureMocks({ seed, now, loggedIn: true, superuser: null, variant: null }))

const board = () => screen.findByRole('group', { name: boardCopy.label }, { timeout: 15_000 })
const details = () => screen.findByTestId('insight-panel', {}, { timeout: 10_000 })
const CODE_REVIEWER = '5eed0000-0001-4000-8000-000000000003'
const JIRA = /The jira-cloud tool fails 18% of calls/

describe('the map (the Org / Fleet Runtime / TokenOps board)', () => {
  it('draws Org, Fleet Runtime and TokenOps, each card counting what it holds', async () => {
    renderApp('/?view=map')
    const b = await board()
    expect(within(b).getByRole('heading', { name: boardCopy.org })).toBeInTheDocument()
    expect(within(b).getByRole('heading', { name: boardCopy.fleet })).toBeInTheDocument()
    expect(within(b).getByRole('heading', { name: boardCopy.tokenops })).toBeInTheDocument()
    for (const card of [
      boardCopy.teams,
      boardCopy.harness,
      boardCopy.agents,
      boardCopy.workflows,
      boardCopy.routers,
      boardCopy.providers,
      boardCopy.tools,
      boardCopy.totalSpend,
      boardCopy.opps.title,
      boardCopy.budget,
    ])
      expect(within(b).getByRole('heading', { name: card })).toBeInTheDocument()
    // Nothing on the board goes to another page by itself: the drawer's buttons do.
    expect(within(b).queryAllByRole('link')).toHaveLength(0)
    // Every team is in the Org, against its own budget.
    expect(
      within(b).getByRole('button', { name: /^Platform, \$\d[\d,]* of \$/ }),
    ).toBeInTheDocument()
  })

  it('a card picked lights, fades the rest, and opens its insights first, then its pages', async () => {
    const user = userEvent.setup()
    const { router } = renderApp('/?view=map')
    const b = await board()
    await user.click(within(b).getByRole('button', { name: boardCopy.cardLabel(boardCopy.agents) }))
    await waitFor(() => expect(router.state.location.search).toMatchObject({ sel: 'card:agents' }))
    expect(b.querySelector('[data-card="agents"]')).toHaveClass('fb-cardsel')
    expect(b.querySelector('[data-card="workflows"]')).toHaveClass('fb-cardfade')
    const d = await details()
    expect(within(d).getByRole('heading', { name: boardCopy.agents })).toBeInTheDocument()
    expect(d).toHaveTextContent(/agents spent \$[\d,]+; \d+ failing today\./)
    expect(within(d).getByText(boardCopy.detail.agentsKey)).toBeInTheDocument()
    expect(within(d).getByText(boardCopy.panel.attention)).toBeInTheDocument()
    expect(within(d).getByRole('link', { name: boardCopy.actions.agents })).toHaveAttribute(
      'href',
      '/agents',
    )
    expect(within(d).getByRole('link', { name: boardCopy.actions.deploy })).toHaveAttribute(
      'href',
      '/deploy',
    )
    // A click on a card's body (not a part in it) picks it too; Escape clears.
    await user.click(b.querySelector('[data-card="routers"]') as HTMLElement)
    await waitFor(() => expect(router.state.location.search).toMatchObject({ sel: 'card:routers' }))
    expect(await details()).toHaveTextContent(/routers? carry/)
    await user.keyboard('{Escape}')
    await waitFor(() => expect(router.state.location.search).not.toHaveProperty('sel'))
    expect(b.querySelector('.fb-cardfade')).toBeNull()
  })

  it('reads total spend as a bill: by provider and by source, to one total', async () => {
    renderApp('/?view=map')
    const b = await board()
    const bill = b.querySelector('[data-card="bill"]') as HTMLElement
    for (const line of [boardCopy.bill.byProvider, boardCopy.bill.bySource, boardCopy.bill.total])
      expect(within(bill).getByText(line)).toBeInTheDocument()
    const money = (label: string) =>
      Number(
        within(bill)
          .getByText(label)
          .closest('.fb-bline')
          ?.querySelector('.fb-val')
          ?.textContent?.replace(/[$,]/g, ''),
      )
    expect(money(boardCopy.bill.agents) + money(boardCopy.bill.harnesses)).toBeCloseTo(
      money(boardCopy.bill.total),
      1,
    )
    // A provider's line picks the provider.
    expect(within(bill).getAllByRole('button', { name: /on the bill$/ }).length).toBeGreaterThan(0)
  })

  it("a peak dot on the bill's line opens that day in the drawer; hovering the bill turns its lines green", async () => {
    const user = userEvent.setup()
    const { router } = renderApp('/?view=map')
    const b = await board()
    const bill = b.querySelector('[data-card="bill"]') as HTMLElement
    const dot = within(bill).getAllByRole('button', {
      name: /over the daily line\. Show the day$/,
    })[0]!
    await user.click(dot)
    await waitFor(() =>
      expect(router.state.location.search).toMatchObject({
        sel: expect.stringMatching(/^day:\d+$/),
      }),
    )
    const d = await details()
    expect(within(d).getByText(boardCopy.panel.spend)).toBeInTheDocument()
    expect(within(d).getByRole('link', { name: boardCopy.panel.dayTokenops })).toHaveAttribute(
      'href',
      expect.stringMatching(/^\/tokenops\?day=\d{4}-\d{2}-\d{2}/),
    )
    expect(bill).toHaveClass('fb-cardsel')
    // Hover: the board marks every line the bill pays for.
    await user.hover(bill)
    expect(screen.getByTestId('overview-canvas')).toHaveClass('fb-paying')
    await user.unhover(bill)
    expect(screen.getByTestId('overview-canvas')).not.toHaveClass('fb-paying')
  })

  it('a router picked lights the savings it carries; an unrelated pick dims them', async () => {
    const user = userEvent.setup()
    renderApp('/?view=map')
    const b = await board()
    const router = b.querySelector<HTMLElement>('.fb-router')!
    await user.click(router)
    await waitFor(() => expect(router).toHaveClass('fb-sel'))
    const saved = within(b).getByRole('button', { name: /^Saved \$[\d,.]+ from optimizations/ })
    expect(saved).toHaveClass('fb-rel')
    expect(b.querySelector('[data-card="opportunities"]')).toHaveClass('fb-rel')
  })

  it('shows tools as marks, most used first, and fades one turned off lately', async () => {
    renderApp('/?view=map')
    const b = await board()
    const tools = b.querySelector('[data-card="tools"]') as HTMLElement
    const off = within(tools).getByRole('button', { name: /^weather-api,.*Turned off 3 days ago/ })
    expect(off).toHaveClass('fb-off')
    // Turned off long ago: not on the board.
    expect(within(tools).queryByRole('button', { name: /^pdf-tools,/ })).toBeNull()
    // No filter tabs: the marks alone.
    expect(within(tools).queryByRole('radio')).toBeNull()
    expect(within(tools).getByRole('button', { name: /^github,/ })).toBeInTheDocument()
  })

  it("sits in the village's canvas: the same HUD, and switching view keeps the canvas", async () => {
    const user = userEvent.setup()
    const { router } = renderApp('/?view=map')
    await board()
    const canvas = screen.getByTestId('village-canvas')
    // No call to action on the Map (Build places on the village); zoom sits in the top tabs.
    expect(within(canvas).queryAllByRole('link')).toHaveLength(0)
    expect(within(canvas).queryByRole('button', { name: villageCopy.hud.buildLabel })).toBeNull()
    expect(within(canvas).getByRole('button', { name: villageCopy.hud.zoomIn })).toBeInTheDocument()
    // The chat box is the village's; the Map finds parts through Go to.
    expect(within(canvas).queryByRole('form', { name: villageCopy.ask.label })).toBeNull()
    // The meters are the village's: the board has the budget and the savings.
    expect(within(canvas).queryByRole('button', { name: /^Budget used:/ })).toBeNull()
    expect(screen.getByRole('radio', { name: copy.views.map })).toBeChecked()
    await user.click(screen.getByRole('radio', { name: copy.views.village }))
    await waitFor(() => expect(router.state.location.search).toMatchObject({ view: 'village' }))
    expect(screen.getByTestId('village-canvas')).toBe(canvas)
    expect(screen.queryByTestId('overview-canvas')).toBeNull()
  })

  it('a picked part keeps what it touches lit, explains itself, and picking it again clears it', async () => {
    const user = userEvent.setup()
    const { router } = renderApp('/?view=map')
    const b = await board()
    await user.click(within(b).getByRole('button', { name: /^Code Reviewer,/ }))
    await waitFor(() =>
      expect(router.state.location.search).toMatchObject({ sel: `node:${CODE_REVIEWER}` }),
    )
    const d = await details()
    expect(within(d).getByRole('heading', { name: 'Code Reviewer' })).toBeInTheDocument()
    expect(within(d).getByRole('link', { name: 'Open in Agents' })).toHaveAttribute(
      'href',
      `/agents/${CODE_REVIEWER}`,
    )
    // It fills; its router and its team stay lit; an agent it has nothing to do with dims.
    expect(within(b).getByRole('button', { name: /^Code Reviewer,/ })).toHaveClass('fb-sel')
    expect(within(b).getByRole('button', { name: /^Translator,/ })).toHaveClass('fb-dim')
    expect(b.querySelector('.fb-router:not(.fb-dim)')).not.toBeNull()
    expect(within(b).getByRole('button', { name: /^Platform,/ })).toHaveClass('fb-rel')
    await user.click(within(b).getByRole('button', { name: /^Code Reviewer,/ }))
    await waitFor(() => expect(router.state.location.search).not.toHaveProperty('sel'))
    expect(within(b).getByRole('button', { name: /^Code Reviewer,/ })).not.toHaveClass('fb-sel')
  })

  it('shows the month’s budget beside the spend, lights the teams for it, and highlights what was saved', async () => {
    const user = userEvent.setup()
    const { router } = renderApp('/?view=map')
    const b = await board()
    expect(within(b).getByText(boardCopy.ungoverned)).toBeInTheDocument()
    await user.click(within(b).getByRole('button', { name: /^Monthly budget: \$[\d,]+ of \$/ }))
    await waitFor(() => expect(router.state.location.search).toMatchObject({ sel: 'place:tank' }))
    expect(within(b).getByRole('button', { name: /^Platform,/ })).toHaveClass('fb-rel')
    expect(
      within(b).getByRole('button', { name: /^Saved \$[\d,.]+ from optimizations/ }),
    ).toBeInTheDocument()
    // The opportunities, largest first; one picked explains itself in the drawer.
    const opps = within(b).getAllByRole('button', { name: /saves about \$[\d.,]+ a month$/ })
    await user.click(opps[0]!)
    await waitFor(() =>
      expect(router.state.location.search).toMatchObject({ sel: expect.stringMatching(/^opp:/) }),
    )
    expect(await details()).toBeInTheDocument()
  })

  it('Go to finds a part and picks it, as a click on the board does', async () => {
    const user = userEvent.setup()
    const { router } = renderApp('/?view=map')
    await board()
    const canvas = screen.getByTestId('village-canvas')
    await user.click(within(canvas).getByRole('button', { name: villageCopy.query.goTo }))
    await user.type(
      within(canvas).getByRole('combobox', { name: villageCopy.query.label }),
      'platform',
    )
    await user.click(await within(canvas).findByRole('option', { name: /^Platform Team$/ }))
    expect(router.state.location.search).toMatchObject({
      sel: 'node:5eed0002-0001-4000-8000-000000000004',
    })
    expect(await details()).toBeInTheDocument()
  })

  it('a harness install fills, lights its kind, its team and its models, and dims the rest', async () => {
    const user = userEvent.setup()
    renderApp('/?view=map')
    const b = await board()
    const cell = within(b).getAllByRole('button', { name: /^Claude Code \(/ })[0]!
    await user.click(cell)
    expect(await details()).toBeInTheDocument()
    await waitFor(() => expect(cell).toHaveClass('fb-sel'))
    expect(b).toHaveClass('fb-focus')
    expect(within(b).getByRole('button', { name: /^Translator,/ })).toHaveClass('fb-dim')
  })

  it("a part's problem has its actions: I'll take it writes the owner through OV-2", async () => {
    const user = userEvent.setup()
    const rec = recordRequestBodies()
    renderApp('/?view=map')
    const b = await board()
    await user.click(within(b).getByRole('button', { name: /^jira-cloud,/ }))
    const d = await details()
    expect(await within(d).findByText(JIRA)).toBeInTheDocument()
    await user.click(within(d).getAllByRole('button', { name: estateCopy.card.take })[0]!)
    await rec.flush()
    const patch = rec.requests.find((r) => r.method === 'PATCH')
    rec.stop()
    expect(patch?.url.pathname).toMatch(/^\/api\/overview\/problems\//)
    expect(patch?.body).toEqual({ owner_id: ADMIN_ID })
  })

  it('a selection from the URL opens its part', async () => {
    renderApp(`/?view=map&sel=node:${CODE_REVIEWER}`)
    const d = await details()
    expect(await within(d).findByRole('heading', { name: 'Code Reviewer' })).toBeInTheDocument()
  })
})

describe('the view switch', () => {
  it('opens on the summary, reads no estate, and swaps only the summary for the canvas', async () => {
    const user = userEvent.setup()
    const rec = recordRequests()
    const { router } = renderApp('/')
    await screen.findByRole('region', { name: copy.summary }, { timeout: 10_000 })
    expect(screen.getByRole('heading', { name: copy.needs.title })).toBeInTheDocument()
    expect(screen.getByRole('radio', { name: copy.views.overview })).toBeChecked()
    expect(screen.queryByTestId('village-canvas')).toBeNull()
    // Org / Mine is the canvas's: the summary has no scope.
    expect(screen.queryByRole('radio', { name: estateCopy.scope.org })).toBeNull()
    expect(rec.urls.some((u) => u.pathname === '/api/overview/estate')).toBe(false)
    rec.stop()
    await user.click(screen.getByRole('radio', { name: copy.views.village }))
    await screen.findByTestId('village-canvas', {}, { timeout: 15_000 })
    expect(router.state.location.search).toMatchObject({ view: 'village' })
    // The canvas takes the summary's place; the cards under it stay.
    expect(screen.queryByRole('region', { name: copy.summary })).toBeNull()
    expect(screen.getByRole('heading', { name: copy.needs.title })).toBeInTheDocument()
    expect(screen.getByRole('radio', { name: estateCopy.scope.org })).toBeChecked()
    // Back to the summary drops the canvas's selection with it.
    await router.navigate({ to: '/', search: { view: 'village', sel: 'place:townhall' } })
    await user.click(screen.getByRole('radio', { name: copy.views.overview }))
    await screen.findByRole('region', { name: copy.summary })
    expect(router.state.location.search).not.toHaveProperty('view')
    expect(router.state.location.search).not.toHaveProperty('sel')
    expect(screen.queryByTestId('village-canvas')).toBeNull()
  })

  it('a bare / opens the last view picked, and a link with a view does not change it', async () => {
    const user = userEvent.setup()
    renderApp('/')
    await user.click(
      await screen.findByRole('radio', { name: copy.views.map }, { timeout: 10_000 }),
    )
    await board()
    cleanup()
    renderApp('/')
    await board()
    expect(screen.getByRole('radio', { name: copy.views.map })).toBeChecked()
    cleanup()
    renderApp('/?view=village')
    await screen.findByTestId('village-canvas', {}, { timeout: 15_000 })
    cleanup()
    renderApp('/')
    await board()
  })
})

describe('scope', () => {
  it('a member sees Mine only: no scope switch', async () => {
    configureMocks({ superuser: false })
    renderApp('/?view=map')
    await screen.findByTestId('overview-canvas', {}, { timeout: 10_000 })
    expect(screen.queryByRole('radio', { name: estateCopy.scope.org })).toBeNull()
  })
})

describe('a server without the estate (bare 404s)', () => {
  it('draws the smaller map from the fleet and says so', async () => {
    configureMocks({ variant: 'overview-absent' })
    renderApp('/?view=map')
    await board()
    expect(await screen.findByText(estateCopy.fallback.note)).toBeInTheDocument()
  })
})
