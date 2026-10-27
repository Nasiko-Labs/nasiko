import { act, screen, waitFor, within } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { http, HttpResponse } from 'msw'
import { afterEach, describe, expect, it, vi } from 'vitest'
import { configureMocks, mockCtx } from '@nasiko/mocks/handlers'
import { optimizationMockState, TIER_DEFAULTS } from '@nasiko/mocks/optimization'
import { now, seed, setupPinnedSeed } from '@nasiko/testing/pinnedSeed'
import { recordRequestBodies, server } from '@nasiko/testing/setup'
import { renderApp } from '@nasiko/testing/renderApp'
import { copy } from './copy'

setupPinnedSeed()
afterEach(() => configureMocks({ seed, now, loggedIn: true, variant: null, superuser: null }))

const T = { timeout: 12_000 }
const heading = (n: number, title: string) => copy.section(n, title)

async function form() {
  const save = await screen.findByRole('button', { name: copy.save }, T)
  return save.closest('form') as HTMLElement
}

// eng E5(1): the M1 form cases, run where the form now lives.
describe('Your settings on /optimization#settings', () => {
  it('shows the four numbered sections on a server with the proposed routes (sketch v2 "full")', async () => {
    renderApp('/optimization#settings')
    const f = await form()
    expect(within(f).getByText(heading(1, copy.switchTitle))).toBeInTheDocument()
    expect(within(f).getByText(heading(2, copy.strategyTitle))).toBeInTheDocument()
    expect(within(f).getByText(heading(3, copy.budgetTitle))).toBeInTheDocument()
    expect(within(f).getByRole('switch', { name: heading(1, copy.switchTitle) })).toBeChecked()
    // 6B: each tile is named by its caption; its words describe it.
    const pacms = within(f).getByRole('radio', { name: copy.strategies.pacms.label })
    expect(pacms).toBeChecked()
    expect(pacms).toHaveAccessibleDescription(copy.strategies.pacms.hint)
    // 2B: the figure is there because the tiers route answered.
    expect(await within(f).findByText(/Medium ≈ 1,000 tokens\./, {}, T)).toBeInTheDocument()
    expect(within(f).getByRole('button', { name: copy.save })).toBeDisabled()
  })

  it("on today's server: no switch, renumbered from 1, no tier figure and no preview (1A, 2B)", async () => {
    configureMocks({ variant: 'optimization-classic' })
    renderApp('/optimization#settings')
    const f = await form()
    expect(screen.getByText(copy.subAlwaysOn)).toBeInTheDocument()
    expect(within(f).queryByRole('switch')).toBeNull()
    expect(within(f).getByText(heading(1, copy.strategyTitle))).toBeInTheDocument()
    expect(within(f).getByText(heading(2, copy.budgetTitle))).toBeInTheDocument()
    expect(within(f).queryByText(/≈/)).toBeNull()
    expect(within(f).queryByRole('table')).toBeNull()
  })

  it('Save sends only what changed, both routes at once, and says what saved (2C, 3A)', async () => {
    const rec = recordRequestBodies()
    renderApp('/optimization#settings')
    const f = await form()
    await userEvent.click(within(f).getByRole('radio', { name: copy.strategies.lastk.label }))
    await userEvent.click(within(f).getByRole('radio', { name: copy.level('high') }))
    await userEvent.click(within(f).getByRole('button', { name: copy.save }))
    expect(
      await screen.findByText(copy.saved([copy.fieldName.strategy, copy.fieldName.level]), {}, T),
    ).toBeInTheDocument()
    await rec.flush()
    const patches = rec.requests.filter((r) => r.method === 'PATCH')
    expect(patches.map((r) => [r.url.pathname, r.body])).toEqual(
      expect.arrayContaining([
        ['/api/me/context-strategy', { strategy: 'lastk' }],
        ['/api/me/pacms-budget', { level: 'high' }],
      ]),
    )
    expect(patches).toHaveLength(2)
    expect(optimizationMockState()).toMatchObject({ strategy: 'lastk', level: 'high' })
    // The remounted form starts clean from the server's values.
    await waitFor(() => expect(screen.getByRole('button', { name: copy.save })).toBeDisabled())
  })

  it('a half-saved Save keeps the failed draft with its error and lets Save retry it (2C)', async () => {
    server.use(
      http.patch('/api/me/pacms-budget', () =>
        HttpResponse.text('internal error', { status: 500 }),
      ),
    )
    renderApp('/optimization#settings')
    let f = await form()
    await userEvent.click(within(f).getByRole('radio', { name: copy.strategies.topk.label }))
    await userEvent.click(within(f).getByRole('radio', { name: copy.level('low') }))
    await userEvent.click(within(f).getByRole('button', { name: copy.save }))
    expect(
      await screen.findByText(copy.saved([copy.fieldName.strategy]), {}, T),
    ).toBeInTheDocument()
    expect(
      await screen.findByText(copy.fieldFailed(copy.fieldName.level, 'internal error'), {}, T),
    ).toBeInTheDocument()
    f = await form()
    expect(within(f).getByRole('radio', { name: copy.level('low') })).toBeChecked()
    expect(within(f).getByRole('radio', { name: copy.strategies.topk.label })).toBeChecked()
    expect(within(f).getByRole('button', { name: copy.save })).toBeEnabled()
    expect(optimizationMockState()).toMatchObject({ strategy: 'topk', level: 'medium' })
  })

  it('turning optimization off greys out strategy and budget (CX-5)', async () => {
    renderApp('/optimization#settings')
    const f = await form()
    await userEvent.click(within(f).getByRole('switch'))
    expect(within(f).getByText(copy.switchOffNote)).toBeInTheDocument()
    expect(within(f).getByRole('radio', { name: copy.strategies.pacms.label })).toBeDisabled()
  })

  it('the preview follows the draft strategy and shows the shared baseline (2A, 2F)', async () => {
    renderApp('/optimization#settings')
    const f = await form()
    const table = await within(f).findByRole('table', { name: copy.preview.label }, T)
    expect(within(table).getByText(copy.preview.without)).toBeInTheDocument()
    // R4C: the definition is said once on the page (under the chart); the baseline row carries the tooltip.
    expect(within(f).queryByText(copy.preview.estimates)).toBeNull()
    expect(within(f).getByText(/· with PACMS$/)).toBeInTheDocument()
    await userEvent.click(within(f).getByRole('radio', { name: copy.strategies.lastk.label }))
    expect(await within(f).findByText(/· with Last-K$/, {}, T)).toBeInTheDocument()
  })

  it('with no chats yet the preview says when it will appear (2F)', async () => {
    server.use(
      http.post('/api/me/context-preview', () =>
        HttpResponse.json({ error: 'no chats yet', code: 'no_session' }, { status: 404 }),
      ),
    )
    renderApp('/optimization#settings')
    expect(await screen.findByText(copy.preview.noChats, {}, T)).toBeInTheDocument()
  })

  it('a user who owns no agents reads "you own no agents" in the lead (eng F1, C1)', async () => {
    server.use(http.get('/api/agents', () => HttpResponse.json([])))
    renderApp('/optimization')
    expect(await screen.findByText(copy.lead.compression['no-agents'], {}, T)).toBeInTheDocument()
  })

  it('every agent on: the lead says compression is on, and the form has no Compression row (C1, P10)', async () => {
    for (const a of mockCtx.agents().agents) a.compress = true
    renderApp('/optimization')
    expect(await screen.findByText(copy.lead.compression['all-on'], {}, T)).toBeInTheDocument()
    const f = await form()
    expect(within(f).queryByText(/Compression/)).toBeNull()
  })

  it('some agents off: the lead leaves it to Needs attention (each fact once)', async () => {
    renderApp('/optimization')
    await screen.findByRole('region', { name: copy.attention.title }, T)
    for (const line of Object.values(copy.lead.compression))
      expect(screen.queryByText(line)).toBeNull()
  })

  it('a load failure shows the shared error with Retry', async () => {
    server.use(
      http.get('/api/me/context-strategy', () =>
        HttpResponse.text('internal error', { status: 500 }),
      ),
    )
    renderApp('/optimization#settings')
    expect(await screen.findByText(/internal error/, {}, T)).toBeInTheDocument()
    expect(screen.getByRole('button', { name: /Retry/ })).toBeInTheDocument()
  })

  // plans/feat-optimization-page.md P2, R1D; eng E5 assertion (2).
  it('the old Settings route lands on /optimization#settings with focus on Your settings; the sidebar marks Optimization', async () => {
    configureMocks({ superuser: false })
    const { router } = renderApp('/settings/optimization')
    await waitFor(() => expect(router.state.location.pathname).toBe('/optimization'), T)
    // The hash is dropped once it has moved focus, so Back/Forward never jumps again (review: red team).
    await waitFor(() => expect(router.state.location.hash).toBe(''))
    await form()
    const heading = screen.getByRole('heading', { level: 2, name: copy.hub.yourSettings })
    await waitFor(() => expect(heading).toHaveFocus())
    const item = await screen.findByRole('link', { name: 'Optimize' }, T)
    expect(item).toHaveAttribute('aria-current', 'page')
  })

  it('a 401 on Save takes the app expiry path, not a field error', async () => {
    server.use(
      http.patch('/api/me/pacms-budget', () =>
        HttpResponse.json(
          { data: null, status_code: 401, message: 'missing or invalid token' },
          { status: 401 },
        ),
      ),
    )
    const { router } = renderApp('/optimization#settings')
    const f = await form()
    await userEvent.click(within(f).getByRole('radio', { name: copy.level('low') }))
    await userEvent.click(within(f).getByRole('button', { name: copy.save }))
    await waitFor(() => expect(router.state.location.pathname).toBe('/login'), T)
    expect(screen.queryByText(/didn’t save/)).toBeNull()
  })

  it('saving only the switch sends the draft strategy with it (CX-5)', async () => {
    const rec = recordRequestBodies()
    renderApp('/optimization#settings')
    const f = await form()
    await userEvent.click(within(f).getByRole('switch'))
    await userEvent.click(within(f).getByRole('button', { name: copy.save }))
    expect(await screen.findByText(copy.saved([copy.fieldName.enabled]), {}, T)).toBeInTheDocument()
    await rec.flush()
    expect(
      rec.requests.filter((r) => r.method === 'PATCH').map((r) => [r.url.pathname, r.body]),
    ).toEqual([['/api/me/context-strategy', { strategy: 'pacms', enabled: false }]])
    expect(optimizationMockState().enabled).toBe(false)
  })

  it('a failed preview says so and Retry asks again; the form stays usable (2F)', async () => {
    let calls = 0
    server.use(
      http.post('/api/me/context-preview', () => {
        calls++
        return HttpResponse.text('internal error', { status: 500 })
      }),
    )
    renderApp('/optimization#settings')
    expect(await screen.findByText(copy.preview.failed, {}, T)).toBeInTheDocument()
    // E6: a costly dry run is not retried by the query itself.
    expect(calls).toBe(1)
    await userEvent.click(screen.getByRole('button', { name: copy.retry }))
    await waitFor(() => expect(calls).toBe(2))
    const f = await form()
    await userEvent.click(within(f).getByRole('radio', { name: copy.level('high') }))
    expect(within(f).getByRole('button', { name: copy.save })).toBeEnabled()
  })

  it('the server-wide switch off reads "off on this server" in the lead (2E, C1)', async () => {
    server.use(
      http.get('/api/settings/context-tiers', () =>
        HttpResponse.json({ ...TIER_DEFAULTS, compress_history: false }),
      ),
    )
    renderApp('/optimization')
    expect(await screen.findByText(copy.lead.compression['server-off'], {}, T)).toBeInTheDocument()
  })

  it('all agents on but the server can’t confirm reads "likely on" (review finding 1)', async () => {
    server.use(
      http.get('/api/agents', () =>
        HttpResponse.json([{ id: 'a1', name: 'one', compress_enabled: true, owner_id: 'x' }]),
      ),
    )
    configureMocks({ variant: 'optimization-classic' })
    renderApp('/optimization')
    expect(await screen.findByText(copy.lead.compression['likely-on'], {}, T)).toBeInTheDocument()
  })

  it('while unsaved, the Save bar sticks and says so (R2G)', async () => {
    renderApp('/optimization#settings')
    const f = await form()
    expect(within(f).queryByText(copy.unsaved)).toBeNull()
    await userEvent.click(within(f).getByRole('radio', { name: copy.level('high') }))
    const note = within(f).getByText(copy.unsaved)
    expect(note.parentElement?.className).toMatch(/sticky bottom-0/)
  })

  it('a window change or a sort keeps the draft without asking; leaving the page asks (C8)', async () => {
    const { router } = renderApp('/optimization#settings')
    const f = await form()
    await userEvent.click(within(f).getByRole('radio', { name: copy.level('high') }))
    await userEvent.click(screen.getAllByRole('radio', { name: '7d' })[0]!)
    await waitFor(() => expect(router.state.location.search).toMatchObject({ preset: '7d' }))
    expect(screen.queryByRole('alertdialog')).toBeNull()
    expect(within(await form()).getByRole('radio', { name: copy.level('high') })).toBeChecked()
    await userEvent.click(screen.getByRole('link', { name: 'Monitor' }))
    expect(await screen.findByRole('alertdialog', {}, T)).toBeInTheDocument()
    expect(router.state.location.pathname).toBe('/optimization')
  })
})

// eng E4: the dry run waits until Your settings is near the screen.
describe('the last-chat preview runs on view', () => {
  afterEach(() => vi.unstubAllGlobals())

  it('no preview request until the section intersects, then one', async () => {
    const observers: ((entries: { isIntersecting: boolean }[]) => void)[] = []
    vi.stubGlobal(
      'IntersectionObserver',
      class {
        constructor(cb: (entries: { isIntersecting: boolean }[]) => void) {
          observers.push(cb)
        }
        observe() {}
        disconnect() {}
      },
    )
    const rec = recordRequestBodies()
    renderApp('/optimization')
    await form()
    await screen.findByRole('heading', { level: 2, name: /^Saving/ }, T)
    await rec.flush()
    const previews = () => rec.requests.filter((r) => r.url.pathname === '/api/me/context-preview')
    expect(previews()).toHaveLength(0)
    expect(screen.getByTestId('preview-loading')).toBeInTheDocument()
    act(() => observers.forEach((cb) => cb([{ isIntersecting: true }])))
    expect(await screen.findByRole('table', { name: copy.preview.label }, T)).toBeInTheDocument()
    await rec.flush()
    expect(previews()).toHaveLength(1)
  })
})

describe('after a save', () => {
  it('the lead describes the change just saved (review: red team)', async () => {
    renderApp('/optimization#settings')
    const f = await form()
    await userEvent.click(within(f).getByRole('radio', { name: copy.strategies.lastk.label }))
    await userEvent.click(within(f).getByRole('button', { name: copy.save }))
    expect(
      await screen.findByText(/You switched to Last-K on \w+ \d+\./, {}, T),
    ).toBeInTheDocument()
  })
})
