/**
 * Page-level tests: the real router + page + app QueryClient against the seed-backed MSW
 * handlers (same handlers as the browser). Time is pinned so results don't depend on the
 * run date.
 */
import { screen, waitFor, within } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { delay, http, HttpResponse } from 'msw'
import { describe, expect, it } from 'vitest'
import { configureMocks } from '@/mocks/handlers'
import { generateHarnessSeed } from '@/mocks/seed-harness'
import { FIXED, now, seed, section, setupPinnedSeed } from '@/test/pinnedSeed'
import { renderApp } from '@/test/renderApp'
import { recordRequests, server } from '@/test/setup'

const FINOPS = '/api/observability/finops'

setupPinnedSeed()

const liveAgent = seed.agents.find((a) => !a.deleted)!
const deletedAgent = seed.agents.find((a) => a.deleted)!

describe('TokenOps page', () => {
  it('renders F1, KPIs, and an attribution table with display names only', async () => {
    renderApp('/tokenops?open=all')
    expect(await screen.findByTestId('mtd-spend')).toHaveTextContent(/\$/)
    const table = await within(await section('Who is driving cost')).findByRole('table')
    expect(within(table).getAllByText(liveAgent.display_name).length).toBeGreaterThan(0)
    expect(within(table).queryByText(liveAgent.name)).toBeNull()
  })

  it('shows savings per category and per agent, with both reduction percentages', async () => {
    renderApp('/tokenops?open=optimise')
    const panel = await section('Token optimisation savings')

    // Both percentages, always together: the gap between them is real (savings are input-side,
    // input is the cheap side) and showing only one invites the reader to assume they match.
    // Each appears twice — once in the hero, once on the category row that produced it.
    expect(within(panel).getAllByText('19.7%')).toHaveLength(2)
    expect(within(panel).getAllByText('11.0%')).toHaveLength(2)

    // The category split — "what did Caveman save vs Ponytail" — answered directly.
    expect(within(panel).getByText('Smaller prompts')).toBeInTheDocument()
    expect(within(panel).getByText('Less code written')).toBeInTheDocument()

    // A zero category stays visible and says why, so it reads as "nobody turned it on" rather
    // than "this feature does nothing".
    expect(within(panel).getByText(/No coding agent has this turned on/)).toBeInTheDocument()

    expect(within(panel).getByRole('link', { name: /Turn on for an agent/ })).toHaveAttribute(
      'href',
      '/agents',
    )
  })

  it('lists the sessions the optimiser helped most', async () => {
    // Savings recur per turn, so a long session compounds them in a way the per-call view
    // understates. This table is where that shows, and it links back to the conversation.
    renderApp('/tokenops?open=optimise')
    const panel = await section('Token optimisation savings')
    const link = within(panel).getByRole('link', { name: /ctx-9f3ad21b/ })
    expect(link).toHaveAttribute('href', '/sessions/ctx-9f3ad21b0c74')
    expect(within(panel).getByText('22.4%')).toBeInTheDocument()
  })

  it('names the layers underneath a category that has more than one', async () => {
    // "Caveman saved this much" is the product question; which injection point carried it is the
    // engineering one, and both belong on the same row.
    renderApp('/tokenops?open=optimise')
    const panel = await section('Token optimisation savings')
    // Named for what got shorter, not for the injection point the ledger stores. Scoped to the
    // row, because the panel's own intro also talks about tool output in plain words.
    const row = within(panel).getByRole('rowheader', { name: /Smaller prompts/ })
    expect(row).toHaveTextContent(/tool output/)
    expect(row).toHaveTextContent(/chat history/)
    expect(row).not.toHaveTextContent(/compress_/)
  })

  it('marks a figure that is estimated rather than measured', async () => {
    // The minimal-code row has no observable counterfactual, so its number is a factor applied to
    // counted traffic. That has to be visible rather than blended into the measured total.
    renderApp('/tokenops?open=optimise')
    const panel = await section('Token optimisation savings')
    expect(within(panel).getAllByLabelText('Estimated').length).toBeGreaterThan(0)
    expect(within(panel).getByText('Measured + estimated')).toBeInTheDocument()
  })

  it('shows one page loader under the sticky bar on a cold load; a window change keeps the panels', async () => {
    server.use(
      http.get(`*${FINOPS}/dashboard`, async () => {
        await delay(300)
        return undefined
      }),
    )
    const { router } = renderApp('/tokenops?open=all')
    // The header is static, so it stays above the loader.
    await screen.findByRole('heading', { level: 1, name: 'TokenOps' })
    expect(screen.getByTestId('page-loader')).toBeInTheDocument()
    expect(screen.queryByText('Spend over time')).toBeNull()
    await screen.findByTestId('mtd-spend')
    expect(screen.queryByTestId('page-loader')).toBeNull()
    await router.navigate({ to: '/tokenops', search: { open: 'all', preset: '7d' } as never })
    expect(screen.queryByTestId('page-loader')).toBeNull()
    expect(screen.getByTestId('mtd-spend')).toBeInTheDocument()
  })

  it('sends range only for rolling presets, explicit UTC bounds for This month', async () => {
    const rec = recordRequests()
    try {
      const { router } = renderApp('/tokenops?open=all&preset=7d')
      await screen.findByTestId('mtd-spend')
      await waitFor(() =>
        expect(
          rec.urls.some(
            (u) => u.pathname.endsWith('/dashboard') && u.searchParams.get('range') === '7d',
          ),
        ).toBe(true),
      )
      await router.navigate({
        to: '/tokenops',
        search: { open: 'all', preset: 'mtd', view: 'agent', sort: 'cost' },
      })
      await waitFor(() => {
        const mtd = rec.urls.find(
          (u) =>
            u.pathname.endsWith('/dashboard') &&
            u.searchParams.get('start_time') === '2026-03-01T00:00:00.000Z',
        )
        expect(mtd).toBeTruthy()
        expect(mtd?.searchParams.get('range')).toBeNull()
      })
    } finally {
      rec.stop()
    }
  })

  it('clicking a calendar day opens the day panel; Back closes it', async () => {
    const user = userEvent.setup()
    const { router } = renderApp('/tokenops?open=all')
    const grid = await screen.findByRole('grid')
    const firstDay = within(grid)
      .getAllByRole('button')
      .find((b) => !b.hasAttribute('disabled'))!
    await user.click(firstDay)
    expect(await screen.findByRole('heading', { name: /hour by hour/ })).toBeInTheDocument()
    expect(router.state.location.search).toHaveProperty('day')
    router.history.back()
    await waitFor(() =>
      expect(screen.queryByRole('heading', { name: /hour by hour/ })).not.toBeInTheDocument(),
    )
  })

  it('spike day: names the spike agent, Table view filters by agent UUID, View traces opens the drawer', async () => {
    const user = userEvent.setup()
    const spike = seed.agents.find((a) => a.name === seed.spikeAgentName)!
    const { router } = renderApp(`/tokenops?open=all&day=${seed.spikeDate}`)
    const list = await screen.findByRole('list', { name: 'Top agents this day' })
    // Day slices carry display names, like the real server.
    expect(within(list).getByText(spike.display_name)).toBeInTheDocument()
    await user.click(within(await section(/hour by hour/)).getByRole('button', { name: 'Table' }))
    const hours = within(await section(/hour by hour/)).getByRole('table')
    expect(within(hours).getByRole('columnheader', { name: 'Hour (UTC)' })).toBeInTheDocument()
    await user.click(within(hours).getByRole('button', { name: spike.display_name }))
    // The display name is mapped to the agent's UUID: the server rejects display names as agent_id.
    await waitFor(() => expect(router.state.location.search).toMatchObject({ agent: spike.id }))
    await user.click(
      within(await section(/hour by hour/)).getByRole('button', { name: 'View traces' }),
    )
    const drawer = await screen.findByRole('dialog')
    expect(within(drawer).getByRole('heading', { name: 'Most expensive traces' })).toHaveFocus()
    expect(await within(drawer).findAllByText(/Open trace/)).not.toHaveLength(0)
  })

  it('F4: a 404 explains the missing endpoint', async () => {
    server.use(
      http.get(`${FINOPS}/top-traces`, () => new HttpResponse('not found', { status: 404 })),
    )
    renderApp('/tokenops?open=all&traces=true')
    expect(await screen.findByText(/needs a newer nasiko-server/)).toBeInTheDocument()
  })

  it('F4: a 500 offers Retry, not the newer-server message', async () => {
    server.use(
      http.get(`${FINOPS}/top-traces`, () => new HttpResponse('internal error', { status: 500 })),
    )
    renderApp('/tokenops?open=all&traces=true')
    const drawer = await screen.findByRole('dialog')
    expect(await within(drawer).findByRole('button', { name: /Retry/ })).toBeInTheDocument()
    expect(within(drawer).queryByText(/newer nasiko-server/)).toBeNull()
  })

  it('F4: an empty result shows the empty state', async () => {
    server.use(
      http.get(`${FINOPS}/top-traces`, () =>
        HttpResponse.json({ data: { rows: [], has_more: false }, status_code: 200, message: 'ok' }),
      ),
    )
    renderApp('/tokenops?open=all&traces=true')
    expect(await screen.findByText('No traces in this scope')).toBeInTheDocument()
  })

  it('a failed dashboard shows a panel error with Retry while the month hero still renders', async () => {
    server.use(
      http.get(`${FINOPS}/dashboard`, () => new HttpResponse('internal error', { status: 500 })),
    )
    renderApp('/tokenops?open=all')
    expect(await screen.findByTestId('mtd-spend')).toBeInTheDocument()
    const alerts = await screen.findAllByRole('alert')
    expect(alerts.some((a) => /Couldn't load KPIs/.test(a.textContent ?? ''))).toBe(true)
    expect(screen.getAllByRole('button', { name: /Retry/ }).length).toBeGreaterThan(0)
  })

  it('previous-window failure shows Δ as unavailable, not a made-up number', async () => {
    server.use(
      http.get(`${FINOPS}/dashboard`, ({ request }) => {
        const u = new URL(request.url)
        // The previous-window query is the one with explicit bounds while the page is on 30d.
        if (!u.searchParams.get('range') && u.searchParams.get('start_time'))
          return new HttpResponse('internal error', { status: 500 })
        return undefined
      }),
    )
    renderApp('/tokenops?open=all')
    expect((await screen.findAllByLabelText('comparison unavailable')).length).toBeGreaterThan(0)
  })

  it('the latest window wins when an older response arrives later', async () => {
    let slowStarted!: () => void
    const slowInFlight = new Promise<void>((resolve) => {
      slowStarted = resolve
    })
    server.use(
      http.get(`${FINOPS}/dashboard`, async ({ request }) => {
        if (new URL(request.url).searchParams.get('range') === '7d') {
          slowStarted()
          await delay(300)
        }
        return undefined
      }),
    )
    const { router, queryClient } = renderApp('/tokenops?open=all&preset=7d')
    // Navigate only once the 7d request is really in flight, so its late answer races the 24h one.
    await slowInFlight
    await router.navigate({
      to: '/tokenops',
      search: { open: 'all', preset: '24h', view: 'agent', sort: 'cost' },
    })
    expect(await screen.findByLabelText('Key metrics, Last 24 hours')).toBeInTheDocument()
    // Settled: the 7d request has either landed in its own cache entry or been cancelled.
    await waitFor(() => expect(queryClient.isFetching()).toBe(0), { timeout: 8000 })
    expect(screen.getByLabelText('Key metrics, Last 24 hours')).toBeInTheDocument()
    expect(screen.queryByLabelText('Key metrics, Last 7 days')).toBeNull()
  })

  it('unknown agent filter (400) explains and offers to clear it', async () => {
    const user = userEvent.setup()
    const { router } = renderApp('/tokenops?open=all&agent=does-not-exist')
    expect((await screen.findAllByText(/unknown agent 'does-not-exist'/)).length).toBeGreaterThan(0)
    await user.click(screen.getByRole('button', { name: 'Clear agent filter' }))
    await waitFor(() => expect(router.state.location.search).not.toHaveProperty('agent'))
  })

  it('a deleted agent is unknown to the mock too (400), like the real resolve_agent', async () => {
    renderApp(`/tokenops?open=all&agent=${deletedAgent.name}`)
    expect(await screen.findByRole('button', { name: 'Clear agent filter' })).toBeInTheDocument()
  })

  it('an inaccessible agent (404 "agent not found") gets the same notice', async () => {
    server.use(
      http.get(`${FINOPS}/dashboard`, ({ request }) =>
        new URL(request.url).searchParams.get('agent_id')
          ? new HttpResponse('agent not found', { status: 404 })
          : undefined,
      ),
    )
    renderApp(`/tokenops?open=all&agent=${liveAgent.id}`)
    expect(await screen.findByRole('button', { name: 'Clear agent filter' })).toBeInTheDocument()
  })

  it('/api/me 502 shows the server-down page, not endless skeletons', async () => {
    server.use(http.get('/api/me', () => new HttpResponse('bad gateway', { status: 502 })))
    renderApp('/tokenops?open=all')
    expect(await screen.findByText(/Can't reach nasiko-server/)).toBeInTheDocument()
  })

  it('a dashboard network failure shows the server-down page', async () => {
    server.use(http.get(`${FINOPS}/dashboard`, () => HttpResponse.error()))
    renderApp('/tokenops?open=all')
    expect(await screen.findByText(/Can't reach nasiko-server/)).toBeInTheDocument()
  })

  it('impossible or future custom dates fall back to 30d instead of crashing', async () => {
    renderApp('/tokenops?open=all&preset=custom&from=2026-02-30&to=2026-03-02')
    expect(await screen.findByLabelText('Key metrics, Last 30 days')).toBeInTheDocument()
  })

  it('future custom dates fall back to 30d', async () => {
    renderApp('/tokenops?open=all&preset=custom&from=2026-04-01&to=2026-04-05')
    expect(await screen.findByLabelText('Key metrics, Last 30 days')).toBeInTheDocument()
  })

  it('no session: redirects to login with a relative redirect and no "expired" copy', async () => {
    configureMocks({ loggedIn: false })
    const { router } = renderApp('/tokenops?preset=7d')
    expect(await screen.findByRole('heading', { name: 'Sign in to Nasiko' })).toBeInTheDocument()
    expect((router.state.location.search as { redirect?: string }).redirect).toMatch(
      /^\/tokenops\?preset=7d/,
    )
    expect(screen.queryByText(/session expired/)).toBeNull()
  })

  it('login round-trip lands back on the original URL with its search intact', async () => {
    configureMocks({ loggedIn: false })
    const user = userEvent.setup()
    const { router } = renderApp('/tokenops?open=all&preset=7d')
    await screen.findByRole('heading', { name: 'Sign in to Nasiko' })
    await user.type(screen.getByLabelText('Username'), 'admin')
    await user.type(screen.getByLabelText('Password'), 'x')
    await user.click(screen.getByRole('button', { name: 'Sign in' }))
    await waitFor(() => expect(router.state.location.pathname).toBe('/tokenops'))
    expect(router.state.location.search).toMatchObject({ preset: '7d' })
  })

  it('login shows 401 and 429 errors, and an off-site redirect is ignored', async () => {
    configureMocks({ loggedIn: false })
    const user = userEvent.setup()
    server.use(
      http.post('/api/auth/login', () =>
        HttpResponse.json({ error: 'invalid credentials' }, { status: 401 }),
      ),
    )
    const { router } = renderApp('/login?redirect=//evil.example')
    await user.type(await screen.findByLabelText('Username'), 'admin')
    await user.type(screen.getByLabelText('Password'), 'bad')
    await user.click(screen.getByRole('button', { name: 'Sign in' }))
    expect(await screen.findByText('Wrong username or password.')).toBeInTheDocument()
    server.use(http.post('/api/auth/login', () => HttpResponse.json({}, { status: 429 })))
    await user.click(screen.getByRole('button', { name: 'Sign in' }))
    expect(await screen.findByText(/Too many attempts/)).toBeInTheDocument()
    // ea233d20: a locked account (3 failures) is a 429 with code account_locked and the real wait.
    server.use(
      http.post('/api/auth/login', () =>
        HttpResponse.json(
          {
            error: 'account temporarily locked due to too many failed login attempts',
            code: 'account_locked',
            retry_after_secs: 840,
          },
          { status: 429 },
        ),
      ),
    )
    await user.click(screen.getByRole('button', { name: 'Sign in' }))
    expect(
      await screen.findByText('Too many failed sign-ins. Try again in 14 minutes.'),
    ).toBeInTheDocument()
    server.resetHandlers()
    await user.click(screen.getByRole('button', { name: 'Sign in' }))
    await waitFor(() => expect(router.state.location.pathname).toBe('/'))
  })

  it('a 401 mid-session clears the cache and redirects with expired=true', async () => {
    const { router, queryClient } = renderApp('/tokenops?open=all&preset=7d')
    await screen.findByTestId('mtd-spend')
    configureMocks({ loggedIn: false })
    await router.navigate({
      to: '/tokenops',
      search: { open: 'all', preset: '24h', view: 'agent', sort: 'cost' },
    })
    expect(await screen.findByText(/session expired/)).toBeInTheDocument()
    expect(router.state.location.search).toMatchObject({ expired: true })
    expect(
      queryClient
        .getQueryCache()
        .getAll()
        .filter((q) => q.queryKey[0] === 'tokenops'),
    ).toHaveLength(0)
  })

  it('a 401 on /api/me after sign-in is an expiry too, not a Retry panel', async () => {
    const { router, queryClient } = renderApp('/tokenops?open=all')
    await screen.findByTestId('mtd-spend')
    configureMocks({ loggedIn: false })
    await queryClient.refetchQueries({ queryKey: ['me'] })
    expect(await screen.findByText(/session expired/)).toBeInTheDocument()
    expect(router.state.location.search).toMatchObject({ expired: true })
  })

  it('shows first-run guidance when there is no spend in the last two months', async () => {
    // The admin's harness turns count as spend too (aggregate.ts withHarnessTurns), so empty both seeds.
    configureMocks({
      seed: { ...seed, agents: [], traces: [], sessions: [] },
      harnessSeed: { ...generateHarnessSeed({ anchor: FIXED }), sessions: [] },
    })
    try {
      renderApp('/tokenops?open=all')
      expect(await screen.findByText('No AI spend recorded yet')).toBeInTheDocument()
      expect(screen.getByRole('heading', { level: 1, name: 'TokenOps' })).toBeInTheDocument()
    } finally {
      configureMocks({ harnessSeed: generateHarnessSeed({ anchor: FIXED }) })
    }
  })

  it('an empty window offers "Show last 30 days"', async () => {
    const user = userEvent.setup()
    configureMocks({
      seed: { ...seed, traces: seed.traces.filter((t) => t.ts < now() - 2 * 86_400_000) },
    })
    const { router } = renderApp('/tokenops?open=all&preset=24h')
    const buttons = await screen.findAllByRole('button', { name: 'Show last 30 days' })
    await user.click(buttons[0])
    await waitFor(() => expect(router.state.location.search).toMatchObject({ preset: '30d' }))
  })

  it('Workflows refetches with view=workflow and labels latency as an average', async () => {
    const rec = recordRequests()
    try {
      const { router } = renderApp('/tokenops?open=all')
      await section('Who is driving cost')
      await router.navigate({
        to: '/tokenops',
        search: { open: 'all', preset: '30d', view: 'workflow', sort: 'cost' },
      })
      await waitFor(() =>
        expect(rec.urls.some((u) => u.searchParams.get('view') === 'workflow')).toBe(true),
      )
      expect(await screen.findAllByText(seed.workflows[0].workflow_name)).not.toHaveLength(0)
      expect(
        within(await section('Who is driving cost')).getByRole('columnheader', {
          name: /Avg latency/,
        }),
      ).toBeInTheDocument()
    } finally {
      rec.stop()
    }
  })
})
