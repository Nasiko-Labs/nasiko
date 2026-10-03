/**
 * TokenOps drill-downs: the month calendar, the day panel (open, close, focus, legend filter) and the traces drawer
 * (day-scoped traces, row names, focus on close), plus closing without a duplicate history entry and the Spend over
 * time disclosure that ?day= forces open.
 */
import { screen, waitFor, within } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { describe, expect, it } from 'vitest'
import { configureMocks } from '@/mocks/handlers'
import { now, seed, section, setupPinnedSeed } from '@/test/pinnedSeed'
import { renderApp } from '@/test/renderApp'
import { recordRequests } from '@/test/setup'

const liveAgent = seed.agents.find((a) => !a.deleted)!
const spike = seed.agents.find((a) => a.name === seed.spikeAgentName)!
const names = (els: HTMLElement[]) => els.map((el) => (el.textContent ?? '').trim())

setupPinnedSeed()

describe('calendar', () => {
  it('arrow keys move the roving focus and never onto a future day', async () => {
    const user = userEvent.setup()
    renderApp('/tokenops?open=all')
    const grid = await screen.findByRole('grid')
    const cells = within(grid).getAllByRole('button')
    const today = cells.find((b) => b.tabIndex === 0)!
    const i = cells.indexOf(today)
    today.focus()
    await user.keyboard('{ArrowLeft}')
    expect(cells[i - 1]).toHaveFocus()
    expect(cells[i - 1].tabIndex).toBe(0)
    await user.keyboard('{ArrowUp}')
    expect(cells[i - 8]).toHaveFocus()
    await user.keyboard('{ArrowDown}{ArrowRight}')
    expect(cells[i]).toHaveFocus()
    await user.keyboard('{ArrowRight}')
    expect(cells[i]).toHaveFocus()
    expect(cells[i + 1]).toBeDisabled()
    await user.keyboard('{Home}')
    expect(cells[i]).toHaveFocus()
  })
})

describe('TokenOps disclosures forced open by URL state', () => {
  it('?day= opens Spend over time, a click still collapses it (keeping ?day), and a second click reopens it', async () => {
    const user = userEvent.setup()
    const { router } = renderApp(`/tokenops?day=${seed.spikeDate}`)
    const spend = await screen.findByRole('button', { name: /^Spend over time/ })
    expect(spend).toHaveAttribute('aria-expanded', 'true')

    await user.click(spend)
    await waitFor(() =>
      expect(screen.getByRole('button', { name: /^Spend over time/ })).toHaveAttribute(
        'aria-expanded',
        'false',
      ),
    )
    expect(router.state.location.search).toMatchObject({ day: seed.spikeDate })
    expect(router.state.location.search).not.toHaveProperty('open')

    await user.click(screen.getByRole('button', { name: /^Spend over time/ }))
    await waitFor(() =>
      expect(screen.getByRole('button', { name: /^Spend over time/ })).toHaveAttribute(
        'aria-expanded',
        'true',
      ),
    )
  })

  // Regression from the /ship adversarial review (2026-09-26): a collapsed disclosure reopens for a new drill-down.
  it('collapsing Spend over time for one day does not keep it closed for the next day', async () => {
    const user = userEvent.setup()
    const other = new Date(Date.parse(`${seed.spikeDate}T00:00:00Z`) - 86_400_000)
      .toISOString()
      .slice(0, 10)
    const { router } = renderApp(`/tokenops?day=${seed.spikeDate}`)
    const spend = await screen.findByRole('button', { name: /^Spend over time/ })
    await user.click(spend)
    await waitFor(() =>
      expect(screen.getByRole('button', { name: /^Spend over time/ })).toHaveAttribute(
        'aria-expanded',
        'false',
      ),
    )
    await router.navigate({
      to: '/tokenops',
      search: ((prev: Record<string, unknown>) => ({ ...prev, day: other })) as never,
    })
    await waitFor(() =>
      expect(screen.getByRole('button', { name: /^Spend over time/ })).toHaveAttribute(
        'aria-expanded',
        'true',
      ),
    )
  })
})

describe('day panel', () => {
  it('an empty day, Close, reopening from the timeline table, and the legend filter', async () => {
    const user = userEvent.setup()
    const { router } = renderApp('/tokenops?open=all&day=2025-06-01')
    const panel = await section(/hour by hour/)
    expect(await within(panel).findByText('No spend on this day')).toBeInTheDocument()
    await user.click(within(panel).getByRole('button', { name: 'Close day panel' }))
    await waitFor(() => expect(router.state.location.search).not.toHaveProperty('day'))

    // Timeline Table view: "Open day" opens the bucket's UTC day (the last row is the latest day).
    const timeline = await section(/Spend over time/)
    await user.click(await within(timeline).findByRole('button', { name: 'Table' }))
    const opens = within(timeline)
      .getAllByRole('button')
      .filter((b) => b.textContent?.startsWith('Open day'))
    await user.click(opens[opens.length - 1])
    await waitFor(() => expect(router.state.location.search).toMatchObject({ day: '2026-03-20' }))

    // Day panel legend: an agent entry (display name) filters the page by the agent's UUID.
    await router.navigate({
      to: '/tokenops',
      search: { open: 'all', preset: '30d', view: 'agent', sort: 'cost', day: seed.spikeDate },
    })
    const legend = await screen.findByRole('list', { name: 'Top agents this day' })
    await user.click(within(legend).getByRole('button', { name: new RegExp(spike.display_name) }))
    await waitFor(() =>
      expect(router.state.location.search).toMatchObject({ agent: spike.id, view: 'agent' }),
    )
  })
})

describe('day panel focus and filtering', () => {
  it('opening focuses the heading; picking another day keeps focus in the calendar; closing returns to it', async () => {
    const user = userEvent.setup()
    renderApp('/tokenops?open=all')
    const grid = await screen.findByRole('grid')
    const days = within(grid)
      .getAllByRole('button')
      .filter((b) => !b.hasAttribute('disabled'))
    await user.click(days[0])
    const heading = await screen.findByRole('heading', { name: /hour by hour/ })
    await waitFor(() => expect(heading).toHaveFocus())
    await user.click(days[1])
    await waitFor(() =>
      expect(screen.getByRole('heading', { name: /hour by hour/ })).toBeInTheDocument(),
    )
    expect(days[1]).toHaveFocus()
    await user.click(screen.getByRole('button', { name: 'Close day panel' }))
    await waitFor(() => expect(screen.queryByRole('heading', { name: /hour by hour/ })).toBeNull())
    await waitFor(() => expect(days[1]).toHaveFocus())
  })

  it('filtering from the panel keeps focus on its heading while the body reloads', async () => {
    const user = userEvent.setup()
    renderApp(`/tokenops?open=all&day=${seed.spikeDate}`)
    const legend = await screen.findByRole('list', { name: 'Top agents this day' })
    await user.click(within(legend).getByRole('button', { name: new RegExp(spike.display_name) }))
    await waitFor(() => expect(document.activeElement?.id).toBe('day-panel-title'))
  })

  it('a display name shared by two agents is not clickable (no guessing)', async () => {
    const twin = seed.agents.find((a) => !a.deleted && a.name !== spike.name)!
    const agents = seed.agents.map((a) =>
      a.name === twin.name ? { ...a, display_name: spike.display_name } : a,
    )
    configureMocks({ seed: { ...seed, agents }, now, loggedIn: true })
    renderApp(`/tokenops?open=all&day=${seed.spikeDate}`)
    const legend = await screen.findByRole('list', { name: 'Top agents this day' })
    expect(within(legend).getAllByText(spike.display_name).length).toBeGreaterThan(0)
    expect(
      within(legend).queryByRole('button', { name: new RegExp(spike.display_name) }),
    ).toBeNull()
  })
})

describe('day-scoped traces', () => {
  it("traces opened with a day selected are that UTC day's traces", async () => {
    const rec = recordRequests()
    try {
      renderApp(`/tokenops?open=all&day=${seed.spikeDate}&traces=true`)
      const drawer = await screen.findByRole('dialog')
      await waitFor(() => {
        const t = rec.urls.find((u) => u.pathname.endsWith('/top-traces'))
        expect(t?.searchParams.get('start_time')).toBe(`${seed.spikeDate}T00:00:00.000Z`)
        expect(t?.searchParams.get('range')).toBeNull()
      })
      expect(within(drawer).getByText(/\(UTC\)$/)).toBeInTheDocument()
    } finally {
      rec.stop()
    }
  })

  it('a future day is an empty window, not a silent 30 days', async () => {
    const rec = recordRequests()
    try {
      renderApp('/tokenops?open=all&day=2026-04-02&traces=true')
      await screen.findByRole('dialog')
      await waitFor(() => {
        const t = rec.urls.find((u) => u.pathname.endsWith('/top-traces'))
        expect(t).toBeTruthy()
        expect(t!.searchParams.get('start_time')).toBe(t!.searchParams.get('end_time'))
      })
    } finally {
      rec.stop()
    }
  })

  it('row "Traces" covers the row\'s whole window, so it leaves the day drill-down', async () => {
    const user = userEvent.setup()
    const { router } = renderApp(`/tokenops?open=all&day=${seed.spikeDate}`)
    const attribution = await section('Who is driving cost')
    await user.click(
      await within(attribution).findByRole('button', {
        name: `View traces for ${liveAgent.display_name}`,
      }),
    )
    await waitFor(() =>
      expect(router.state.location.search).toMatchObject({ agent: liveAgent.id, traces: true }),
    )
    expect(router.state.location.search).not.toHaveProperty('day')
  })
})

// Regression: ISSUE-004 (/qa 2026-09-26, .gstack/qa-reports/qa-report-localhost-3000-2026-09-26.md): every
// timeline row read "Open day" and every trace link "Open trace".
describe('row actions have distinct names', () => {
  it('timeline table: each "Open day" names its day, including hourly buckets', async () => {
    const user = userEvent.setup()
    renderApp('/tokenops?open=all&preset=24h')
    const panel = await section(/Spend over time/)
    await user.click(await within(panel).findByRole('button', { name: 'Table' }))
    const opens = within(panel)
      .getAllByRole('button')
      .filter((b) => b.textContent?.startsWith('Open day'))
    expect(opens.length).toBeGreaterThan(1)
    // 24h: hourly rows span at most two UTC days, so names repeat per day but always carry one
    // (format is locale-dependent: "Mar 19" or "19 Mar").
    for (const b of opens) expect(b).toHaveAccessibleName(expect.stringMatching(/^Open day.*\d/))
    expect(within(panel).getAllByRole('rowheader').length).toBe(opens.length)
  })

  it('traces drawer: each link names its agent, time and cost', async () => {
    renderApp('/tokenops?open=all&traces=true')
    const drawer = await screen.findByRole('dialog')
    const links = await within(drawer).findAllByRole('link', { name: /^Open trace:/ })
    expect(links.length).toBeGreaterThan(1)
    const all = names(links)
    expect(new Set(all).size).toBe(all.length)
    expect(all[0]).toMatch(
      /^Open trace: (seed|admin)-[a-z-]+, .*\d.*, .*\d \(opens in a new tab\)$/,
    )
  })
})

describe('closing panels', () => {
  it('closing a drawer this page opened goes back to where the user was (no duplicate entry)', async () => {
    const user = userEvent.setup()
    const { router } = renderApp('/tokenops?open=all')
    const attribution = await section('Who is driving cost')
    const before = router.state.location.href
    await user.click(
      await within(attribution).findByRole('button', {
        name: `View traces for ${liveAgent.display_name}`,
      }),
    )
    await screen.findByRole('dialog')
    await user.keyboard('{Escape}')
    await waitFor(() => expect(router.state.location.href).toBe(before))
    expect(screen.queryByRole('dialog')).toBeNull()
  })

  it('closing a day panel this page opened goes back too', async () => {
    const user = userEvent.setup()
    const { router } = renderApp('/tokenops?open=all')
    const grid = await screen.findByRole('grid')
    const before = router.state.location.href
    await user.click(
      within(grid)
        .getAllByRole('button')
        .find((b) => !b.hasAttribute('disabled'))!,
    )
    await screen.findByRole('heading', { name: /hour by hour/ })
    await user.click(screen.getByRole('button', { name: 'Close day panel' }))
    await waitFor(() => expect(router.state.location.href).toBe(before))
  })

  it('closing after a filter change made inside the panel keeps the filter (no Back)', async () => {
    const user = userEvent.setup()
    const { router } = renderApp('/tokenops?open=all')
    const grid = await screen.findByRole('grid')
    const label = new Date(`${seed.spikeDate}T00:00:00Z`).toLocaleDateString(undefined, {
      month: 'short',
      day: 'numeric',
      timeZone: 'UTC',
    })
    await user.click(
      within(grid)
        .getAllByRole('button')
        .find((b) => b.getAttribute('aria-label')?.startsWith(label))!,
    )
    const legend = await screen.findByRole('list', { name: 'Top agents this day' })
    await user.click(
      await within(legend).findByRole('button', { name: new RegExp(spike.display_name) }),
    )
    await waitFor(() => expect(router.state.location.search).toMatchObject({ agent: spike.id }))
    await user.click(screen.getByRole('button', { name: 'Close day panel' }))
    await waitFor(() => expect(router.state.location.search).not.toHaveProperty('day'))
    expect(router.state.location.search).toMatchObject({ agent: spike.id })
  })

  it('a deep-linked drawer closes by replacing (history depth unchanged)', async () => {
    const user = userEvent.setup()
    const { router } = renderApp('/tokenops?open=all&traces=true')
    await screen.findByRole('dialog')
    const depth = router.history.length
    await user.keyboard('{Escape}')
    await waitFor(() => expect(router.state.location.search).not.toHaveProperty('traces'))
    expect(router.history.length).toBe(depth)
  })
})

// Regression: ISSUE-005 (/qa 2026-09-26, .gstack/qa-reports/qa-report-localhost-3000-2026-09-26.md): closing the
// traces drawer dropped keyboard focus on <body>.
describe('traces drawer focus', () => {
  it('Escape returns focus to the day panel "View traces" button', async () => {
    const user = userEvent.setup()
    renderApp(`/tokenops?open=all&day=${seed.spikeDate}`)
    const panel = await section(/hour by hour/)
    const opener = within(panel).getByRole('button', { name: 'View traces' })
    opener.focus()
    await user.keyboard('{Enter}')
    const drawer = await screen.findByRole('dialog')
    await waitFor(() =>
      expect(within(drawer).getByRole('heading', { name: 'Most expensive traces' })).toHaveFocus(),
    )
    await user.keyboard('{Escape}')
    await waitFor(() => expect(screen.queryByRole('dialog')).toBeNull())
    await waitFor(() =>
      expect(within(panel).getByRole('button', { name: 'View traces' })).toHaveFocus(),
    )
  })

  it("a row-opened drawer returns focus to that row's Traces button", async () => {
    const user = userEvent.setup()
    renderApp('/tokenops?open=all')
    const attribution = await section('Who is driving cost')
    const name = `View traces for ${liveAgent.display_name}`
    await user.click(await within(attribution).findByRole('button', { name }))
    await screen.findByRole('dialog')
    await user.keyboard('{Escape}')
    await waitFor(() => expect(screen.queryByRole('dialog')).toBeNull())
    await waitFor(() => expect(document.activeElement).toHaveAccessibleName(name))
  })

  it('a drawer opened straight from the URL (no opener) leaves focus on <body> on close', async () => {
    const user = userEvent.setup()
    renderApp('/tokenops?open=all&traces=true')
    await screen.findByRole('dialog')
    await user.keyboard('{Escape}')
    await waitFor(() => expect(screen.queryByRole('dialog')).toBeNull())
    // Nothing opened it, so there is nothing to return to: Radix's default. Asserted
    // explicitly so any change to this outcome is deliberate.
    expect(document.activeElement).toBe(document.body)
  })
})
