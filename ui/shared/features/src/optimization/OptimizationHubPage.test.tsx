/**
 * The /optimization lead (plans/feat-optimization-page.md T2: R1A, R2B, R2E, R4B, R5A, R7A, R7C; eng C1–C3, C5, C6)
 * against the mocked CX-V3a and CX-H reads.
 */
import { screen, waitFor, within } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { http, HttpResponse } from 'msw'
import { afterEach, describe, expect, it } from 'vitest'
import { configureMocks } from '@nasiko/mocks/handlers'
import { now, seed, setupPinnedSeed } from '@nasiko/testing/pinnedSeed'
import { recordRequests, server } from '@nasiko/testing/setup'
import { renderApp } from '@nasiko/testing/renderApp'
import { copy } from './copy'

setupPinnedSeed()
afterEach(() => configureMocks({ seed, now, loggedIn: true, variant: null, superuser: null }))

const T = { timeout: 12_000 }
const SAVINGS = '/api/observability/finops/context-savings'

/** One answer for the lead's read (the agents and top-requests reads keep their mocks). */
function savingsReply(body: Record<string, unknown>) {
  server.use(
    http.get(SAVINGS, () =>
      HttpResponse.json({
        data: {
          reports: 0,
          eligible_requests: 0,
          priced_reports: 0,
          pool_tokens: 0,
          sent_tokens: 0,
          pool_cost_usd: null,
          sent_cost_usd: null,
          messages_dropped: 0,
          compressed_bytes: 0,
          recorded_since: null,
          series: { bucket: 'day', points: [] },
          ...body,
        },
      }),
    ),
  )
}

describe('/optimization lead', () => {
  it('answers in the heading, then the five numbers, coverage and the TokenOps link (R1A, R4B, C2)', async () => {
    renderApp('/optimization')
    const h = await screen.findByRole(
      'heading',
      { level: 2, name: /^Saving ~\d+% of history tokens$/ },
      T,
    )
    const region = h.closest('section') as HTMLElement
    // A superuser sees every agent (P4); each figure is an estimate (R7A meta).
    expect(within(region).getByText(copy.lead.meta('Last 30 days', true))).toBeInTheDocument()
    expect(
      within(region).getByText(
        /^All agents sent ~[\d.,]+[kM]? tokens where they would have sent ~[\d.,]+[kM]?: ~[\d.,]+[kM]? fewer \(≈ \$[\d,.]+, \d+%\)\.$/,
      ),
    ).toBeInTheDocument()
    // C2: the mock reports one request in ten never, and recording began 14 days back, inside a 30-day window.
    expect(
      within(region).getByText(/Based on [\d,]+ of [\d,]+ requests \(\d+%\)\./),
    ).toBeInTheDocument()
    expect(
      within(region).getByText(/Recorded since \w+ \d+; earlier days aren’t counted\./),
    ).toBeInTheDocument()
    const link = within(region).getByRole('link', { name: copy.lead.tokenops })
    expect(link.getAttribute('href')).toMatch(/^\/tokenops\?preset=30d/)
  })

  it('describes your latest setting change, never as a cause (CX-H, C6)', async () => {
    renderApp('/optimization')
    expect(await screen.findByText(/You switched to PACMS on \w+ \d+\./, {}, T)).toBeInTheDocument()
    expect(screen.queryByText(/because|after you/i)).toBeNull()
  })

  it('a member reads "agents you can access" (R2E(3))', async () => {
    configureMocks({ superuser: false })
    renderApp('/optimization')
    expect(await screen.findByText(/^Agents you can access sent ~/, {}, T)).toBeInTheDocument()
    expect(screen.getByText(copy.lead.meta('Last 30 days', false))).toBeInTheDocument()
  })

  it('the Table view lists every day, hatched ones as not recorded, and picks a day (R5A, R7C, C5)', async () => {
    const { router } = renderApp('/optimization')
    const toggle = await screen.findByRole('button', { name: copy.lead.chart.table }, T)
    const region = toggle.closest('section') as HTMLElement
    await userEvent.click(toggle)
    const rows = within(within(region).getByRole('table')).getAllByRole('row')
    expect(rows.length).toBeGreaterThan(28)
    expect(within(region).getAllByText(copy.lead.chart.notRecorded).length).toBeGreaterThan(1)
    const pick = screen.getAllByRole('button', { name: /^Show requests/ }).at(-2)!
    const day = pick.textContent!.replace(copy.lead.chart.open, '').trim()
    await userEvent.click(pick)
    await waitFor(() => expect(router.state.location.search).toHaveProperty('slice'))
    const slice = (router.state.location.search as { slice: string }).slice
    expect(slice).toMatch(/^\d{4}-\d\d-\d\dT00:00:00\.000Z\/\d{4}-\d\d-\d\dT00:00:00\.000Z$/)
    expect(screen.getByText(copy.lead.slice.selected(day))).toBeInTheDocument()
    // Back undoes the pick (pushed); Clear replaces.
    router.history.back()
    await waitFor(() => expect(screen.queryByText(copy.lead.slice.selected(day))).toBeNull())
    router.history.forward()
    await userEvent.click(
      await screen.findByRole('button', { name: copy.lead.slice.clearLabel(day) }),
    )
    await waitFor(() => expect(router.state.location.search).not.toHaveProperty('slice'))
  })

  it('a new window clears the picked bar; 24h is hourly (C5, R5A)', async () => {
    const day = new Date(now() - 3 * 86_400_000)
    day.setUTCHours(0, 0, 0, 0)
    const slice = `${day.toISOString()}/${new Date(day.getTime() + 86_400_000).toISOString()}`
    const rec = recordRequests()
    const { router } = renderApp(`/optimization?slice=${encodeURIComponent(slice)}`)
    expect(await screen.findByText(/ selected$/, {}, T)).toBeInTheDocument()
    await userEvent.click(screen.getAllByRole('radio', { name: '24h' })[0]!)
    await waitFor(() => expect(router.state.location.search).not.toHaveProperty('slice'))
    expect(
      await screen.findByText(/Click an hour to see its biggest senders\./, {}, T),
    ).toBeInTheDocument()
    expect(
      rec.urls.some((u) => u.pathname === SAVINGS && u.searchParams.get('series') === 'hourly'),
    ).toBe(true)
    rec.stop()
  })

  it('ignores a slice from another window', async () => {
    renderApp(
      `/optimization?slice=${encodeURIComponent('2020-01-01T00:00:00.000Z/2020-01-02T00:00:00.000Z')}`,
    )
    await screen.findByRole('heading', { level: 2, name: /^Saving/ }, T)
    expect(screen.queryByText(/ selected$/)).toBeNull()
  })

  it("on today's server: the reason once, the chart's place says what will show (R2B)", async () => {
    configureMocks({ variant: 'optimization-classic' })
    renderApp('/optimization')
    expect(
      await screen.findByRole('heading', { level: 2, name: copy.lead.heading.absent }, T),
    ).toBeInTheDocument()
    expect(screen.getByText(copy.lead.absentLine)).toBeInTheDocument()
    expect(screen.getByText(copy.lead.chart.outline(false))).toBeInTheDocument()
    expect(screen.queryByRole('link', { name: copy.lead.tokenops })).toBeNull()
  })

  it('no reports yet: says so and offers a chat (R2E(1))', async () => {
    savingsReply({ eligible_requests: 12 })
    renderApp('/optimization')
    expect(
      await screen.findByRole('heading', { level: 2, name: copy.lead.heading.noReports }, T),
    ).toBeInTheDocument()
    expect(screen.getByRole('link', { name: copy.lead.startChat })).toHaveAttribute('href', '/chat')
  })

  it('under 50% coverage: no percentage anywhere in the lead (C2)', async () => {
    savingsReply({
      reports: 40,
      eligible_requests: 100,
      priced_reports: 40,
      pool_tokens: 100_000,
      sent_tokens: 40_000,
      pool_cost_usd: 1,
      sent_cost_usd: 0.4,
    })
    renderApp('/optimization')
    const h = await screen.findByRole(
      'heading',
      { level: 2, name: copy.lead.heading.lowCoverage },
      T,
    )
    const region = h.closest('section') as HTMLElement
    expect(within(region).getByText(/fewer \(≈ \$0\.60\)\.$/)).toBeInTheDocument()
    expect(within(region).getByText('Based on 40 of 100 requests (40%).')).toBeInTheDocument()
    // The figure tiles show the counts but no saved share either.
    expect(within(region).getByText('~60k')).toBeInTheDocument()
    expect(within(region).queryByText(/% fewer tokens$/)).toBeNull()
  })

  it('the lead bento: the answer, then without, sent, saved and not billed as figures', async () => {
    savingsReply({
      reports: 90,
      eligible_requests: 100,
      priced_reports: 90,
      pool_tokens: 100_000,
      sent_tokens: 40_000,
      pool_cost_usd: 1,
      sent_cost_usd: 0.4,
    })
    renderApp('/optimization')
    const h = await screen.findByRole(
      'heading',
      { level: 2, name: copy.lead.heading.saving(60) },
      T,
    )
    const region = h.closest('section') as HTMLElement
    expect(within(region).getByText('~100k')).toBeInTheDocument()
    expect(within(region).getByText('~40k')).toBeInTheDocument()
    expect(within(region).getByText(copy.lead.figures.fewer(60))).toBeInTheDocument()
    expect(within(region).getByText('≈ $0.60')).toBeInTheDocument()
    expect(within(region).getByText(copy.lead.figures.reported(90))).toBeInTheDocument()
  })

  it('cost covers only priced requests, and says so (C3)', async () => {
    savingsReply({
      reports: 100,
      eligible_requests: 100,
      priced_reports: 60,
      pool_tokens: 100_000,
      sent_tokens: 40_000,
      pool_cost_usd: 1,
      sent_cost_usd: 0.4,
    })
    renderApp('/optimization')
    expect(await screen.findByText(/Cost covers 60 of 100 requests\./, {}, T)).toBeInTheDocument()
  })

  it('a failed read fails the lead alone, with Retry (R2C)', async () => {
    server.use(http.get(SAVINGS, () => new HttpResponse('internal error', { status: 500 })))
    renderApp('/optimization')
    expect(
      await screen.findByRole('heading', { level: 2, name: copy.lead.heading.error }, T),
    ).toBeInTheDocument()
    expect(screen.getByRole('button', { name: /retry/i })).toBeInTheDocument()
    // Your settings still load.
    expect(await screen.findByRole('button', { name: copy.save }, T)).toBeInTheDocument()
  })

  it('the header names your settings and jumps to them (R1D)', async () => {
    renderApp('/optimization')
    const link = await screen.findByRole('link', { name: /^Your settings: PACMS · Medium/ }, T)
    expect(link.getAttribute('href')).toMatch(/#settings$/)
    await userEvent.click(link)
    await waitFor(() =>
      expect(screen.getByRole('heading', { level: 2, name: copy.hub.yourSettings })).toHaveFocus(),
    )
  })
  it('a failed history read leaves the lead as it is, with no change line and no error (review: testing)', async () => {
    server.use(
      http.get(
        '/api/me/settings-history',
        () => new HttpResponse('internal error', { status: 500 }),
      ),
    )
    renderApp('/optimization')
    const h = await screen.findByRole('heading', { level: 2, name: /^Saving/ }, T)
    const region = h.closest('section') as HTMLElement
    await within(region).findByText(/Based on/, {}, T)
    expect(within(region).queryByText(/You switched/)).toBeNull()
    expect(within(region).queryByRole('button', { name: /retry/i })).toBeNull()
  })

  it('a slice that starts before the window is clipped to it when sent (review: api-contract)', async () => {
    const start = Math.floor(now() / 60_000) * 60_000 - 86_400_000
    const hour = Math.floor(start / 3_600_000) * 3_600_000
    const slice = `${new Date(hour).toISOString()}/${new Date(hour + 3_600_000).toISOString()}`
    const rec = recordRequests()
    renderApp(`/optimization?preset=24h&slice=${encodeURIComponent(slice)}`)
    await waitFor(() => {
      const u = rec.urls.find((x) => x.pathname === `${SAVINGS}/top-requests`)
      expect(u?.searchParams.get('from')).toBe(new Date(start).toISOString())
    }, T)
    rec.stop()
  })
  it('a boolean enabled change reads as on/off (review: Codex)', async () => {
    server.use(
      http.get('/api/me/settings-history', () =>
        HttpResponse.json([
          {
            at: new Date(now() - 86_400_000).toISOString(),
            field: 'enabled',
            from: false,
            to: true,
          },
        ]),
      ),
    )
    renderApp('/optimization')
    expect(
      await screen.findByText(/You turned context optimization on on \w+ \d+\./, {}, T),
    ).toBeInTheDocument()
  })

  it('24h: every hourly bucket has its own label, so no two bars share a band (review: Codex)', async () => {
    renderApp('/optimization?preset=24h')
    await userEvent.click(await screen.findByRole('button', { name: copy.lead.chart.table }, T))
    const region = screen
      .getByRole('heading', { level: 2, name: /^Saving|^Too few|^No requests/ })
      .closest('section') as HTMLElement
    const labels = within(within(region).getByRole('table'))
      .getAllByRole('rowheader')
      .map((h) => h.textContent)
    expect(labels.length).toBeGreaterThan(20)
    expect(new Set(labels).size).toBe(labels.length)
  })
  it('an unparseable bucket time fails the lead alone, never the page (review: Codex)', async () => {
    savingsReply({
      reports: 10,
      eligible_requests: 10,
      pool_tokens: 1000,
      sent_tokens: 400,
      series: {
        bucket: 'day',
        points: [
          {
            bucket_start: 'not-a-date',
            eligible_requests: 1,
            reports: 1,
            pool_tokens: 1,
            sent_tokens: 1,
          },
        ],
      },
    })
    renderApp('/optimization')
    expect(
      await screen.findByRole('heading', { level: 2, name: copy.lead.heading.error }, T),
    ).toBeInTheDocument()
    expect(await screen.findByRole('button', { name: copy.save }, T)).toBeInTheDocument()
  })

  it('a picked bar sends the page’s absolute window, not a range the server would re-resolve (review: Codex)', async () => {
    const day = new Date(now() - 3 * 86_400_000)
    day.setUTCHours(0, 0, 0, 0)
    const slice = `${day.toISOString()}/${new Date(day.getTime() + 86_400_000).toISOString()}`
    const rec = recordRequests()
    renderApp(`/optimization?slice=${encodeURIComponent(slice)}`)
    await waitFor(() => {
      const u = rec.urls.find(
        (x) => x.pathname === `${SAVINGS}/top-requests` && x.searchParams.get('from'),
      )
      expect(u?.searchParams.get('start_time')).toBeTruthy()
      expect(u?.searchParams.get('range')).toBeNull()
    }, T)
    rec.stop()
  })
})
