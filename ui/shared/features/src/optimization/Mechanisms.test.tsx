/**
 * What's optimizing your tokens on /optimization (plans/feat-optimization-page.md §11, ledger B12).
 */
import { screen, waitFor, within } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { http, HttpResponse } from 'msw'
import { afterEach, describe, expect, it } from 'vitest'
import { savings as savingsSample } from '@nasiko/mocks/aggregate'
import { configureMocks, mockCtx } from '@nasiko/mocks/handlers'
import { ADMIN_ID } from '@nasiko/mocks/seed-harness'
import { now, seed, setupPinnedSeed } from '@nasiko/testing/pinnedSeed'
import { renderApp } from '@nasiko/testing/renderApp'
import { server } from '@nasiko/testing/setup'
import { copy } from './copy'

setupPinnedSeed()
afterEach(() => configureMocks({ seed, now, loggedIn: true, variant: null, superuser: null }))

const T = { timeout: 12_000 }
const SAVINGS = '/api/observability/finops/savings'
const m = copy.mechanisms
const block = () => screen.findByRole('region', { name: m.title }, T)
const row = (b: HTMLElement, name: string | RegExp) =>
  within(b).getByText(name).closest('li') as HTMLElement
const live = () => mockCtx.agents().agents.filter((a) => !a.deleted)

/** The savings reply with its programs changed, the rest the mock's sample. */
const withPrograms = (
  programs: Partial<ReturnType<typeof savingsSample>['by_program'][number]>[],
) => {
  const s = savingsSample()
  return {
    ...s,
    by_program: s.by_program.map((p, i) => ({ ...p, ...(programs[i] ?? {}) })),
  }
}

describe("What's optimizing your tokens", () => {
  it('names each mechanism, what it trims, where it is set and its state across the workspace (B12)', async () => {
    renderApp('/optimization')
    const b = await block()
    const history = row(b, m.history)
    expect(await within(history).findByText('PACMS · Medium', {}, T)).toBeInTheDocument()
    expect(
      within(history).getByRole('link', { name: m.historyWhere }).getAttribute('href'),
    ).toMatch(/^\/optimization\?.*#settings$/)
    expect(within(history).getByRole('link', { name: m.tiersWhere })).toHaveAttribute(
      'href',
      '/settings/optimization-tiers',
    )
    // savings.rs counts every live agent in the workspace; the mock counts the agents mock's switches.
    const on = live().filter((a) => a.compress).length
    const caveman = row(b, 'Smaller prompts')
    expect(
      await within(caveman).findByText(m.agentsOn(on, live().length), {}, T),
    ).toBeInTheDocument()
    expect(within(caveman).getByRole('link', { name: m.yourAgents })).toHaveAttribute(
      'href',
      '/agents/mine',
    )
    expect(row(b, 'Less code written')).toHaveTextContent(/Minimal-code mode/)
    expect(row(b, m.comments)).toHaveTextContent(/in the workspace/)
  })

  it('introduces PACMS in a tooltip on focus (B12)', async () => {
    renderApp('/optimization')
    const b = await block()
    await within(b).findByText(m.history, {}, T)
    within(b).getByText(m.history).focus()
    expect((await screen.findAllByText(copy.codenames.pacms, {}, T)).length).toBeGreaterThan(0)
  })

  it('takes the program name from the server’s label, not the fallback copy', async () => {
    server.use(
      http.get(SAVINGS, () =>
        HttpResponse.json({
          data: withPrograms([{ label: 'Trimmed prompts' }]),
          status_code: 200,
          message: 'ok',
        }),
      ),
    )
    renderApp('/optimization')
    const b = await block()
    expect(await within(b).findByText('Trimmed prompts', {}, T)).toBeInTheDocument()
    expect(within(b).queryByText(m.caveman)).toBeNull()
  })

  it('shows saved tokens only above zero, marked estimated unless measured', async () => {
    server.use(
      http.get(SAVINGS, () =>
        HttpResponse.json({
          data: withPrograms([
            { saved_tokens: 1_200_000, basis: 'measured' },
            { saved_tokens: 0, basis: 'seed_default' },
          ]),
          status_code: 200,
          message: 'ok',
        }),
      ),
    )
    renderApp('/optimization')
    const b = await block()
    const caveman = row(b, m.caveman)
    expect(await within(caveman).findByText(/^~.+ tokens saved$/, {}, T)).toBeInTheDocument()
    expect(within(caveman).queryByText(/estimated/)).toBeNull()
    expect(within(row(b, m.ponytail)).queryByText(/tokens saved/)).toBeNull()
  })

  it('marks a saving that isn’t measured as estimated', async () => {
    server.use(
      http.get(SAVINGS, () =>
        HttpResponse.json({
          data: withPrograms([{ saved_tokens: 900, basis: 'seed_default' }]),
          status_code: 200,
          message: 'ok',
        }),
      ),
    )
    renderApp('/optimization')
    const caveman = row(await block(), m.caveman)
    expect(
      await within(caveman).findByText(/^~.+ tokens saved \(estimated\)$/, {}, T),
    ).toBeInTheDocument()
  })

  it('reads Off when history selection is switched off (CX-5)', async () => {
    server.use(
      http.get('/api/me/context-strategy', () =>
        HttpResponse.json({ strategy: 'pacms', enabled: false }),
      ),
    )
    renderApp('/optimization')
    const history = row(await block(), m.history)
    expect(await within(history).findByText(m.off, {}, T)).toBeInTheDocument()
  })

  it('a failed preference read leaves the history row without a state, never a forever skeleton (R2C)', async () => {
    server.use(http.get('/api/me/pacms-budget', () => new HttpResponse('boom', { status: 400 })))
    renderApp('/optimization')
    const history = row(await block(), m.history)
    await waitFor(() => expect(history.querySelector('[data-slot="skeleton"]')).toBeNull(), T)
    expect(within(history).queryByText(/PACMS ·/)).toBeNull()
  })

  it('a member gets no tiers link (superusers set tier sizes)', async () => {
    configureMocks({ superuser: false })
    renderApp('/optimization')
    const history = row(await block(), m.history)
    expect(within(history).queryByRole('link', { name: m.tiersWhere })).toBeNull()
  })

  it('a server without /finops/savings keeps the rows and links and says what will show (R2B)', async () => {
    server.use(http.get(SAVINGS, () => new HttpResponse(null, { status: 404 })))
    renderApp('/optimization')
    const b = await block()
    expect(await within(b).findByText(m.absent, {}, T)).toBeInTheDocument()
    expect(within(row(b, m.caveman)).queryByText(/agents on/)).toBeNull()
    expect(within(row(b, m.caveman)).getByRole('link', { name: m.yourAgents })).toBeInTheDocument()
    // Your own setting still shows: it doesn't come from the savings read.
    expect(await within(row(b, m.history)).findByText('PACMS · Medium', {}, T)).toBeInTheDocument()
  })

  it('a failed read says so, and Retry brings the counts back', async () => {
    let fail = true
    server.use(
      http.get(SAVINGS, () =>
        fail ? new HttpResponse('internal error', { status: 500 }) : undefined,
      ),
    )
    renderApp('/optimization')
    const b = await block()
    expect(await within(b).findByText(m.failed, {}, T)).toBeInTheDocument()
    fail = false
    await userEvent.click(within(b).getByRole('button', { name: copy.retry }))
    expect(
      await within(row(b, m.caveman)).findByText(/workspace agents on/, {}, T),
    ).toBeInTheDocument()
    expect(within(b).queryByText(m.failed)).toBeNull()
  })

  it('the bulk turn-on in Needs attention moves the caveman count (review: performance, Codex)', async () => {
    const mine = live().filter((a) => a.owner_id === ADMIN_ID && !a.compress).length
    const before = live().filter((a) => a.compress).length
    renderApp('/optimization')
    const b = await block()
    const caveman = row(b, m.caveman)
    await within(caveman).findByText(m.agentsOn(before, live().length), {}, T)
    const strip = await screen.findByRole('region', { name: copy.attention.title }, T)
    await userEvent.click(
      within(strip).getByRole('button', { name: copy.attention.turnOnMine(mine) }),
    )
    const dialog = await screen.findByRole('alertdialog')
    await userEvent.click(
      within(dialog).getByRole('button', { name: copy.attention.confirm(mine) }),
    )
    expect(
      await within(caveman).findByText(m.agentsOn(before + mine, live().length), {}, T),
    ).toBeInTheDocument()
  })
})
