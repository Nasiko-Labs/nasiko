import { screen, waitFor, within } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { http, HttpResponse } from 'msw'
import { afterEach, describe, expect, it } from 'vitest'
import { configureMocks } from '@nasiko/mocks/handlers'
import { tiersMockState } from '@nasiko/mocks/optimization'
import { now, seed, setupPinnedSeed } from '@nasiko/testing/pinnedSeed'
import { recordRequestBodies, server } from '@nasiko/testing/setup'
import { renderApp } from '@nasiko/testing/renderApp'
import { copy } from './copy'

setupPinnedSeed()
afterEach(() => configureMocks({ seed, now, loggedIn: true, variant: null, superuser: null }))

const T = { timeout: 12000 }
const c = copy.tiers

describe('Settings → Workspace → Optimization tiers', () => {
  it("on today's server: labelled defaults as plain text with their variables, nothing to edit (2B)", async () => {
    configureMocks({ variant: 'optimization-classic' })
    renderApp('/settings/optimization-tiers')
    expect(await screen.findByText(c.defaultsTitle, {}, T)).toBeInTheDocument()
    expect(screen.getByText('PACMS_BUDGET_MEDIUM')).toBeInTheDocument()
    expect(screen.getByText('1,000 tokens')).toBeInTheDocument()
    expect(screen.getByText('1 message')).toBeInTheDocument()
    // V4: the server-wide History compression default is listed too, with its variable.
    expect(screen.getByText('TOKEN_COMPRESS_HISTORY')).toBeInTheDocument()
    expect(screen.getByText(c.on)).toBeInTheDocument()
    expect(screen.queryByRole('spinbutton')).toBeNull()
    expect(screen.queryByRole('switch')).toBeNull()
    expect(screen.queryByRole('button', { name: copy.save })).toBeNull()
  })

  it('numbers its three sections', async () => {
    renderApp('/settings/optimization-tiers')
    expect(await screen.findByText(copy.section(1, c.budgets), {}, T)).toBeInTheDocument()
    expect(screen.getByText(copy.section(2, c.counts))).toBeInTheDocument()
    expect(screen.getByText(copy.section(3, c.shared))).toBeInTheDocument()
  })

  it('keeps Low ≤ Medium ≤ High and the pool ≥ always kept, before sending', async () => {
    const rec = recordRequestBodies()
    renderApp('/settings/optimization-tiers')
    const budgets = (await screen.findByText(copy.section(1, c.budgets), {}, T)).closest('section')!
    const medium = within(budgets).getByRole('spinbutton', { name: 'Medium' })
    await userEvent.clear(medium)
    await userEvent.type(medium, '400')
    const pool = screen.getByRole('spinbutton', { name: c.pool })
    await userEvent.clear(pool)
    await userEvent.type(pool, '2')
    await userEvent.click(screen.getByRole('button', { name: copy.save }))
    expect(await screen.findByText(c.problem.order)).toBeInTheDocument()
    expect(screen.getByText(c.problem.pool)).toBeInTheDocument()
    expect(medium).toHaveAttribute('aria-invalid', 'true')
    await rec.flush()
    expect(rec.requests.some((r) => r.method === 'PUT')).toBe(false)
  })

  it('saves the whole set and reports it; Account → Optimization then shows the new figure', async () => {
    const rec = recordRequestBodies()
    const { router } = renderApp('/settings/optimization-tiers')
    const budgets = (await screen.findByText(copy.section(1, c.budgets), {}, T)).closest('section')!
    const save = screen.getByRole('button', { name: copy.save })
    expect(save).toBeDisabled()
    const medium = within(budgets).getByRole('spinbutton', { name: 'Medium' })
    await userEvent.clear(medium)
    await userEvent.type(medium, '1500')
    expect(save).toBeEnabled()
    await userEvent.click(save)
    expect(await screen.findByText(c.saved, {}, T)).toBeInTheDocument()
    await rec.flush()
    const put = rec.requests.find((r) => r.method === 'PUT')!
    expect(put.body).toMatchObject({
      pacms_budget: { low: 500, medium: 1500, high: 5000 },
      context_k: { low: 1, medium: 5, high: 20 },
      pool_size: 150,
      compress_history: true,
    })
    expect(tiersMockState().pacms_budget.medium).toBe(1500)
    await router.navigate({ to: '/settings/optimization' })
    expect(await screen.findByText(/Medium ≈ 1,500 tokens\./, {}, T)).toBeInTheDocument()
  })

  it('a member is sent to their settings on /optimization, with no tiers row in the Settings nav', async () => {
    configureMocks({ superuser: false })
    const { router } = renderApp('/settings/optimization-tiers')
    await waitFor(() => expect(router.state.location.pathname).toBe('/optimization'), T)
    // The jump's hash is dropped once it has moved focus (review: red team).
    await waitFor(() => expect(router.state.location.hash).toBe(''))
    await router.navigate({ to: '/settings/secrets' })
    // The settings nav has rendered (Secrets is every user's) before the absence is checked.
    expect(await screen.findByRole('link', { name: 'Secrets' }, T)).toBeInTheDocument()
    expect(screen.queryByRole('link', { name: c.title })).toBeNull()
  })

  it('an admin finds it under Workspace', async () => {
    renderApp('/settings/secrets')
    const link = await screen.findByRole('link', { name: c.title }, T)
    expect(link).toHaveAttribute('href', '/settings/optimization-tiers')
  })

  it('keeps History compression in Shared limits, with the one Save in that card (V4)', async () => {
    renderApp('/settings/optimization-tiers')
    const shared = (await screen.findByText(copy.section(3, c.shared), {}, T)).closest('section')!
    expect(within(shared).getByRole('switch', { name: c.compressHistory })).toBeInTheDocument()
    expect(within(shared).getByRole('button', { name: copy.save })).toBeInTheDocument()
  })

  async function editMedium(value: string) {
    const budgets = (await screen.findByText(copy.section(1, c.budgets), {}, T)).closest('section')!
    const medium = within(budgets).getByRole('spinbutton', { name: 'Medium' })
    await userEvent.clear(medium)
    await userEvent.type(medium, value)
    return medium
  }

  it('a rejected save keeps the draft and shows the server’s words', async () => {
    server.use(
      http.put('/api/settings/context-tiers', () =>
        HttpResponse.text('pool too large for this server', { status: 400 }),
      ),
    )
    renderApp('/settings/optimization-tiers')
    const medium = await editMedium('1500')
    await userEvent.click(screen.getByRole('button', { name: copy.save }))
    expect(await screen.findByText('pool too large for this server', {}, T)).toBeInTheDocument()
    expect(medium).toHaveValue(1500)
    expect(screen.getByRole('button', { name: copy.save })).toBeEnabled()
    expect(tiersMockState().pacms_budget.medium).toBe(1000)
  })

  it('sends the version it loaded; a save that lost the race keeps the draft and offers the new values (V2)', async () => {
    const rec = recordRequestBodies()
    renderApp('/settings/optimization-tiers')
    const loaded = tiersMockState().updated_at
    const medium = await editMedium('1500')
    // Another admin saves first.
    tiersMockState().updated_at = '2026-09-30T00:00:00.000Z'
    tiersMockState().pacms_budget.medium = 2000
    await userEvent.click(screen.getByRole('button', { name: copy.save }))
    expect(await screen.findByText(c.conflict, {}, T)).toBeInTheDocument()
    await rec.flush()
    expect(rec.requests.find((r) => r.method === 'PUT')?.body).toMatchObject({
      expected_updated_at: loaded,
    })
    expect(medium).toHaveValue(1500)
    expect(tiersMockState().pacms_budget.medium).toBe(2000)
    // The stale draft can't be sent again under the new version: Save waits for "Load their values".
    expect(screen.getByRole('button', { name: copy.save })).toBeDisabled()
    await userEvent.click(screen.getByRole('button', { name: c.conflictAction }))
    await waitFor(
      () =>
        expect(
          within(screen.getByText(copy.section(1, c.budgets)).closest('section')!).getByRole(
            'spinbutton',
            { name: 'Medium' },
          ),
        ).toHaveValue(2000),
      T,
    )
    expect(screen.queryByText(c.conflict)).toBeNull()
    rec.stop()
  })

  it('a failed read of their values keeps the draft (V2)', async () => {
    renderApp('/settings/optimization-tiers')
    const medium = await editMedium('1500')
    tiersMockState().updated_at = '2026-09-30T00:00:00.000Z'
    await userEvent.click(screen.getByRole('button', { name: copy.save }))
    await screen.findByText(c.conflict, {}, T)
    server.use(
      http.get('/api/settings/context-tiers', () =>
        HttpResponse.text('bad request', { status: 400 }),
      ),
    )
    await userEvent.click(screen.getByRole('button', { name: c.conflictAction }))
    expect(await screen.findByText(c.conflictLoadFailed, {}, T)).toBeInTheDocument()
    expect(medium).toHaveValue(1500)
    expect(screen.getByText(c.conflict)).toBeInTheDocument()
  })

  it('a reply missing a value stays read-only: nothing it didn’t read is ever sent (V2)', async () => {
    server.use(
      http.get('/api/settings/context-tiers', () =>
        HttpResponse.json({
          pacms_budget: { low: 600, medium: 1200, high: 6000 },
          context_k: { low: 1, medium: 5, high: 20 },
          compress_history: false,
        }),
      ),
    )
    renderApp('/settings/optimization-tiers')
    expect(await screen.findByText(c.partialTitle, {}, T)).toBeInTheDocument()
    expect(screen.getByText('1,200 tokens')).toBeInTheDocument()
    expect(screen.getByText(c.off)).toBeInTheDocument()
    expect(screen.queryByRole('spinbutton')).toBeNull()
    expect(screen.queryByRole('button', { name: copy.save })).toBeNull()
  })

  it('a failed read shows Retry, never the defaults or a form', async () => {
    server.use(
      http.get('/api/settings/context-tiers', () =>
        HttpResponse.text('internal error', { status: 500 }),
      ),
    )
    renderApp('/settings/optimization-tiers')
    expect(
      await screen.findByText(new RegExp(`Couldn't load ${c.what}`), {}, { timeout: 20000 }),
    ).toBeInTheDocument()
    expect(screen.getByRole('button', { name: /retry/i })).toBeInTheDocument()
    expect(screen.queryByText(c.defaultsTitle)).toBeNull()
    expect(screen.queryByRole('spinbutton')).toBeNull()
  })
})
