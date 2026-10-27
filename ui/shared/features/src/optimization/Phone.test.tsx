/**
 * /optimization below 640 px (plans/feat-optimization-page.md T9: R6B; eng C5): the header is the title and the window,
 * the jump to your settings sits under the lead, the chart draws weeks and a weekly bar picks its week, while the
 * Table view keeps the daily rows.
 */
import { screen, waitFor, within } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { configureMocks } from '@nasiko/mocks/handlers'
import { now, seed, setupPinnedSeed } from '@nasiko/testing/pinnedSeed'
import { renderApp } from '@nasiko/testing/renderApp'
import { copy } from './copy'

setupPinnedSeed()
const T = { timeout: 12_000 }

beforeEach(() => {
  // A 375 px phone: every min-width query is false.
  vi.stubGlobal('matchMedia', (query: string) => ({
    matches: !/min-width/.test(query),
    media: query,
    addEventListener: () => {},
    removeEventListener: () => {},
  }))
})
afterEach(() => {
  vi.unstubAllGlobals()
  configureMocks({ seed, now, loggedIn: true, variant: null, superuser: null })
})

describe('/optimization on a phone', () => {
  it('moves the settings link under the lead and draws weeks; the Table keeps days', async () => {
    renderApp('/optimization')
    const h = await screen.findByRole('heading', { level: 2, name: /^Saving/ }, T)
    const lead = h.closest('section') as HTMLElement
    const link = await within(lead).findByRole(
      'link',
      { name: /^Your settings: PACMS · Medium/ },
      T,
    )
    expect(link).toBeInTheDocument()
    expect(
      // The page's own header, not the app's top bar.
      within(screen.getAllByRole('banner').find((h) => h.dataset.slot !== 'topbar')!).queryByRole(
        'link',
        { name: /^Your settings/ },
      ),
    ).toBeNull()
    expect(within(lead).getByText(/Click a week to see its biggest senders\./)).toBeInTheDocument()
    await userEvent.click(within(lead).getByRole('button', { name: copy.lead.chart.table }))
    const rows = within(within(lead).getByRole('table')).getAllByRole('row')
    expect(rows.length).toBeGreaterThan(28)
  })

  it('a day picked from the Table view is a day (the chart’s weeks don’t change it)', async () => {
    const { router } = renderApp('/optimization')
    await userEvent.click(await screen.findByRole('button', { name: copy.lead.chart.table }, T))
    await userEvent.click(screen.getAllByRole('button', { name: /^Show requests/ }).at(-2)!)
    await waitFor(() => expect(router.state.location.search).toHaveProperty('slice'))
    const [a, b] = (router.state.location.search as { slice: string }).slice.split('/')
    expect(Date.parse(b!) - Date.parse(a!)).toBe(86_400_000)
  })
})
