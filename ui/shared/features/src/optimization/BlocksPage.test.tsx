/**
 * By agent and Biggest senders on /optimization (plans/feat-optimization-page.md T4: R1C, R2B, R2C, R4A, R7B, R7C;
 * eng C4, C5, C7).
 */
import { screen, waitFor, within } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { http, HttpResponse } from 'msw'
import { afterEach, describe, expect, it } from 'vitest'
import { configureMocks, mockCtx } from '@nasiko/mocks/handlers'
import { now, seed, setupPinnedSeed } from '@nasiko/testing/pinnedSeed'
import { renderApp } from '@nasiko/testing/renderApp'
import { recordRequests, server } from '@nasiko/testing/setup'
import { copy } from './copy'

setupPinnedSeed()
afterEach(() => configureMocks({ seed, now, loggedIn: true, variant: null, superuser: null }))

const T = { timeout: 12_000 }
const SAVINGS = '/api/observability/finops/context-savings'
const block = async (title: string) =>
  (await screen.findByRole('heading', { level: 2, name: title }, T)).closest(
    'section',
  ) as HTMLElement
const agentRows = (s: HTMLElement) => within(s).getAllByRole('row').slice(1)

describe('By agent', () => {
  it('lists the top 6 by history volume, with neutral On/Off badges and Show all (R7B, R4A)', async () => {
    const { router } = renderApp('/optimization')
    const s = await block(copy.byAgent.title)
    await waitFor(() => expect(agentRows(s)).toHaveLength(6), T)
    expect(within(s).getByText(/^Top 6 of \d+ by history volume\.$/)).toBeInTheDocument()
    const sorted = within(s).getByRole('columnheader', { name: /Without/ })
    expect(sorted).toHaveAttribute('aria-sort', 'descending')
    // Every badge is On or Off and matches the agent's switch.
    for (const r of agentRows(s)) {
      const name = within(r).getAllByRole('link')[0]!.textContent
      const agent = mockCtx.agents().agents.find((a) => a.display_name === name)!
      expect(
        within(r).getAllByText(agent.compress ? copy.byAgent.on : copy.byAgent.off).length,
      ).toBeGreaterThan(0)
    }
    const total = Number(/of (\d+)/.exec(within(s).getByText(/^Top 6 of/).textContent!)![1])
    await userEvent.click(within(s).getByRole('button', { name: copy.byAgent.showAll(total) }))
    await waitFor(() => expect(agentRows(s)).toHaveLength(total))
    expect(router.state.location.search).toMatchObject({ all: true })
  })

  it('a header re-sorts, kept in the URL by replacing history (R7B)', async () => {
    const { router } = renderApp('/optimization')
    const s = await block(copy.byAgent.title)
    await waitFor(() => expect(agentRows(s)).toHaveLength(6), T)
    const before = router.history.length
    await userEvent.click(within(s).getByRole('button', { name: copy.byAgent.sortBy('Requests') }))
    await waitFor(() => expect(router.state.location.search).toMatchObject({ sort: 'requests' }))
    expect(router.history.length).toBe(before)
    expect(within(s).getByRole('columnheader', { name: /Requests/ })).toHaveAttribute(
      'aria-sort',
      'descending',
    )
    const counts = agentRows(s).map((r) =>
      Number(within(r).getAllByRole('cell')[1]!.textContent!.replace(/,/g, '')),
    )
    expect(counts).toEqual([...counts].sort((a, b) => b - a))
  })

  it('badges flip after a bulk turn-on, in the same batch refetch as the strip (C7)', async () => {
    renderApp('/optimization?all=1')
    const s = await block(copy.byAgent.title)
    const off = mockCtx
      .agents()
      .agents.filter((a) => !a.deleted && a.owner_id !== mockCtx.viewer().id && !a.compress)
    const target = off[0]!
    await waitFor(() => {
      const row = within(s).getByRole('link', { name: target.display_name }).closest('tr')!
      expect(within(row).getAllByText(copy.byAgent.off).length).toBeGreaterThan(0)
    }, T)
    const strip = await screen.findByRole('region', { name: copy.attention.title }, T)
    await userEvent.click(
      within(strip).getByRole('button', { name: copy.attention.turnOnAll(off.length) }),
    )
    const dialog = await screen.findByRole('alertdialog')
    await userEvent.click(
      within(dialog).getByRole('button', { name: copy.attention.confirm(off.length) }),
    )
    await waitFor(() => {
      const row = within(s).getByRole('link', { name: target.display_name }).closest('tr')!
      expect(within(row).queryAllByText(copy.byAgent.off)).toHaveLength(0)
    }, T)
  })

  it('fails on its own with Retry (R2C)', async () => {
    server.use(
      http.get(`${SAVINGS}/agents`, () => new HttpResponse('internal error', { status: 500 })),
    )
    renderApp('/optimization')
    const s = await block(copy.byAgent.title)
    expect(await within(s).findByRole('button', { name: /retry/i }, T)).toBeInTheDocument()
    expect(await screen.findByRole('heading', { level: 2, name: /^Saving/ }, T)).toBeInTheDocument()
  })
})

describe('Biggest senders', () => {
  it('your chats open the trace at the request (R1C)', async () => {
    renderApp('/optimization')
    const s = await block(copy.senders.title)
    const links = await within(s).findAllByRole('link', { name: /^Review PR #\d+/ }, T)
    expect(links[0]!.getAttribute('href')).toMatch(
      /^\/sessions\/5eed-sess-pr-\d+\?trace=5eed[0-9a-f]+$/,
    )
    expect(within(s).getAllByText(/~[\d.,]+k? without → ~[\d.,]+k? sent/).length).toBe(5)
  })

  it('a member sees another user’s chat without its title or a link (C4)', async () => {
    configureMocks({ superuser: false })
    renderApp('/optimization')
    const s = await block(copy.senders.title)
    await within(s).findAllByText(/without →/, {}, T)
    const other = within(s).queryAllByText(copy.senders.otherChat)
    expect(other.length).toBeGreaterThan(0)
    for (const o of other) expect(o.closest('a')).toBeNull()
    const rows = within(s).getAllByRole('listitem')
    expect(rows.length).toBe(5)
  })

  it('a picked bar narrows the list to its interval; its chip names it and clears it (R7C, C5)', async () => {
    const rec = recordRequests()
    const { router } = renderApp('/optimization')
    await userEvent.click(await screen.findByRole('button', { name: copy.lead.chart.table }, T))
    const pick = screen.getAllByRole('button', { name: /^Show requests/ }).at(-2)!
    const day = pick.textContent!.replace(copy.lead.chart.open, '').trim()
    await userEvent.click(pick)
    const s = await block(copy.senders.title)
    expect(
      await within(s).findByText(`${day} · ${copy.senders.perMessage}`, {}, T),
    ).toBeInTheDocument()
    await waitFor(() =>
      expect(
        rec.urls.some(
          (u) => u.pathname === `${SAVINGS}/top-requests` && u.searchParams.get('from'),
        ),
      ).toBe(true),
    )
    await userEvent.click(within(s).getByRole('button', { name: copy.lead.slice.clearLabel(day) }))
    await waitFor(() => expect(router.state.location.search).not.toHaveProperty('slice'))
    rec.stop()
  })
})

describe("today's server (R2B)", () => {
  it('both blocks keep their place and say what will show there', async () => {
    configureMocks({ variant: 'optimization-classic' })
    renderApp('/optimization')
    expect(
      await within(await block(copy.byAgent.title)).findByText(copy.byAgent.outline, {}, T),
    ).toBeInTheDocument()
    expect(
      await within(await block(copy.senders.title)).findByText(copy.senders.outline, {}, T),
    ).toBeInTheDocument()
  })
})
