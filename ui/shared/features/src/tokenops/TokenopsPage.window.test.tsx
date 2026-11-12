/**
 * The TokenOps time window: presets and Custom dates (an invalid range keeps the inputs and shows 30 d meanwhile,
 * edits replace history), the This-month KPI label, the narrow-screen window Select, and "now" moving only when the
 * user comes back to the page after 1 min+ (month boundary included).
 */
import { act, screen, waitFor, within } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { describe, expect, it, vi } from 'vitest'
import { configureMocks } from '@nasiko/mocks/handlers'
import { FIXED, now, seed, section, setupPinnedSeed } from '@nasiko/testing/pinnedSeed'
import { renderApp } from '@nasiko/testing/renderApp'
import { recordRequests } from '@nasiko/testing/setup'

setupPinnedSeed()

function setVisibility(state: 'hidden' | 'visible') {
  Object.defineProperty(document, 'visibilityState', { configurable: true, get: () => state })
  document.dispatchEvent(new Event('visibilitychange'))
}

function restoreClockAndVisibility() {
  vi.setSystemTime(FIXED)
  Object.defineProperty(document, 'visibilityState', { configurable: true, get: () => 'visible' })
}

async function choose(
  user: ReturnType<typeof userEvent.setup>,
  combobox: HTMLElement,
  option: string,
) {
  await user.click(combobox)
  await user.click(await screen.findByRole('option', { name: option }))
}

/** The Custom range's picker button (one per page: the top bar's on desktop). */
const rangeButton = () => screen.findByRole('button', { name: /^Custom range \(UTC\)/ })

/** Opens the picker and returns it. */
async function openPicker(user: ReturnType<typeof userEvent.setup>) {
  await user.click(await rangeButton())
  return screen.findByRole('dialog')
}

/** A day in the open picker, by its full name ("March 5th, 2026"). */
const day = (picker: HTMLElement, name: string) =>
  within(picker).getByRole('button', { name: new RegExp(`${name}, 2026`) })

// Regression: ISSUE-001 (/qa 2026-09-26, .gstack/qa-reports/qa-report-localhost-3000-2026-09-26.md): an invalid
// custom range hid the date inputs and lost the user's edit.
describe('invalid custom range', () => {
  it.each([
    ['inverted', '2026-03-10', '2026-03-01'],
    ['impossible date', '2026-02-30', '2026-03-02'],
    ['future', '2026-04-01', '2026-04-05'],
  ])('%s: keeps the range, flags it, and shows 30d meanwhile', async (_, from, to) => {
    renderApp(`/tokenops?open=all&preset=custom&from=${from}&to=${to}`)
    expect(await screen.findByLabelText('Key metrics, Last 30 days')).toBeInTheDocument()
    expect(await rangeButton()).toHaveAttribute('aria-invalid', 'true')
    expect(screen.getByRole('status')).toHaveTextContent('Invalid range, showing the last 30 days')
  })

  it('a valid range shows no warning, and a link to it leaves the picker shut', async () => {
    renderApp('/tokenops?open=all&preset=custom&from=2026-03-01&to=2026-03-10')
    const button = await rangeButton()
    expect(button).not.toHaveAttribute('aria-invalid')
    expect(button).toHaveAccessibleName('Custom range (UTC): Mar 1 – Mar 10')
    expect(screen.queryByText(/Invalid range/)).toBeNull()
    expect(screen.queryByRole('dialog')).toBeNull()
  })
})

describe('time control', () => {
  it('a preset replaces the window; Custom seeds today and opens the picker, where two clicks pick the range', async () => {
    const user = userEvent.setup()
    const { router } = renderApp('/tokenops?open=all')
    await screen.findByLabelText('Key metrics, Last 30 days')
    await user.click(screen.getByRole('radio', { name: '7d' }))
    expect(await screen.findByLabelText('Key metrics, Last 7 days')).toBeInTheDocument()
    await user.click(screen.getByRole('radio', { name: 'Custom' }))
    await waitFor(() =>
      expect(router.state.location.search).toMatchObject({
        preset: 'custom',
        from: '2026-03-20',
        to: '2026-03-20',
      }),
    )
    const picker = await screen.findByRole('dialog')
    await user.click(day(picker, 'March 5th'))
    await user.click(day(picker, 'March 10th'))
    await waitFor(() =>
      expect(router.state.location.search).toMatchObject({
        preset: 'custom',
        from: '2026-03-05',
        to: '2026-03-10',
      }),
    )
    await waitFor(() => expect(screen.queryByRole('dialog')).toBeNull())
  })

  // Regression (NAS-1516): the custom dates were native date inputs that saved every keystroke, so typing a year
  // refetched the page with years like 0002 and dragged the other end along. Months are paged in the picker, and only
  // a whole range is saved.
  it('pages to another month and saves the range only once both ends are picked', async () => {
    const user = userEvent.setup()
    const { router } = renderApp('/tokenops?open=all&preset=custom&from=2026-03-01&to=2026-03-10')
    const picker = await openPicker(user)
    expect(within(picker).getByText('Pick the first and last day (UTC)')).toBeInTheDocument()
    await user.click(within(picker).getByRole('button', { name: /previous month/i }))
    expect(within(picker).getByRole('grid', { name: /^January 2026/ })).toBeInTheDocument()
    expect(within(picker).getByRole('grid', { name: /^February 2026/ })).toBeInTheDocument()
    // No spill-over days: with two months they would show one day twice.
    expect(within(picker).queryByRole('button', { name: /December 31st, 2025/ })).toBeNull()
    await user.click(day(picker, 'February 1st'))
    expect(router.state.location.search).toMatchObject({ from: '2026-03-01', to: '2026-03-10' })
    expect(within(picker).getByText('Feb 1 – now pick the last day')).toBeInTheDocument()
    // The range follows the pointer before the second click.
    await user.hover(day(picker, 'February 10th'))
    expect(day(picker, 'February 5th')).toHaveAttribute('data-range-middle', 'true')
    await user.click(day(picker, 'February 28th'))
    await waitFor(() =>
      expect(router.state.location.search).toMatchObject({ from: '2026-02-01', to: '2026-02-28' }),
    )
    expect(await screen.findByRole('heading', { name: /February 2026/ })).toBeInTheDocument()
  })

  it('an end picked before the start swaps them; days after today and later months are off', async () => {
    const user = userEvent.setup()
    const { router } = renderApp('/tokenops?open=all&preset=custom&from=2026-03-01&to=2026-03-10')
    const picker = await openPicker(user)
    expect(day(picker, 'March 21st')).toBeDisabled()
    expect(within(picker).getByRole('button', { name: /next month/i })).toHaveAttribute(
      'aria-disabled',
      'true',
    )
    await user.click(day(picker, 'March 12th'))
    await user.click(day(picker, 'March 4th'))
    await waitFor(() =>
      expect(router.state.location.search).toMatchObject({ from: '2026-03-04', to: '2026-03-12' }),
    )
  })

  it('the month and year dropdowns jump months, back to January 2025 and no further', async () => {
    const user = userEvent.setup()
    renderApp('/tokenops?open=all&preset=custom&from=2026-03-01&to=2026-03-10')
    const picker = await openPicker(user)
    const year = within(picker).getAllByRole('combobox', { name: /year/i })[0]
    expect(
      within(year)
        .getAllByRole('option')
        .map((o) => o.textContent),
    ).toEqual(['2025', '2026'])
    await user.selectOptions(year, '2025')
    await user.selectOptions(within(picker).getAllByRole('combobox', { name: /month/i })[0], 'Jan')
    expect(within(picker).getByRole('grid', { name: /^January 2025/ })).toBeInTheDocument()
    expect(within(picker).getByRole('button', { name: /previous month/i })).toHaveAttribute(
      'aria-disabled',
      'true',
    )
  })

  it('picking a range replaces history instead of adding a Back step', async () => {
    const user = userEvent.setup()
    const { router } = renderApp('/tokenops?open=all&preset=30d')
    await screen.findByLabelText('Key metrics, Last 30 days')
    await user.click(screen.getAllByRole('radio', { name: 'Custom' })[0])
    const picker = await screen.findByRole('dialog')
    await user.click(day(picker, 'March 1st'))
    await user.click(day(picker, 'March 10th'))
    await waitFor(() =>
      expect(router.state.location.search).toMatchObject({
        preset: 'custom',
        from: '2026-03-01',
        to: '2026-03-10',
      }),
    )
    act(() => router.history.back())
    await waitFor(() => expect(router.state.location.search).toMatchObject({ preset: '30d' }))
  })

  it('This month labels what the KPI Δ compares against', async () => {
    renderApp('/tokenops?open=all&preset=mtd')
    await screen.findByLabelText('Key metrics, This month')
    const summary = screen.getByRole('region', { name: 'Summary' })
    expect(within(summary).getByText(/^vs the previous (day|\d+ days)$/)).toBeInTheDocument()
  })
})

describe('narrow screens', () => {
  it('the hero "Calendar" button discloses the grid, and the time-window Select sets the preset', async () => {
    vi.stubGlobal('matchMedia', (query: string) => ({
      matches: false,
      media: query,
      addEventListener: () => {},
      removeEventListener: () => {},
    }))
    const user = userEvent.setup()
    const { router } = renderApp('/tokenops?open=all')
    const hero = await section(/^This month/)
    const toggle = within(hero).getByRole('button', { name: 'Calendar' })
    const calendar = hero.querySelector('#month-calendar')!
    expect(toggle).toHaveAttribute('aria-expanded', 'false')
    expect(calendar.className).toMatch(/(^|\s)hidden(\s|$)/)
    await user.click(toggle)
    expect(toggle).toHaveAttribute('aria-expanded', 'true')
    expect(calendar.className).toMatch(/(^|\s)block(\s|$)/)

    const timeWindow = screen.getByRole('combobox', { name: 'Time window' })
    await choose(user, timeWindow, 'Last month')
    await waitFor(() =>
      expect(router.state.location.search).toMatchObject({ preset: 'last-month' }),
    )
    expect(await screen.findByLabelText(/^Key metrics, /)).toBeInTheDocument()
    await choose(user, screen.getByRole('combobox', { name: 'Time window' }), 'Custom')
    await waitFor(() =>
      expect(router.state.location.search).toMatchObject({
        preset: 'custom',
        from: '2026-03-20',
        to: '2026-03-20',
      }),
    )
    // The picker opens by itself here too, one month wide.
    const picker = await screen.findByRole('dialog')
    expect(within(picker).getAllByRole('grid')).toHaveLength(1)
  })
})

describe('coming back to the page', () => {
  it('moves the window and refreshes month-keyed data after 1 min+, not after a quick switch', async () => {
    const rec = recordRequests()
    try {
      renderApp('/tokenops?open=all&preset=mtd')
      await screen.findByLabelText('Key metrics, This month')
      // Current-window dashboard requests (the previous-window one ends at the month start).
      const dashEnds = () =>
        new Set(
          rec.urls
            .filter(
              (u) =>
                u.pathname.endsWith('/dashboard') &&
                u.searchParams.get('start_time') === '2026-03-01T00:00:00.000Z',
            )
            .map((u) => u.searchParams.get('end_time')),
        )
      const calendarCalls = () =>
        rec.urls.filter((u) => u.pathname.endsWith('/spend-calendar')).length
      await waitFor(() => expect(calendarCalls()).toBeGreaterThan(0))
      const calendarsBefore = calendarCalls()

      // 30 s after load: too soon, nothing moves.
      act(() => setVisibility('hidden'))
      vi.setSystemTime(new Date(FIXED.getTime() + 30_000))
      act(() => setVisibility('visible'))
      // 2 h later: the MTD end moves and the (month-keyed) calendar refetches too.
      act(() => setVisibility('hidden'))
      vi.setSystemTime(new Date(FIXED.getTime() + 2 * 3_600_000))
      act(() => setVisibility('visible'))
      await waitFor(() => expect(dashEnds().has('2026-03-20T17:00:00.000Z')).toBe(true))
      await waitFor(() => expect(calendarCalls()).toBeGreaterThan(calendarsBefore))
      // Only the load-time end and the 2 h one: the 30 s return never moved the window.
      expect([...dashEnds()].sort()).toEqual([
        '2026-03-20T15:00:00.000Z',
        '2026-03-20T17:00:00.000Z',
      ])
    } finally {
      restoreClockAndVisibility()
      rec.stop()
    }
  })

  it('crossing a UTC month boundary switches the hero to the new month', async () => {
    const rec = recordRequests()
    try {
      vi.setSystemTime(new Date('2026-03-31T23:30:00Z'))
      configureMocks({ seed, now: () => Date.now(), loggedIn: true })
      renderApp('/tokenops?open=all&preset=mtd')
      await screen.findByLabelText('Key metrics, This month')
      await waitFor(() =>
        expect(rec.urls.some((u) => u.searchParams.get('month') === '2026-03')).toBe(true),
      )
      act(() => setVisibility('hidden'))
      vi.setSystemTime(new Date('2026-04-01T01:00:00Z'))
      act(() => setVisibility('visible'))
      await waitFor(() =>
        expect(
          rec.urls.some(
            (u) =>
              u.pathname.endsWith('/spend-calendar') && u.searchParams.get('month') === '2026-04',
          ),
        ).toBe(true),
      )
      expect(await screen.findByText(/April 2026/)).toBeInTheDocument()
    } finally {
      configureMocks({ seed, now, loggedIn: true })
      restoreClockAndVisibility()
      rec.stop()
    }
  })
})

// Regression (user report 2026-11-05): picking another month left the month card on this one.
describe('the month card follows the window', () => {
  it('shows the whole of last month, without a forecast, when Last month is picked', async () => {
    renderApp('/tokenops?preset=last-month')
    const title = await screen.findByRole('heading', { name: /February 2026/ })
    expect(title).toHaveTextContent(/^Month/)
    const card = title.closest('section')!
    expect(await within(card).findByTestId('mtd-spend')).toHaveTextContent(/\$/)
    expect(within(card).queryByTestId('forecast')).toBeNull()
    expect(within(card).getByText(/the month before|No spend the month before/)).toBeInTheDocument()
  })

  it('stays on this month for a window that ends now', async () => {
    renderApp('/tokenops?preset=7d')
    const title = await screen.findByRole('heading', { name: /March 2026/ })
    expect(title).toHaveTextContent(/^This month/)
  })
})
