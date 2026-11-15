/**
 * The Overview page (plans/feat-overview.md §14): each card renders from the pinned seed and fails on its own, the
 * spend is said as fleet spend (eng R2), and the Fleet health counts match the Agents catalog filter (eng R1).
 */
import { act, screen, waitFor, within } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { http, HttpResponse } from 'msw'
import { afterEach, describe, expect, it, vi } from 'vitest'
import { clearChatRegistry } from '@nasiko/features/chat/registry'
import { apiFetch } from '@nasiko/data-access/client'
import { configureMocks } from '@nasiko/mocks/handlers'
import { configureChatMock } from '@nasiko/mocks/chatStore'
import { ADMIN_ID } from '@nasiko/mocks/seed-harness'
import { now, seed, setupPinnedSeed } from '@nasiko/testing/pinnedSeed'
import { renderApp } from '@nasiko/testing/renderApp'
import { recordRequests, server } from '@nasiko/testing/setup'
import { copy as deployCopy } from '@nasiko/features/deploy/copy'
import { clearSeededDeploys, setSeededDeploy } from '@nasiko/features/onboarding/api'
import { copy } from './copy'
import { PREVIEW_MS } from './preview'
import { writeDone } from './tryRun'
import { DEFAULT_RANGE, RANGE_DAYS } from './search'
import {
  FAILED_SESSION_ROWS,
  HEALTH_SQUARES_MAX,
  PREVIEW_HOLD_MS,
  PREVIEW_STEP_MS,
  RECENT_SESSION_ROWS,
  SAVED_BARS,
} from './tuning'

setupPinnedSeed()
afterEach(() => {
  configureMocks({ seed, now, loggedIn: true, superuser: null, variant: null, routerVariants: [] })
  window.history.replaceState(null, '', '/')
  configureChatMock({ waiting: false })
  clearChatRegistry()
  clearSeededDeploys()
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

  it('stacks 30 days by the top drivers, whose series come from one grouped timeseries', async () => {
    const reqs = recordRequests()
    renderApp('/')
    await loaded()
    const spend = card('overview-spend')
    const fig = await within(spend).findByRole('figure', {}, { timeout: 5000 })
    await userEvent.click(within(fig).getByRole('button', { name: copy.spend.showTable }))
    const heads = within(fig)
      .getAllByRole('columnheader')
      .map((h) => h.textContent)
    // The drivers' series used to be one filtered request each — a fan-out that
    // grew with the chart. `group_by=agent` returns all of them at once.
    expect(
      reqs.urls.filter(
        (u) => u.pathname.endsWith('/spend-timeseries') && u.searchParams.has('agent_id'),
      ),
    ).toEqual([])
    const grouped = reqs.urls.filter(
      (u) => u.pathname.endsWith('/spend-timeseries') && u.searchParams.get('group_by') === 'agent',
    )
    expect(grouped).toHaveLength(1)
    expect(grouped[0].searchParams.get('top')).toBe('5')
    // Columns are Day, one per driver that actually spent, Other, Total — so the
    // chart can carry fewer series than the drivers table has rows.
    const k = heads.length - 3
    expect(k).toBeGreaterThan(0)
    expect(k).toBeLessThanOrEqual(5)
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
          expect(
            within(screen.getByRole('main'))
              .queryAllByRole('listitem')
              // The top bar's breadcrumb is a list too.
              .filter((li) => !li.closest('[data-slot="breadcrumb"]')),
          ).toHaveLength(n),
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

describe('Needs your attention', () => {
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

  it('a request row links its chat’s paused flow, without a second row (plans/feat-flows.md O2)', async () => {
    configureChatMock({ waiting: true })
    // The paused flows, one per chat with a pending request (as the server would have for those pauses).
    server.use(
      http.get('/api/flows', async ({ request }) => {
        if (new URL(request.url).searchParams.get('status') !== 'paused') return undefined
        const pending = await apiFetch<{
          data?: { execution: { chat_session_id: string | null } }[]
        }>('/api/hitl/pending')
        const chats = [
          ...new Set((pending.data ?? []).flatMap((r) => r.execution.chat_session_id ?? [])),
        ]
        return HttpResponse.json({
          data: chats.map((c, i) => ({
            flow_id: `5eedf00000000000000000000000${String(i).padStart(4, '0')}`,
            status: 'paused',
            root_agent_name: 'orchestrator',
            metadata: { context_id: c },
            created_at: '2026-03-20T14:00:00Z',
          })),
          total: chats.length,
        })
      }),
    )
    renderApp('/')
    await settled()
    const rows = within(needs())
      .getAllByRole('listitem')
      .filter((li) => li.querySelector('[data-kind="request"]'))
    expect(rows.length).toBeGreaterThan(0)
    await waitFor(() =>
      expect(within(rows[0]!).getByRole('link', { name: 'Open flow' })).toHaveAttribute(
        'href',
        expect.stringMatching(/^\/flows\/5eedf/),
      ),
    )
    // Still one row per chat: no paused-flow rows of their own.
    expect(needs().querySelectorAll('[data-kind="flow"]')).toHaveLength(0)
  })

  it('a failed paused-flows read only means no flow links: the inbox still shows (O2a)', async () => {
    configureChatMock({ waiting: true })
    server.use(
      http.get('/api/flows', ({ request }) =>
        new URL(request.url).searchParams.get('status') === 'paused'
          ? new HttpResponse('internal error', { status: 500 })
          : undefined,
      ),
    )
    renderApp('/')
    await settled()
    const rows = within(needs())
      .getAllByRole('listitem')
      .filter((li) => li.querySelector('[data-kind="request"]'))
    expect(rows.length).toBeGreaterThan(0)
    expect(within(needs()).queryByRole('link', { name: 'Open flow' })).toBeNull()
    // The word, not "Workflows" (the setup item).
    expect(within(needs()).queryByText(/\bflows\b/i)).toBeNull()
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
  it('puts the Setup guide in the hero beside Deploy, and it reopens the guide', async () => {
    server.use(http.get('*/api/agents', () => HttpResponse.json([])))
    const user = userEvent.setup()
    renderApp('/')
    const guideCard = await screen.findByTestId('overview-setup-guide')
    // The guide fills the hero's right half (redesign 2026-10-21), not a card of its own; no range with nothing to range.
    expect(card('overview-first-run')).toContainElement(guideCard)
    expect(screen.queryByRole('group', { name: copy.range.label })).toBeNull()
    // One next action: Deploy beside the headline; the header's Setup guide steps aside.
    expect(screen.getAllByRole('link', { name: deployCopy.entry.label })).toHaveLength(1)
    expect(screen.queryByRole('button', { name: copy.setup.button })).toBeNull()
    expect(within(card('overview-actions')).queryByRole('button', { name: /Copy/ })).toBeNull()
    // The seed user picked a role and has router configs, but no agents yet: the guide resumes at Agent and key.
    await user.click(within(guideCard).getByRole('button', { name: 'Resume guide' }))
    expect(
      await screen.findByRole('dialog', { name: 'Bring an agent, then add your key' }),
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
    await user.click(within(guideCard).getByRole('button', { name: /^Add your model key/ }))
    expect(await screen.findByRole('heading', { name: '2. Your model key' })).toBeInTheDocument()
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
    // NAS-1448: the first step not done says it is next; the rest say nothing, never "not yet".
    expect(
      within(guideCard).getByRole('button', { name: /^Add your model key.*Up next$/ }),
    ).toBeInTheDocument()
    expect(
      within(guideCard).getByRole('button', { name: /^Deploy an agent/ }),
    ).not.toHaveAccessibleName(/Up next/)
  })

  it("says the guide's agent is deploying while the fleet is still empty, then that it didn't start (NAS-1450)", async () => {
    server.use(http.get('*/api/agents', () => HttpResponse.json([])))
    configureMocks({ onboarding: { persona: null, completed: true } })
    const title = 'Meeting Notes Processor'
    setSeededDeploy('meeting-notes-processor', { phase: 'deploying', title, startedAt: Date.now() })
    renderApp('/')
    const guideCard = await screen.findByTestId('overview-setup-guide')
    expect(
      await within(guideCard).findByRole('button', { name: /^Deploy an agent.*Deploying…$/ }),
    ).toBeInTheDocument()
    act(() =>
      setSeededDeploy('meeting-notes-processor', {
        phase: 'failed',
        title,
        reason: 'internal error',
      }),
    )
    expect(
      await within(guideCard).findByRole('button', { name: /^Deploy an agent.*Didn't start$/ }),
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
    // A finished user with configs and agents reopens at the last step, Coding harnesses.
    expect(
      await screen.findByRole('dialog', { name: 'Bring in your coding harnesses' }),
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
    // Setup steps belong to Needs your attention, which the first run doesn't have (2026-11-05).
    expect(paths.filter((p) => p.includes('/maf/workflows'))).toEqual([])
  })
})

describe('Recent sessions and the session checks (eng R7)', () => {
  it('reads session status from the list, with no per-session or per-trace fan-out', async () => {
    const rec = recordRequests()
    renderApp('/')
    await loaded()
    await within(card('overview-sessions')).findAllByTestId('recent-session')
    await waitFor(
      () => expect(within(card('overview-needs')).queryByLabelText(copy.loading)).toBeNull(),
      { timeout: 5000 },
    )
    rec.stop()
    const paths = rec.urls.map((u) => u.pathname)
    // `session/list` carries `status`, so the Overview no longer downloads a
    // session detail (and 3 traces each) to compute one boolean per row. This
    // was up to 100 requests, every one a trace-store read on the server.
    expect(paths.filter((p) => /\/session\/(?!list)[^/]+$/.test(p))).toEqual([])
    expect(paths.filter((p) => p.includes('/trace/'))).toEqual([])
    // Two list reads: the rows shown, and the failed ones Needs you counts.
    const lists = rec.urls.filter((u) => u.pathname.endsWith('/session/list'))
    expect(lists).toHaveLength(2)
    expect(lists.map((u) => Number(u.searchParams.get('limit'))).sort((x, y) => x - y)).toEqual(
      [RECENT_SESSION_ROWS, FAILED_SESSION_ROWS].sort((x, y) => x - y),
    )
    expect(lists.some((u) => u.searchParams.get('status') === 'failed')).toBe(true)
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

  it('labels harness cost as value at API prices, not a charge', async () => {
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
  it.skip('shows the card when the server has budgets', async () => {
    renderApp('/')
    await loaded()
    expect(
      await within(card('overview-budget')).findByText(copy.budget.forecast, {}, { timeout: 5000 }),
    ).toBeInTheDocument()
  })

  it('hides it on a server without budgets (a bare 404); Recent sessions sits beside Saved tokens', async () => {
    server.use(http.get('*/api/budgets', () => new HttpResponse(null, { status: 404 })))
    renderApp('/')
    await loaded()
    await waitFor(() => expect(card('overview-budget')).toBeNull())
    expect(card('overview-sessions').className).toContain('@[1100px]/overview:flex-7')
    expect(card('overview-sessions').parentElement).toBe(card('overview-saved').parentElement)
  })
})

describe('structure (design 14A)', () => {
  it('has one h1 and an h2 per card, each card a labelled section', async () => {
    renderApp('/')
    await loaded()
    expect(screen.getAllByRole('heading', { level: 1 })).toHaveLength(1)
    const ids = [
      // Savings is the summary card's last block; Fleet health sits beside Needs you (redesign 2026-10-19).
      'overview-savings',
      'overview-needs',
      'overview-health',
      'overview-harness-activity',
      'overview-spend',
      'overview-sessions',
      'overview-saved',
      'overview-chats',
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
  it('shows what optimization saved plus one next step, then hands off to TokenOps', async () => {
    // Not a small TokenOps: one number and one thing to do about it. Repeating the breakdown here
    // would give the reader the same work twice and a reason to skip both.
    renderApp('/')
    const row = await screen.findByTestId('overview-savings')
    expect(row).toHaveAttribute('data-state', 'saving')
    expect(row).toHaveTextContent(/Token optimization/)
    expect(row).toHaveTextContent(/fewer tokens/)
    // The insight names the agent worth switching on, not the layer that happened to win.
    expect(row).toHaveTextContent(/biggest win left|Mostly from/)
    // Deep-links to the section, not the top of a page with four other panels above it.
    expect(within(row).getByRole('link', { name: /See the breakdown/ })).toHaveAttribute(
      'href',
      '/tokenops?open=optimize',
    )
    // The band fills the lead card's foot: no tip beside it.
    expect(screen.queryByTestId('overview-tip')).toBeNull()
  })

  it('the breakdown link lands on the optimization section', async () => {
    const user = userEvent.setup()
    const { router } = renderApp('/')
    const row = await screen.findByTestId('overview-savings')
    await user.click(within(row).getByRole('link', { name: /See the breakdown/ }))
    await waitFor(() => expect(router.state.location.pathname).toBe('/tokenops'))
    // TokenOps has no expanders: `open=optimise` scrolls to the section, which is always on the page.
    expect(router.state.location.search).toMatchObject({ open: 'optimize' })
    expect(
      await screen.findByRole('heading', { name: 'Token optimization savings' }),
    ).toBeInTheDocument()
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
    // The band's slot takes a tip instead (user 2026-11-06), even beside tiles with figures.
    expect(document.querySelector('[data-tour="overview-summary"]')).toContainElement(
      screen.getByTestId('overview-tip'),
    )
    // Nothing saved by any agent: Saved tokens steps out (user 2026-11-06) and Recent sessions takes its row.
    await waitFor(() => expect(card('overview-saved')).toBeNull())
    expect(card('overview-sessions')).toBeInTheDocument()
  })
})

describe('setup items (2026-11-05)', () => {
  it('turns what isn’t set up into steps and keeps what is, with its count and where to manage it', async () => {
    configureMocks({ variant: 'mcp-empty', routerVariants: ['router-empty'] })
    renderApp('/')
    await loaded()
    const steps = await within(card('overview-needs')).findByTestId('needs-setup')
    expect(
      within(steps).getByRole('link', { name: copy.needs.setup.router.action }),
    ).toHaveAttribute('href', '/router')
    expect(within(steps).getByRole('link', { name: copy.needs.setup.mcp.action })).toHaveAttribute(
      'href',
      '/mcp',
    )
    // The seed has workflows: the item stays, saying so, and opens them.
    expect(steps).toHaveTextContent(copy.needs.setup.workflows.doneTitle)
    expect(steps).toHaveTextContent(/\d+\+? workflows? deployed/)
    expect(
      within(steps).getByRole('link', { name: copy.needs.setup.workflows.manage }),
    ).toHaveAttribute('href', '/workflows')
    expect(card('overview-needs')).toHaveTextContent(copy.needs.setupCount(2))
  })

  it('keeps all three when everything is set up, with nothing counted as left', async () => {
    renderApp('/')
    await loaded()
    const steps = await within(card('overview-needs')).findByTestId('needs-setup')
    expect(steps.querySelectorAll('[data-kind="setup"][data-done="true"]')).toHaveLength(3)
    expect(card('overview-needs')).not.toHaveTextContent(/to set up/)
  })
})

describe('lower cards with nothing to show (user 2026-11-06)', () => {
  it('step out, and a row whose cards all stepped out leaves no gap', async () => {
    configureMocks({ variant: 'empty' })
    renderApp('/')
    await loaded()
    await waitFor(() => expect(card('overview-sessions')).toBeNull())
    expect(card('overview-needs')).toBeInTheDocument()
    // No Quick actions: the sidebar, Agents and "Did you know?" cover them.
    expect(card('overview-actions')).toBeNull()
  })
})

describe('coding harness activity (2026-11-06)', () => {
  it('shows models, subagent runs and tools in a section of its own', async () => {
    renderApp('/')
    await loaded()
    const section = await within(card('overview-harness-activity')).findByRole('list', {
      name: copy.activity.models,
    })
    expect(within(section).getAllByRole('listitem').length).toBeGreaterThan(0)
    const activity = card('overview-harness-activity')
    expect(activity).toHaveTextContent(copy.activity.subagents)
    expect(within(activity).getByRole('list', { name: copy.activity.tools })).toBeInTheDocument()
    expect(activity).toHaveTextContent(/[\d,]+ tool calls/)
  })

  it('is left out when the harnesses sat idle, since the Harnesses tile already says so', async () => {
    window.history.replaceState(null, '', '/?mock=no-activity')
    renderApp('/')
    await loaded()
    const tile = await screen.findByTestId('quiet-harnesses', {}, { timeout: 8000 })
    expect(tile).toHaveTextContent(copy.quiet.harnessesIdle(RANGE_DAYS[DEFAULT_RANGE]))
    expect(card('overview-harness-activity')).toBeNull()
  })
})

describe('the coding harness preview, in place (2026-11-06)', () => {
  const p = copy.harnessPreview
  afterEach(() => vi.useRealTimers())

  it('stays out while a harness is connected', async () => {
    renderApp('/')
    await loaded()
    await screen.findByTestId('overview-harness-activity', {}, { timeout: 8000 })
    expect(screen.queryByRole('button', { name: p.watch })).toBeNull()
  })

  it('plays on Home itself, in the tile and the activity card, then comes back 5 s after it ends', async () => {
    window.history.replaceState(null, '', '/?mock=no-harnesses')
    // Between the try-a-run card's two runs, so it shows too.
    writeDone(ADMIN_ID, 'baseline')
    renderApp('/')
    await loaded()
    const tile = await screen.findByTestId('quiet-harnesses', {}, { timeout: 8000 })
    expect(tile).toHaveTextContent(copy.quiet.harnessesNone)
    expect(card('overview-harness-activity')).toBeNull()
    // Only the preview's clock is faked: the page's reads and Testing Library keep real time.
    vi.useFakeTimers({ toFake: ['setInterval', 'clearInterval'] })
    await userEvent.click(within(tile).getByRole('button', { name: p.watch }))
    // No dialog: a bar takes the header's place, and Today's command plays in the tile.
    expect(screen.queryByRole('dialog')).toBeNull()
    const bar = screen.getByRole('region', { name: p.bar })
    expect(bar).toHaveFocus()
    expect(screen.getByRole('heading', { level: 1, name: copy.title })).toBeInTheDocument()
    expect(card('quiet-harnesses')).toHaveTextContent(`nasiko connect ${window.location.origin}`)
    await userEvent.click(within(bar).getByRole('button', { name: p.jump('Month 1') }))
    expect(within(bar).getByRole('button', { name: p.jump('Month 1') })).toHaveAttribute(
      'aria-current',
      'step',
    )
    // Home's own Harnesses tile and activity card, on sample numbers.
    expect(card('quiet-harnesses')).toBeNull()
    expect(card('overview-harnesses')).not.toBeNull()
    // The sample card plays above Needs your attention; the real one sits below it (user 2026-11-06).
    expect(
      card('overview-harness-activity').compareDocumentPosition(card('overview-needs')) &
        Node.DOCUMENT_POSITION_FOLLOWING,
    ).toBeTruthy()
    // ...and above the try-a-run card, which steps down while the preview plays (user 2026-11-13).
    expect(
      card('overview-harness-activity').compareDocumentPosition(card('overview-try-run')) &
        Node.DOCUMENT_POSITION_FOLLOWING,
    ).toBeTruthy()
    act(() => {
      vi.advanceTimersByTime(PREVIEW_STEP_MS)
    })
    expect(card('overview-harnesses')).toHaveTextContent('$1,133.48')
    expect(bar).toHaveTextContent(p.back(PREVIEW_HOLD_MS / 1000))
    act(() => {
      vi.advanceTimersByTime(PREVIEW_HOLD_MS)
    })
    expect(screen.queryByRole('region', { name: p.bar })).toBeNull()
    const back = await screen.findByTestId('quiet-harnesses')
    expect(back).toHaveTextContent(copy.quiet.harnessesNone)
    expect(card('overview-harness-activity')).toBeNull()
    // The focus left with the bar: it returns to the button that started it.
    expect(within(back).getByRole('button', { name: p.watch })).toHaveFocus()
  })

  it('keeps the preview when the viewer stays, and Exit leaves at once', async () => {
    window.history.replaceState(null, '', '/?mock=no-harnesses')
    renderApp('/')
    await loaded()
    const tile = await screen.findByTestId('quiet-harnesses', {}, { timeout: 8000 })
    vi.useFakeTimers({ toFake: ['setInterval', 'clearInterval'] })
    await userEvent.click(within(tile).getByRole('button', { name: p.watch }))
    act(() => {
      vi.advanceTimersByTime(PREVIEW_MS)
    })
    const bar = screen.getByRole('region', { name: p.bar })
    await userEvent.click(within(bar).getByRole('button', { name: p.stay }))
    act(() => {
      vi.advanceTimersByTime(2 * PREVIEW_HOLD_MS)
    })
    expect(bar).toBeInTheDocument()
    expect(within(bar).getByRole('link', { name: p.connectYours })).toHaveAttribute(
      'href',
      '/harnesses',
    )
    await userEvent.click(within(bar).getByRole('button', { name: p.exit }))
    expect(screen.queryByRole('region', { name: p.bar })).toBeNull()
    expect(card('quiet-harnesses')).toHaveTextContent(copy.quiet.harnessesNone)
  })

  it('hands over the commands for the team, connect included, and stays while they are read', async () => {
    window.history.replaceState(null, '', '/?mock=no-harnesses')
    renderApp('/')
    await loaded()
    const tile = await screen.findByTestId('quiet-harnesses', {}, { timeout: 8000 })
    vi.useFakeTimers({ toFake: ['setInterval', 'clearInterval'] })
    const user = userEvent.setup()
    const writeText = vi.fn().mockResolvedValue(undefined)
    Object.defineProperty(navigator, 'clipboard', { value: { writeText }, configurable: true })
    await user.click(within(tile).getByRole('button', { name: p.watch }))
    act(() => {
      vi.advanceTimersByTime(PREVIEW_MS)
    })
    const bar = screen.getByRole('region', { name: p.bar })
    await user.click(within(bar).getByRole('button', { name: p.copyCommands }))
    expect(writeText).toHaveBeenCalledWith(
      expect.stringContaining(`| bash\nnasiko connect ${window.location.origin}`),
    )
    act(() => {
      vi.advanceTimersByTime(2 * PREVIEW_HOLD_MS)
    })
    expect(bar).toBeInTheDocument()
    expect(within(bar).getByRole('button', { name: p.copied })).toHaveFocus()
  })
})

describe('quiet tiles (2026-11-05)', () => {
  it('folds zero spend, tokens and runs into one line that says what fills them', async () => {
    configureMocks({ variant: 'no-spend' })
    renderApp('/')
    const quiet = await screen.findByTestId('quiet-activity', {}, { timeout: 8000 })
    expect(quiet).toHaveTextContent(copy.quiet.activity(RANGE_DAYS[DEFAULT_RANGE]))
    expect(within(quiet).getByRole('link', { name: copy.quiet.startChat })).toHaveAttribute(
      'href',
      '/chat',
    )
    for (const id of ['kpi-spend', 'kpi-tokens', 'kpi-runs'])
      expect(screen.queryByTestId(id)).toBeNull()
    expect(screen.getByTestId('overview-harnesses')).toBeInTheDocument()
    // Nothing spent: the Spend card steps out, never $0.00 and a table of zeros (user 2026-11-06).
    expect(card('overview-spend')).toBeNull()
  })

  it('fills the 2×2 with the quiet panel when nothing has a figure, and never puts the tip in it', async () => {
    window.history.replaceState(null, '', '/?mock=no-activity')
    configureMocks({ variant: 'no-spend' })
    renderApp('/')
    const harnesses = await screen.findByTestId('quiet-harnesses', {}, { timeout: 8000 })
    expect(harnesses).toHaveTextContent(copy.quiet.harnessesIdle(RANGE_DAYS[DEFAULT_RANGE]))
    expect(screen.getByTestId('quiet-activity')).toBeInTheDocument()
    expect(within(screen.getByTestId('overview-quiet')).queryByTestId('overview-tip')).toBeNull()
    // An empty dashboard: the tip takes the savings band's slot in the lead card, without a counter; Next turns it.
    const tip = screen.getByTestId('overview-tip')
    expect(document.querySelector('[data-tour="overview-summary"]')).toContainElement(tip)
    expect(tip).not.toHaveTextContent(/Tip \d+ of \d+/)
    const shown = () => tip.querySelector('[aria-hidden="false"]')?.textContent
    const first = shown()
    await userEvent.click(within(tip).getByRole('button', { name: copy.tips.next }))
    expect(shown()).not.toBe(first)
  })
  it('keeps the band out until the tiles show activity, with the tip in its place (2026-11-06)', async () => {
    // Savings count a call at once (`token_usage`), the tiles minutes later (`trace_usage`): savings with spend to
    // trim beside tiles that have none yet must never read "could be trimmed" beside "No agent activity".
    const savings = await apiFetch<object>('/api/observability/finops/savings')
    configureMocks({ variant: 'no-spend' })
    server.use(http.get('*/api/observability/finops/savings', () => HttpResponse.json(savings)))
    renderApp('/')
    await screen.findByTestId('quiet-activity', {}, { timeout: 8000 })
    expect(await screen.findByTestId('overview-tip')).toBeInTheDocument()
    expect(screen.queryByTestId('overview-savings')).toBeNull()
  })
})

describe('the summary tiles (redesign 2026-10-19)', () => {
  it('Tokens shows the range’s total and its input / output split; Runs shows cost per run', async () => {
    renderApp('/')
    await loaded()
    const tokens = card('kpi-tokens')
    expect(within(tokens).getByText(/^\d[\d.]*[KMB]?$/)).toBeInTheDocument()
    expect(within(tokens).getByText(/^input \d/)).toBeInTheDocument()
    expect(within(tokens).getByText(/^output \d/)).toBeInTheDocument()
    expect(within(card('kpi-runs')).getByTestId('kpi-cost-per-run')).toHaveTextContent(/per run$/)
    // Tool calls from the same dashboard (2026-11-05).
    expect(within(card('kpi-runs')).getByTestId('kpi-tool-calls')).toHaveTextContent(
      /^[\d,]+ tool calls/,
    )
    // The run states moved from the old Agents running tile to Fleet health.
    expect(screen.queryByTestId('kpi-agents')).toBeNull()
    expect(within(card('overview-health')).getByTestId('health-run-states')).toHaveTextContent(
      /^\d+ of \d+ running/,
    )
  })
})

describe('Fleet health at any fleet size (NAS-1519)', () => {
  const counts = (health: HTMLElement) =>
    within(health)
      .getAllByRole('link', { name: /^\d+ (healthy|watch|needs? action|unknown)/ })
      .map((l) => Number.parseInt(l.textContent ?? '', 10))

  it(`draws a square per agent up to ${HEALTH_SQUARES_MAX} agents`, async () => {
    window.history.replaceState(null, '', '/?mock=fleet-100')
    renderApp('/')
    await loaded()
    const health = card('overview-health')
    await waitFor(() =>
      expect(within(health).getByTestId('health-run-states')).toHaveTextContent(
        /^\d+ of 100 running/,
      ),
    )
    expect(within(health).getByTestId('health-squares').children).toHaveLength(100)
    expect(within(health).queryByTestId('health-bar')).toBeNull()
  })

  it('lists as many agents as fit the height its row gives it, not just the first 5', async () => {
    // jsdom has no layout: the list's box reports ten 44 px rows of room once observed.
    vi.stubGlobal(
      'ResizeObserver',
      class {
        cb: () => void
        constructor(cb: () => void) {
          this.cb = cb
        }
        observe(el: Element) {
          if (el.getAttribute('data-testid') === 'health-list') this.cb()
        }
        unobserve() {}
        disconnect() {}
      },
    )
    vi.spyOn(HTMLElement.prototype, 'clientHeight', 'get').mockImplementation(function (
      this: HTMLElement,
    ) {
      return this.dataset.testid === 'health-list' ? 440 : 0
    })
    vi.spyOn(HTMLElement.prototype, 'offsetHeight', 'get').mockImplementation(function (
      this: HTMLElement,
    ) {
      return this.tagName === 'LI' ? 44 : 0
    })
    window.history.replaceState(null, '', '/?mock=fleet-100')
    renderApp('/')
    await loaded()
    const list = within(card('overview-health')).getByRole('list', { name: copy.health.title })
    await waitFor(() => expect(list.children).toHaveLength(10))
  })

  it('past that, one bar whose segments are the rating counts, in the badges’ order', async () => {
    window.history.replaceState(null, '', '/?mock=fleet-1000')
    renderApp('/')
    await loaded()
    const health = card('overview-health')
    await waitFor(() =>
      expect(within(health).getByTestId('health-run-states')).toHaveTextContent(
        /^\d+ of 1000 running/,
      ),
    )
    expect(within(health).queryByTestId('health-squares')).toBeNull()
    const segments = [...within(health).getByTestId('health-bar').children] as HTMLElement[]
    expect(segments.map((s) => Number(s.style.flexGrow))).toEqual(counts(health).filter(Boolean))
  })
})

describe('Saved tokens by agent (redesign 2026-10-19)', () => {
  it('ranks visible agents largest first, each opening its hub, and folds the rest into Other', async () => {
    renderApp('/')
    await loaded()
    const saved = card('overview-saved')
    const list = await within(saved).findByRole('list', { name: copy.saved.title })
    const links = within(list).getAllByRole('link')
    expect(links.length).toBeGreaterThan(0)
    expect(links.length).toBeLessThanOrEqual(SAVED_BARS)
    for (const l of links) expect(l.getAttribute('href')).toMatch(/^\/agents\/[^/]+$/)
    expect(saved).toHaveTextContent(copy.saved.subtitle('measured'))
  })
})
