/**
 * The Overview page (plans/feat-overview.md §14): each card renders from the pinned seed and fails on its own, the
 * spend is said as fleet spend (eng R2), and the Fleet health counts match the Agents catalog filter (eng R1).
 */
import { screen, waitFor, within } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { http, HttpResponse } from 'msw'
import { afterEach, describe, expect, it } from 'vitest'
import { clearChatRegistry } from '@/features/chat/registry'
import { configureMocks } from '@/mocks/handlers'
import { configureChatMock } from '@/mocks/chatStore'
import { now, seed, setupPinnedSeed } from '@/test/pinnedSeed'
import { renderApp } from '@/test/renderApp'
import { recordRequests, server } from '@/test/setup'
import { copy as deployCopy } from '@/features/deploy/copy'
import { copy } from './copy'

setupPinnedSeed()
afterEach(() => {
  configureMocks({ seed, now, loggedIn: true, superuser: null, variant: null })
  configureChatMock({ waiting: false })
  clearChatRegistry()
})

const card = (id: string) => document.querySelector(`[data-testid="${id}"]`) as HTMLElement
const loaded = async () => {
  await screen.findByRole('heading', { level: 1, name: copy.title })
  await waitFor(() =>
    expect(
      within(card('overview-health')).getAllByRole('link', {
        name: /^\d+ (healthy|watch|needs? action|unknown)/,
      }),
    ).toHaveLength(4),
  )
  await waitFor(() =>
    expect(within(card('overview-month')).getByTestId('overview-mtd')).toBeInTheDocument(),
  )
}

describe('Spend', () => {
  it('says fleet spend with the scope note, and never "You\'ve spent" (eng R2)', async () => {
    renderApp('/')
    await loaded()
    expect(card('overview-month')).toHaveTextContent(copy.month.label)
    expect(card('overview-spend')).toHaveTextContent(copy.spend.scopeNote)
    expect(document.body).not.toHaveTextContent(/You've spent/i)
  })

  it('stacks 30 days by the top drivers, whose own series come from the filtered timeseries', async () => {
    const reqs = recordRequests()
    renderApp('/')
    await loaded()
    const spend = card('overview-spend')
    const fig = await within(spend).findByRole('figure', {}, { timeout: 5000 })
    await userEvent.click(within(fig).getByRole('button', { name: copy.spend.showTable }))
    const heads = within(fig)
      .getAllByRole('columnheader')
      .map((h) => h.textContent)
    const filtered = reqs.urls.filter(
      (u) => u.pathname.endsWith('/spend-timeseries') && u.searchParams.has('agent_id'),
    )
    // One filtered series per driver with spend, at most five (one per chart colour).
    const k = new Set(filtered.map((u) => u.searchParams.get('agent_id'))).size
    expect(k).toBeGreaterThan(0)
    expect(k).toBeLessThanOrEqual(5)
    // Day, the drivers (named as in the drivers table), Other, Total.
    expect(heads).toHaveLength(k + 3)
    const drivers = within(spend).getAllByTestId('spend-driver')
    heads.slice(1, k + 1).forEach((h, i) => expect(drivers[i]).toHaveTextContent(h!))
    reqs.stop()
  })

  it('keeps the other cards when the calendar fails', async () => {
    server.use(
      http.get(
        '*/api/observability/finops/spend-calendar',
        () => new HttpResponse('boom', { status: 500 }),
      ),
    )
    renderApp('/')
    await within(await screen.findByTestId('overview-month')).findByText(
      copy.couldntCheck(copy.spend.what),
    )
    await waitFor(() =>
      expect(within(card('overview-health')).getAllByRole('link').length).toBeGreaterThan(1),
    )
  })
})

describe('Fleet health', () => {
  it('links each count to the catalog filtered by that rating, with the same number of agents (eng R1)', async () => {
    renderApp('/')
    await loaded()
    const health = card('overview-health')
    for (const r of ['watch', 'healthy'] as const) {
      const link = within(health)
        .getAllByRole('link')
        .find((l) => l.getAttribute('href')?.includes(`health=${r}`))!
      const n = Number(link.textContent!.match(/^\d+/)![0])
      expect(link).toHaveAttribute('href', `/agents?health=${r}`)
      if (r === 'watch') {
        // The seed has agents to watch, so the parity check isn't trivially 0 = 0.
        expect(n).toBeGreaterThan(0)
        await userEvent.click(link)
        await screen.findByRole('button', {
          name: `Remove the Health: ${copy.rating.watch} filter`,
        })
        await waitFor(() =>
          expect(within(screen.getByRole('main')).queryAllByRole('listitem')).toHaveLength(n),
        )
      }
    }
  })

  it('shows Watch agents with a reason and a plain icon, never a tinted circle (design 9A)', async () => {
    renderApp('/')
    await loaded()
    const health = card('overview-health')
    const rows = (await within(health).findAllByRole('listitem')).filter((r) =>
      r.textContent!.includes(copy.rating.watch),
    )
    expect(rows.length).toBeGreaterThan(0)
    for (const row of rows) expect(row.textContent).toMatch(/ · /)
    // The rows' severity mark is a plain icon; the tinted status badges are only the counts above them.
    for (const row of rows)
      expect(row.querySelector('[class*="rounded-full"][class*="bg-warning/"]')).toBeNull()
  })
})

describe('Needs you', () => {
  const needs = () => card('overview-needs')
  const settled = async () => {
    await loaded()
    await waitFor(() => expect(within(needs()).queryByLabelText(copy.loading)).toBeNull(), {
      timeout: 5000,
    })
  }

  it('a superuser sees only requests provably theirs, with a link to each chat (eng R5)', async () => {
    configureChatMock({ waiting: true })
    renderApp('/')
    await settled()
    const rows = within(needs())
      .getAllByRole('listitem')
      .filter((li) => li.querySelector('[data-kind="request"]'))
    expect(rows.length).toBeGreaterThan(0)
    for (const r of rows)
      expect(
        within(r)
          .getByRole('link', { name: /Review the request in/ })
          .getAttribute('href'),
      ).toMatch(/^\/chat\//)
    expect(needs().querySelector('[data-kind="outside"]')).toBeNull()
    // A row is one chat; a chat can hold several requests ("(+1 more)").
    const waiting = rows.reduce(
      (n, r) => n + 1 + Number(r.textContent!.match(/\(\+(\d+) more\)/)?.[1] ?? 0),
      0,
    )
    expect(screen.getByTestId('headline-attention')).toHaveTextContent(
      new RegExp(`${waiting} requests? (is|are) waiting for you`),
    )
  })

  it('a normal user gets one "outside Chat" row for requests no chat claims (eng R5)', async () => {
    configureMocks({ superuser: false })
    configureChatMock({ waiting: true })
    renderApp('/')
    await settled()
    expect(needs().querySelector('[data-kind="outside"]')).toHaveTextContent(copy.needs.outside(2))
  })

  it('a failed source adds a line and never says "Nothing needs you" (design 5A)', async () => {
    server.use(http.get('/api/hitl/pending', () => new HttpResponse('boom', { status: 500 })))
    renderApp('/')
    await loaded()
    expect(
      await within(needs()).findByText(copy.couldntCheck(copy.needs.source.requests)),
    ).toBeInTheDocument()
    expect(within(needs()).queryByTestId('needs-empty')).toBeNull()
    expect(screen.getByTestId('overview-headline')).not.toHaveTextContent(copy.needs.nothing)
  })

  it('always says when it last checked (design 8A)', async () => {
    renderApp('/')
    await settled()
    expect(within(needs()).getByText(/^Checked /)).toBeInTheDocument()
  })
})

describe('first run (design 7A)', () => {
  it('leads with the Setup guide card instead of the deploy card, and it reopens the guide', async () => {
    server.use(http.get('*/api/agents', () => HttpResponse.json([])))
    const user = userEvent.setup()
    renderApp('/')
    const guideCard = await screen.findByTestId('overview-setup-guide')
    expect(document.querySelector('[data-testid="overview-first-run"]')).toBeNull()
    // One next action: Deploy beside the headline; the header's Setup guide and Quick actions' Deploy step aside.
    expect(screen.getAllByRole('link', { name: deployCopy.entry.label })).toHaveLength(1)
    expect(screen.queryByRole('button', { name: copy.setup.button })).toBeNull()
    expect(within(card('overview-actions')).queryByRole('button', { name: /Copy/ })).toBeNull()
    // The seed user picked a role and has router configs, but no agents yet: the guide resumes at Deploy an agent.
    await user.click(within(guideCard).getByRole('button', { name: 'Resume guide' }))
    expect(
      await screen.findByRole('heading', { name: 'Deploy your first agent' }),
    ).toBeInTheDocument()
  })

  it('opens the CLI path with every command, and each guide row at its own step', async () => {
    server.use(http.get('*/api/agents', () => HttpResponse.json([])))
    const user = userEvent.setup()
    renderApp('/')
    const guideCard = await screen.findByTestId('overview-setup-guide')
    // The deploy command alone fails before `nasiko connect` and `nasiko new`, so the CLI shows all three.
    await user.click(screen.getByRole('button', { name: copy.firstRun.cli(3) }))
    expect(await screen.findByText(/^nasiko connect /)).toBeInTheDocument()
    expect(screen.getByText('nasiko deploy ./my-agent')).toBeInTheDocument()
    await user.click(within(guideCard).getByRole('button', { name: /^Connect a model/ }))
    expect(
      await screen.findByRole('heading', { name: 'Connect a model provider' }),
    ).toBeInTheDocument()
  })

  it('says Start guide until a step is done', async () => {
    server.use(http.get('*/api/agents', () => HttpResponse.json([])))
    configureMocks({ onboarding: { persona: null, completed: true } })
    server.use(http.get('*/api/llm-configs', () => HttpResponse.json({ data: [] })))
    renderApp('/')
    const guideCard = await screen.findByTestId('overview-setup-guide')
    expect(
      await within(guideCard).findByRole('button', { name: 'Start guide' }),
    ).toBeInTheDocument()
  })

  it('previews the lead cards without numbers and keeps Harnesses to one line', async () => {
    server.use(http.get('*/api/agents', () => HttpResponse.json([])))
    renderApp('/')
    const preview = await screen.findByTestId('overview-preview')
    expect(
      within(preview)
        .getAllByRole('heading', { level: 3 })
        .map((h) => h.textContent),
    ).toEqual([copy.needs.title, copy.spend.title, copy.health.title])
    // Honest numbers: nothing has run, so the preview states none.
    expect(preview.textContent).not.toMatch(/\d/)
    const line = card('overview-harnesses')
    await within(line).findByText(/coding harness/)
    expect(within(line).getByRole('link')).toHaveAttribute('href', '/harnesses')
    expect(within(line).queryByText(copy.kpi.connected)).toBeNull()
  })

  it('opens the guide from the header Setup guide', async () => {
    const user = userEvent.setup()
    renderApp('/')
    const button = await screen.findByRole('button', { name: 'Setup guide' })
    await waitFor(() => expect(button).toBeEnabled())
    await user.click(button)
    // A finished user with configs and agents reopens at Ready.
    expect(
      await screen.findByRole('dialog', { name: 'Your workspace is ready' }),
    ).toBeInTheDocument()
  })

  it('shows the deploy card and fetches no finops or sessions with an empty fleet on an older server', async () => {
    configureMocks({ variant: 'onboarding-absent' })
    server.use(http.get('*/api/agents', () => HttpResponse.json([])))
    const rec = recordRequests()
    renderApp('/')
    expect(
      await screen.findByRole('heading', { level: 2, name: copy.firstRun.title }),
    ).toBeInTheDocument()
    expect(screen.getByTestId('overview-headline')).toHaveTextContent(copy.firstRun.headline)
    expect(
      within(card('overview-first-run')).getAllByRole('button', { name: /Copy/ }).length,
    ).toBeGreaterThan(0)
    expect(document.querySelector('[data-testid="overview-needs"]')).toBeNull()
    // Let the first-run render's own requests start before judging them.
    await within(card('overview-harnesses')).findByText(
      /connected|No coding harnesses|Your own usage/,
      {},
      { timeout: 3000 },
    )
    rec.stop()
    const paths = rec.urls.map((u) => u.pathname)
    expect(paths.filter((p) => p.includes('/finops/'))).toEqual([])
    expect(paths.filter((p) => p.includes('/session/'))).toEqual([])
  })
})

describe('Recent sessions and the session checks (eng R7)', () => {
  it('status-checks at most the newest 25 sessions', async () => {
    const rec = recordRequests()
    renderApp('/')
    await loaded()
    await within(card('overview-sessions')).findAllByTestId('recent-session')
    await waitFor(
      () => expect(within(card('overview-needs')).queryByLabelText(copy.loading)).toBeNull(),
      { timeout: 5000 },
    )
    rec.stop()
    const details = new Set(
      rec.urls.map((u) => u.pathname).filter((p) => /\/session\/(?!list)[^/]+$/.test(p)),
    )
    expect(details.size).toBeGreaterThan(0)
    expect(details.size).toBeLessThanOrEqual(25)
  })

  it('says the trace store is needed when the session list fails with a 503', async () => {
    server.use(
      http.get(
        '*/api/observability/session/list',
        () => new HttpResponse('Tempo is not configured', { status: 503 }),
      ),
    )
    renderApp('/')
    expect(
      await within(await screen.findByTestId('overview-sessions')).findByText(
        copy.sessions.traceStore,
      ),
    ).toBeInTheDocument()
  })
})

describe('Coding harnesses', () => {
  it('falls back to your own usage when the org endpoint is absent', async () => {
    configureMocks({ variant: 'usage-404' })
    renderApp('/')
    expect(
      await within(await screen.findByTestId('overview-harnesses')).findByText(
        copy.harnesses.ownOnly,
      ),
    ).toBeInTheDocument()
  })

  it('labels cost as an estimate at API list price', async () => {
    renderApp('/')
    expect(
      await within(await screen.findByTestId('overview-harnesses')).findByText(
        copy.harnesses.costNote(30),
      ),
    ).toBeInTheDocument()
  })
})

describe('Budgets', () => {
  // Budgets hidden (no server support for /api/budgets yet, R-L10): un-skip with the commented budget code.
  it.skip('shows the card and Adjust budget when the server has budgets', async () => {
    renderApp('/')
    await loaded()
    expect(
      await within(card('overview-budget')).findByText(copy.budget.forecast, {}, { timeout: 5000 }),
    ).toBeInTheDocument()
    expect(
      within(card('overview-actions')).getByRole('link', { name: copy.actions.adjustBudget }),
    ).toHaveAttribute('href', '/router#router-budgets')
  })

  it('hides both on a server without budgets (a bare 404), and Recent sessions takes the row', async () => {
    server.use(http.get('*/api/budgets', () => new HttpResponse(null, { status: 404 })))
    renderApp('/')
    await loaded()
    await waitFor(() => expect(card('overview-budget')).toBeNull())
    const actions = card('overview-actions')
    expect(within(actions).queryByRole('link', { name: copy.actions.adjustBudget })).toBeNull()
    expect(card('overview-sessions').className).toContain('@[1100px]/overview:col-span-4')
  })
})

describe('structure (design 14A)', () => {
  it('has one h1 and an h2 per card, each card a labelled section', async () => {
    renderApp('/')
    await loaded()
    expect(screen.getAllByRole('heading', { level: 1 })).toHaveLength(1)
    const ids = [
      // Savings sits between the summary and the detail grid: a headline feature, above the fold.
      'overview-savings',
      'overview-needs',
      'overview-spend',
      // 'overview-budget', // Budgets hidden: no server support yet (R-L10).
      'overview-health',
      'overview-sessions',
      'overview-chats',
      'overview-actions',
    ]
    for (const id of ids) {
      const c = card(id)
      expect(c.tagName).toBe('SECTION')
      expect(within(c).getAllByRole('heading', { level: 2 })).toHaveLength(1)
    }
    // Priority order in the DOM (design 1A).
    const order = [...document.querySelectorAll('section[data-testid^="overview-"]')].map((e) =>
      e.getAttribute('data-testid'),
    )
    expect(order).toEqual(ids)
  })
})

describe('Loading', () => {
  it('shows one page loader on a cold load, then the cards; a range change keeps the cards', async () => {
    renderApp('/')
    // The header is static, so it stays above the loader.
    await screen.findByRole('heading', { level: 1, name: copy.title })
    expect(screen.getByTestId('page-loader')).toBeInTheDocument()
    expect(card('overview-needs')).toBeNull()
    await loaded()
    expect(screen.queryByTestId('page-loader')).toBeNull()
    // A new range refetches the range's reads: the cards show their own loading, never the page loader again.
    await userEvent.click(screen.getByRole('radio', { name: copy.range.item(90) }))
    expect(screen.queryByTestId('page-loader')).toBeNull()
    expect(card('overview-spend')).toBeInTheDocument()
  })
})

describe('Savings', () => {
  it('shows what optimisation saved plus one next step, then hands off to TokenOps', async () => {
    // Not a small TokenOps: one number and one thing to do about it. Repeating the breakdown here
    // would give the reader the same work twice and a reason to skip both.
    renderApp('/')
    const row = await screen.findByTestId('overview-savings')
    expect(row).toHaveAttribute('data-state', 'saving')
    expect(row).toHaveTextContent(/Token optimisation/)
    expect(row).toHaveTextContent(/fewer tokens/)
    // The insight names the agent worth switching on, not the layer that happened to win.
    expect(row).toHaveTextContent(/biggest win left|Mostly from/)
    // Deep-links to the section, not the top of a page with four other panels above it.
    expect(within(row).getByRole('link', { name: /See the breakdown/ })).toHaveAttribute(
      'href',
      '/tokenops?open=optimise',
    )
  })

  it('the breakdown link lands on the optimisation section', async () => {
    const user = userEvent.setup()
    const { router } = renderApp('/')
    const row = await screen.findByTestId('overview-savings')
    await user.click(within(row).getByRole('link', { name: /See the breakdown/ }))
    await waitFor(() => expect(router.state.location.pathname).toBe('/tokenops'))
    const section = await screen.findByRole('button', { name: /^Token optimisation/ })
    expect(section).toHaveAttribute('aria-expanded', 'true')
  })

  it('keeps the tile grid to the four like-for-like counters', async () => {
    // A fifth tile wrapping onto its own row was what made the grid look broken.
    renderApp('/')
    await screen.findByTestId('kpi-spend')
    expect(screen.queryByTestId('kpi-savings')).toBeNull()
  })

  it('turns into an opportunity when nothing is switched on, rather than reading $0 saved', async () => {
    // Money on the table is a reason to act; "$0 saved" reads as a broken feature.
    server.use(
      http.get('*/finops/savings', () =>
        HttpResponse.json({
          data: {
            window: { start: '2026-03-01T00:00:00Z', end: '2026-03-31T00:00:00Z' },
            total: {
              saved_tokens: 0,
              saved_input_tokens: 0,
              saved_output_tokens: 0,
              saved_cost_usd: 0,
              actual_tokens: 0,
              actual_cost_usd: 0,
              baseline_tokens: 0,
              baseline_cost_usd: 0,
              token_reduction_pct: null,
              cost_reduction_pct: null,
              basis: 'measured',
            },
            by_program: [],
            by_agent: [],
            by_session: [],
            coverage: {
              calls_in_window: 0,
              calls_with_any_layer_enabled: 0,
              agents_total: 3,
              agents_optimized: 0,
              agents_with_compress_enabled: 0,
              agents_with_minimal_code_enabled: 0,
              agents_with_prompt_comments: 0,
              optimized_spend_usd: 0,
              unoptimized_spend_usd: 412.5,
              top_unoptimized: {
                agent_id: 'a9',
                agent_name: 'Sales Assistant',
                spend_usd: 412.5,
              },
              calibrated_pct: null,
            },
          },
          status_code: 200,
          message: 'ok',
        }),
      ),
    )
    renderApp('/')
    const band = await screen.findByTestId('overview-savings')
    expect(band).toHaveAttribute('data-state', 'idle')
    expect(band).toHaveTextContent(/could be trimmed/)
    expect(band).not.toHaveTextContent(/\$0\.00 saved/)
    expect(within(band).getByRole('link', { name: /Choose an agent/ })).toHaveAttribute(
      'href',
      '/agents',
    )
  })

  it('says nothing at all on a workspace with no spend to talk about', async () => {
    server.use(
      http.get('*/finops/savings', () =>
        HttpResponse.json({
          data: {
            window: { start: '2026-03-01T00:00:00Z', end: '2026-03-31T00:00:00Z' },
            total: {
              saved_tokens: 0,
              saved_input_tokens: 0,
              saved_output_tokens: 0,
              saved_cost_usd: 0,
              actual_tokens: 0,
              actual_cost_usd: 0,
              baseline_tokens: 0,
              baseline_cost_usd: 0,
              token_reduction_pct: null,
              cost_reduction_pct: null,
              basis: 'measured',
            },
            by_program: [],
            by_agent: [],
            by_session: [],
            coverage: {
              calls_in_window: 0,
              calls_with_any_layer_enabled: 0,
              agents_total: 0,
              agents_optimized: 0,
              agents_with_compress_enabled: 0,
              agents_with_minimal_code_enabled: 0,
              agents_with_prompt_comments: 0,
              optimized_spend_usd: 0,
              unoptimized_spend_usd: 0,
              calibrated_pct: null,
            },
          },
          status_code: 200,
          message: 'ok',
        }),
      ),
    )
    renderApp('/')
    await screen.findByTestId('kpi-spend')
    expect(screen.queryByTestId('overview-savings')).toBeNull()
  })
})
