/**
 * The TokenOps time window: presets and Custom dates (an invalid range keeps the inputs and shows 30 d meanwhile,
 * edits replace history), the This-month KPI label, the narrow-screen window Select, and "now" moving only when the
 * user comes back to the page after 1 min+ (month boundary included).
 */
import { act, fireEvent, screen, waitFor, within } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { describe, expect, it, vi } from 'vitest'
import { configureMocks } from '@/mocks/handlers'
import { FIXED, now, seed, section, setupPinnedSeed } from '@/test/pinnedSeed'
import { renderApp } from '@/test/renderApp'
import { recordRequests } from '@/test/setup'

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

// Regression: ISSUE-001 (/qa 2026-09-26, .gstack/qa-reports/qa-report-localhost-3000-2026-09-26.md): an invalid
// custom range hid the date inputs and lost the user's edit.
describe('invalid custom range', () => {
  it.each([
    ['inverted', '2026-03-10', '2026-03-01'],
    ['impossible date', '2026-02-30', '2026-03-02'],
    ['future', '2026-04-01', '2026-04-05'],
  ])('%s: keeps the inputs, flags the range, and shows 30d meanwhile', async (_, from, to) => {
    renderApp(`/tokenops?open=all&preset=custom&from=${from}&to=${to}`)
    expect(await screen.findByLabelText('Key metrics, Last 30 days')).toBeInTheDocument()
    expect(screen.getByLabelText('From (UTC)')).toBeInTheDocument()
    expect(screen.getByLabelText('To (UTC)')).toHaveAttribute('aria-invalid', 'true')
    expect(screen.getByRole('status')).toHaveTextContent('Invalid range, showing the last 30 days')
  })

  it('a valid range shows no warning', async () => {
    renderApp('/tokenops?open=all&preset=custom&from=2026-03-01&to=2026-03-10')
    expect(await screen.findByLabelText('To (UTC)')).not.toHaveAttribute('aria-invalid')
    expect(screen.queryByText(/Invalid range/)).toBeNull()
  })
})

describe('time control', () => {
  it('a preset replaces the window; Custom seeds today and edits the From date', async () => {
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
    fireEvent.change(screen.getByLabelText('From (UTC)'), { target: { value: '2026-03-05' } })
    await waitFor(() =>
      expect(router.state.location.search).toMatchObject({
        preset: 'custom',
        from: '2026-03-05',
        to: '2026-03-20',
      }),
    )
    fireEvent.change(screen.getByLabelText('To (UTC)'), { target: { value: '2026-03-10' } })
    await waitFor(() =>
      expect(router.state.location.search).toMatchObject({ from: '2026-03-05', to: '2026-03-10' }),
    )
  })

  it('editing custom dates replaces history instead of adding a Back step per change', async () => {
    const user = userEvent.setup()
    const { router } = renderApp('/tokenops?open=all&preset=30d')
    await screen.findByLabelText('Key metrics, Last 30 days')
    await user.click(screen.getAllByRole('radio', { name: 'Custom' })[0])
    const from = await screen.findByLabelText('From (UTC)')
    await user.clear(from)
    await user.type(from, '2026-03-01')
    await user.clear(screen.getByLabelText('To (UTC)'))
    await user.type(screen.getByLabelText('To (UTC)'), '2026-03-10')
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
    const kpis = await screen.findByLabelText('Key metrics, This month')
    expect(within(kpis).getByText(/^Δ vs the previous (day|\d+ days)$/)).toBeInTheDocument()
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
