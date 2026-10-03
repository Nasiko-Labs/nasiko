/**
 * Harnesses page, OSS (plan §10): the viewer's own Individual level from the usage endpoint, the capability probe
 * (a bare 404 falls back to existing endpoints, a coded 404 is "not visible"), the forced mock variants, and the
 * one-status-line rule. The org levels are the EE layer's (its own tests, in the ee project).
 */
import { screen, waitFor, within } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { http, HttpResponse } from 'msw'
import { act } from 'react'
import { afterEach, describe, expect, it, vi } from 'vitest'
import { configureMocks, type MockVariant } from '@/mocks/handlers'
import { generateHarnessSeed } from '@/mocks/seed-harness'
import { FIXED, now, seed, setupPinnedSeed } from '@/test/pinnedSeed'
import { renderApp } from '@/test/renderApp'
import { recordRequests, server } from '@/test/setup'

const harnessSeed = generateHarnessSeed({ anchor: FIXED })
const USAGE = '/api/observability/coding-agents/usage'

setupPinnedSeed()

function as(persona: string | null, variant: MockVariant | null = null) {
  configureMocks({ seed, now, loggedIn: true, harnessSeed, persona, variant })
}
afterEach(() => as(null))

const summary = () => screen.findByTestId('harness-summary')
const statusLines = () => screen.queryAllByTestId('status-line')

describe('landing', () => {
  it('the superuser lands on their own Individual view, with nothing about another edition', async () => {
    as('admin')
    renderApp('/harnesses')
    // One page loader under the header until the landing answers, then the level.
    await screen.findByRole('heading', { level: 1, name: 'Harnesses' })
    expect(screen.getByTestId('page-loader')).toBeInTheDocument()
    expect(await summary()).toHaveTextContent(/you used/i)
    expect(screen.queryByTestId('page-loader')).toBeNull()
    expect(screen.getByRole('heading', { name: 'Recent sessions' })).toBeInTheDocument()
    expect(screen.getByRole('heading', { name: 'Daily activity' })).toBeInTheDocument()
    expect(screen.queryByRole('table')).toBeNull()
    expect(screen.queryByRole('navigation', { name: 'Breadcrumb' })).toBeNull()
    expect(statusLines()).toHaveLength(0)
    expect(document.body).not.toHaveTextContent(/Enterprise|org and team/i)
    // Mock data is a preview; the persona switcher is the EE layer's.
    expect(screen.getByText('preview (mock)')).toBeInTheDocument()
    expect(screen.queryByRole('combobox', { name: 'View as (mock persona)' })).toBeNull()
  })

  it('asks the server for the landing only: no scope, unit or other developer', async () => {
    const rec = recordRequests()
    try {
      as('admin')
      renderApp('/harnesses?scope=org&unit_id=x&user_id=y')
      await summary()
      const calls = rec.urls.filter((u) => u.pathname === USAGE)
      expect(calls.length).toBeGreaterThan(0)
      for (const u of calls)
        for (const k of ['scope', 'unit_id', 'user_id', 'group_by'])
          expect(u.searchParams.has(k), k).toBe(false)
      expect(rec.urls.some((u) => u.pathname === '/api/org/units')).toBe(false)
    } finally {
      rec.stop()
    }
  })

  it('a non-superuser: /api/users/me is 403 on OSS, so the viewer comes from the /api/me claims', async () => {
    const sam = harnessSeed.users.find((u) => u.username === 'sam')!
    const rec = recordRequests()
    try {
      as('sam')
      renderApp('/harnesses')
      expect(await summary()).toHaveTextContent(/you used/i)
      expect(screen.queryByText("Couldn't identify you")).toBeNull()
      expect(rec.urls.some((u) => u.pathname === '/api/me')).toBe(true)
      // The server answered with sam's own level: their recent sessions, linkable only for themself.
      expect(screen.getByRole('heading', { name: 'Recent sessions' })).toBeInTheDocument()
      expect(screen.queryByText('Only your own sessions open in Sessions.')).toBeNull()
      expect(sam.is_superuser).toBe(false)
    } finally {
      rec.stop()
    }
  })

  it('a non-superuser with no usage endpoint: the claims id reaches the owner query', async () => {
    const sam = harnessSeed.users.find((u) => u.username === 'sam')!
    const rec = recordRequests()
    try {
      as('sam', 'usage-404')
      renderApp('/harnesses')
      // Sam has a live Claude Code registration: "you used" proves the claims id reached the owner query.
      expect(await summary()).toHaveTextContent(/you used/i)
      expect(
        rec.urls.some(
          (u) => u.pathname === '/api/agents' && u.searchParams.get('owner') === sam.id,
        ),
      ).toBe(true)
    } finally {
      rec.stop()
    }
  })
})

describe('controls', () => {
  it('pressing a harness panel adds a removable chip', async () => {
    const user = userEvent.setup()
    as('admin')
    const { router } = renderApp('/harnesses')
    await summary()
    const panel = within(screen.getByRole('region', { name: 'Harnesses' })).getAllByRole('button', {
      pressed: false,
    })[0]!
    await user.click(panel)
    await waitFor(() =>
      expect(router.state.location.search).toMatchObject({ harness: expect.any(String) }),
    )
    expect(panel).toHaveAttribute('aria-pressed', 'true')
    await user.click(screen.getByRole('button', { name: /remove|clear/i }))
    await waitFor(() =>
      expect(router.state.location.search).not.toHaveProperty('harness', expect.any(String)),
    )
  })

  it('harness cards: the name is the toggle, and no focusable element sits inside a button', async () => {
    as('admin')
    renderApp('/harnesses')
    await summary()
    const region = screen.getByRole('region', { name: 'Harnesses' })
    expect(within(region).getByRole('button', { name: 'Filter by Claude Code' })).toHaveAttribute(
      'aria-pressed',
      'false',
    )
    for (const b of region.querySelectorAll('button'))
      expect(b.querySelector('button, a, [tabindex]')).toBeNull()
  })

  it('Compare is a pressed toggle kept in the URL; turning it off drops the turns Δ from the panels', async () => {
    const user = userEvent.setup()
    as('admin')
    const { router } = renderApp('/harnesses')
    await summary()
    const region = within(screen.getByRole('region', { name: 'Harnesses' }))
    expect(screen.getByRole('button', { name: 'Compare' })).toHaveAttribute('aria-pressed', 'true')
    await waitFor(() => expect(region.getAllByText(/^turns/).length).toBeGreaterThan(0))
    await user.click(screen.getByRole('button', { name: 'Compare' }))
    await waitFor(() => expect(router.state.location.search).toMatchObject({ compare: false }))
    expect(screen.getByRole('button', { name: 'Compare' })).toHaveAttribute('aria-pressed', 'false')
    await waitFor(() =>
      expect(
        within(screen.getByRole('region', { name: 'Harnesses' })).queryAllByText(/^turns/),
      ).toHaveLength(0),
    )
  })

  it('a custom window is clamped to 366 days before it reaches the server', async () => {
    const rec = recordRequests()
    try {
      as('admin')
      renderApp('/harnesses?preset=custom&from=0001-01-01&to=2026-03-10')
      await summary()
      const u = rec.urls.find((x) => x.pathname.endsWith('/coding-agents/usage'))!
      expect(u.searchParams.get('start_time')).toBe('2025-03-10T00:00:00.000Z')
    } finally {
      rec.stop()
    }
  })

  it('a custom window ending in the future is clamped against today, not the future end', async () => {
    const rec = recordRequests()
    try {
      as('admin')
      renderApp('/harnesses?preset=custom&from=2025-06-01&to=2027-12-31')
      await summary()
      const u = rec.urls.find((x) => x.pathname.endsWith('/coding-agents/usage'))!
      // Within 366 days of today (2026-03-20): kept as asked, not a silent fallback to 30 days.
      expect(u.searchParams.get('start_time')).toBe('2025-06-01T00:00:00.000Z')
      expect(u.searchParams.get('range')).toBeNull()
    } finally {
      rec.stop()
    }
  })
})

describe('forced states (?mock=)', () => {
  it('usage-404 (bare 404): the server line and the degraded Individual view', async () => {
    as('admin', 'usage-404')
    renderApp('/harnesses')
    expect(await summary()).toHaveTextContent(/you used/i)
    expect(statusLines()).toHaveLength(1)
    expect(statusLines()[0]).toHaveTextContent(/not available on this server/)
    expect(statusLines()[0]).not.toHaveTextContent(/org|team|Enterprise/i)
    expect(screen.getByText(/Activity from removed harnesses/)).toBeInTheDocument()
  })

  it('usage-404: sessions are requested per confirmed harness agent, never as one unfiltered list', async () => {
    as('admin', 'usage-404')
    const rec = recordRequests()
    try {
      renderApp('/harnesses')
      await screen.findByText(/Last 20 sessions/)
      const calls = rec.urls.filter((u) => u.pathname === '/api/chat/sessions')
      expect(calls.length).toBeGreaterThan(0)
      // Exactly the admin's confirmed harness agents: never the spoofed-metadata one.
      const admin = harnessSeed.users.find((u) => u.username === 'admin')!
      const spoof = harnessSeed.agents.find((a) => a.spoofed)!
      const want = harnessSeed.agents
        .filter((a) => a.owner_id === admin.id && !a.deleted && !a.spoofed)
        .map((a) => a.id)
      const asked = calls.map((u) => u.searchParams.get('agent_id'))
      expect(new Set(asked)).toEqual(new Set(want))
      expect(asked).not.toContain(spoof.id)
    } finally {
      rec.stop()
    }
  })

  it('a coded 404: "Not found or not visible", with no org way back', async () => {
    as('admin')
    server.use(
      http.get(USAGE, () =>
        HttpResponse.json(
          { error: 'user not found or not visible', code: 'user_not_visible' },
          { status: 404 },
        ),
      ),
    )
    renderApp('/harnesses')
    expect(await screen.findByText('Not found or not visible.')).toBeInTheDocument()
    expect(screen.queryByRole('button', { name: /Back to Org|your units/ })).toBeNull()
    expect(statusLines()).toHaveLength(0)
  })

  it('no-activity: the empty trend offers a wider window, and "Try 30 days" switches to it', async () => {
    const user = userEvent.setup()
    as('admin', 'no-activity')
    const { router } = renderApp('/harnesses?preset=7d')
    expect(await screen.findByText(/No harness activity in the/)).toBeInTheDocument()
    await user.click(screen.getByRole('button', { name: 'Try 30 days' }))
    await waitFor(() => expect(router.state.location.search).toMatchObject({ preset: '30d' }))
  })

  it('usage-500: an error with Retry, no status line', async () => {
    as('admin', 'usage-500')
    renderApp('/harnesses')
    expect(await screen.findByText("Couldn't load harness usage")).toBeInTheDocument()
    expect(screen.getByRole('button', { name: 'Retry' })).toBeInTheDocument()
    expect(statusLines()).toHaveLength(0)
  })
})

describe('audit gaps', () => {
  it('a failed viewer lookup says so instead of loading forever', async () => {
    as('admin')
    server.use(
      http.get('/api/users/me', () => HttpResponse.json({ error: 'boom' }, { status: 500 })),
    )
    renderApp('/harnesses')
    expect(await screen.findByText("Couldn't identify you")).toBeInTheDocument()
  })

  it('usage-404 with the finops dashboard failing: an error with Retry, not an empty view', async () => {
    as('admin', 'usage-404')
    server.use(
      http.get('/api/observability/finops/dashboard', () =>
        HttpResponse.json({ error: 'down' }, { status: 500 }),
      ),
    )
    renderApp('/harnesses')
    expect(await screen.findByText("Couldn't load your harness usage")).toBeInTheDocument()
    expect(screen.getByRole('button', { name: 'Retry' })).toBeInTheDocument()
    expect(statusLines()).toHaveLength(1)
  })

  it('live fallback pages /api/agents past a full page of 100 and confirms agents from both pages', async () => {
    as('admin', 'usage-404')
    const admin = harnessSeed.users.find((u) => u.username === 'admin')!
    const real = harnessSeed.agents
      .filter((a) => a.owner_id === admin.id && !a.deleted)
      .map((a) => ({ id: a.id, name: a.name, owner_id: a.owner_id, tags: ['coding-agent'] }))
    const filler = Array.from({ length: 100 - real.length }, (_, i) => ({
      id: `filler-${i}`,
      name: `filler-${i}`,
      owner_id: admin.id,
      tags: ['local'],
    }))
    const late = {
      id: '5eed0002-0000-4000-8000-00000000fff1',
      name: 'late-opencode',
      owner_id: admin.id,
      tags: ['coding-agent'],
    }
    server.use(
      http.get('/api/agents', ({ request }) => {
        const offset = Number(new URL(request.url).searchParams.get('offset'))
        return HttpResponse.json(offset === 0 ? [...real, ...filler] : [late])
      }),
      http.get(`/api/agents/${late.id}`, () =>
        HttpResponse.json({
          data: { id: late.id, coding_agent_integration_id: 'opencode' },
          status_code: 200,
          message: 'ok',
        }),
      ),
    )
    const rec = recordRequests()
    try {
      renderApp('/harnesses')
      await screen.findByText(/Last 20 sessions/)
      const pages = rec.urls
        .filter((u) => u.pathname === '/api/agents')
        .map((u) => u.searchParams.get('offset'))
      expect(pages).toEqual(['0', '100'])
      const detailed = rec.urls
        .filter((u) => u.pathname.startsWith('/api/agents/'))
        .map((u) => u.pathname.split('/').at(-1))
      expect(detailed).toEqual(expect.arrayContaining([real[0]!.id, late.id]))
      expect(detailed.some((id) => id!.startsWith('filler-'))).toBe(false)
      // Confirmed on page 2, so its own-only sessions are fetched too.
      const sessionAgents = rec.urls
        .filter((u) => u.pathname === '/api/chat/sessions')
        .map((u) => u.searchParams.get('agent_id'))
      expect(sessionAgents).toContain(late.id)
    } finally {
      rec.stop()
    }
  })
})

describe('review fixes (ship)', () => {
  it('a failed refresh says the numbers are from the last good load, with Retry', async () => {
    const user = userEvent.setup()
    as('admin')
    const { queryClient } = renderApp('/harnesses')
    await summary()
    server.use(
      http.get(USAGE, () =>
        HttpResponse.json({ error: 'boom', code: 'internal' }, { status: 500 }),
      ),
    )
    await queryClient.refetchQueries({ queryKey: ['harnesses'] })
    expect(await screen.findByRole('alert')).toHaveTextContent(/last successful load/)
    expect(screen.getByTestId('harness-summary')).toBeInTheDocument()
    server.resetHandlers()
    await user.click(within(screen.getByRole('alert')).getByRole('button', { name: 'Retry' }))
    await waitFor(() => expect(screen.queryByRole('alert')).toBeNull())
  })

  it('live fallback: an agent deleted between list and detail is not a harness, not an error', async () => {
    as('admin', 'usage-404')
    const victim = harnessSeed.agents.find(
      (a) =>
        a.owner_id === harnessSeed.users.find((u) => u.username === 'admin')!.id &&
        !a.deleted &&
        !a.spoofed,
    )!
    server.use(http.get(`/api/agents/${victim.id}`, () => new HttpResponse(null, { status: 404 })))
    renderApp('/harnesses')
    expect(await summary()).toHaveTextContent(/you used|registered harness/i)
    expect(screen.queryByText("Couldn't load your harness usage")).toBeNull()
  })

  it('live fallback: a failed confirmation (500) is an error, and Retry recovers', async () => {
    const user = userEvent.setup()
    as('admin', 'usage-404')
    let fail = true
    server.use(
      http.get('/api/agents/:id', () =>
        fail ? HttpResponse.json({ error: 'x' }, { status: 500 }) : undefined,
      ),
    )
    renderApp('/harnesses')
    expect(await screen.findByText("Couldn't load your harness usage")).toBeInTheDocument()
    fail = false
    await user.click(screen.getByRole('button', { name: 'Retry' }))
    expect(await summary()).toHaveTextContent(/you used/i)
  })
})

describe('login clears cached data (ship review)', () => {
  it('logging in again drops every cached query, so no page shows the previous account', async () => {
    const user = userEvent.setup()
    as('admin')
    const { queryClient } = renderApp('/login')
    queryClient.setQueryData(['harnesses', 'live', 'someone-else', 'agents'], [{ id: 'x' }])
    await user.type(await screen.findByLabelText(/username/i), 'admin')
    await user.type(screen.getByLabelText('Password'), 'changeme')
    await user.click(screen.getByRole('button', { name: /sign in|log in/i }))
    await waitFor(() =>
      expect(
        queryClient.getQueryData(['harnesses', 'live', 'someone-else', 'agents']),
      ).toBeUndefined(),
    )
  })
})

describe('review fixes (ship cycle 2)', () => {
  it('live fallback: sessions merge newest-first across agents and show 8 of them until Show all', async () => {
    const user = userEvent.setup()
    as('admin', 'usage-404')
    const admin = harnessSeed.users.find((u) => u.username === 'admin')!
    const agents = harnessSeed.agents.filter(
      (a) => a.owner_id === admin.id && !a.deleted && !a.spoofed,
    )
    const row = (agent: string, i: number, updated: string) => ({
      session_id: `x-${agent}-${i}`,
      agent_id: agent,
      created_at: '2026-03-01T00:00:00Z',
      updated_at: updated,
      is_coding_agent: true,
      message_count: i,
      total_tokens: 100,
    })
    server.use(
      http.get('/api/chat/sessions', ({ request }) => {
        const id = new URL(request.url).searchParams.get('agent_id')!
        const k = agents.findIndex((a) => a.id === id)
        // Interleaved: agent k's i-th session was last active on day (2i + k).
        return HttpResponse.json({
          data: Array.from({ length: 12 }, (_, i) =>
            row(id, i, `2026-03-${String(2 * i + k + 1).padStart(2, '0')}T12:00:00Z`),
          ).reverse(),
          has_more: false,
          next_cursor: null,
          prev_cursor: null,
        })
      }),
    )
    renderApp('/harnesses')
    const panel = (await screen.findByRole('heading', { name: 'Last 20 sessions' })).closest(
      'section',
    )!
    const rows = () => within(panel).getAllByText(/messages ·/)
    expect(rows()).toHaveLength(8)
    // The newest (highest day) first: the first row is the latest-updated session of any agent.
    const firstText = within(panel).getAllByRole('listitem')[0]!.textContent!
    expect(firstText).toMatch(/Mar (2[0-9]|3[01])/)
    await user.click(within(panel).getByRole('button', { name: /Show all 20/ }))
    expect(rows()).toHaveLength(20)
  })

  it('a refresh that fails after the rolling window moved keeps the last good numbers, with the alert', async () => {
    as('admin')
    renderApp('/harnesses?preset=7d')
    await summary()
    server.use(
      http.get(USAGE, () =>
        HttpResponse.json({ error: 'boom', code: 'internal' }, { status: 500 }),
      ),
    )
    // Leave the tab and come back 2 h later: the window (and the query key) moves.
    act(() => {
      Object.defineProperty(document, 'visibilityState', {
        configurable: true,
        get: () => 'hidden',
      })
      document.dispatchEvent(new Event('visibilitychange'))
    })
    vi.setSystemTime(new Date(FIXED.getTime() + 2 * 3_600_000))
    act(() => {
      Object.defineProperty(document, 'visibilityState', {
        configurable: true,
        get: () => 'visible',
      })
      document.dispatchEvent(new Event('visibilitychange'))
    })
    try {
      expect(await screen.findByRole('alert')).toHaveTextContent(/last successful load/)
      expect(screen.getByRole('heading', { name: 'Recent sessions' })).toBeInTheDocument()
      expect(screen.queryByText("Couldn't load harness usage")).toBeNull()
    } finally {
      vi.setSystemTime(FIXED)
    }
  })
})
