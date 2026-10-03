/**
 * Catalog and Your agents (plan §10, component): search, tags, `/`, the Yours chip, the
 * harness toggle, paging past 100, partial failure, the first-run steps; owner=sub, tabs and
 * counts, harnesses excluded, the pinned section, the deleted notice and the turn count wording.
 */
import { screen, waitFor, within } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { http, HttpResponse } from 'msw'
import { describe, expect, it } from 'vitest'
import { ADMIN_ID } from '@/mocks/seed-harness'
import { seed, setupPinnedSeed } from '@/test/pinnedSeed'
import { renderApp } from '@/test/renderApp'
import { recordRequests, server } from '@/test/setup'

setupPinnedSeed()

const live = seed.agents.filter((a) => !a.deleted)
const cards = () =>
  screen
    .getAllByRole('link')
    .filter((l) => l.getAttribute('href')?.match(/^\/agents\/[0-9a-f-]{36}$/))
const firstCard = () => screen.findByRole('link', { name: new RegExp(live[0]!.display_name) })

function row(i: number, owner = ADMIN_ID) {
  return {
    id: `0000beef-0000-4000-8000-${String(i).padStart(12, '0')}`,
    name: `bulk-${i}`,
    display_name: `Bulk ${i}`,
    description: '',
    status: 'running',
    owner_id: owner,
    version: '1.0.0',
    tags: [],
    skills: [],
    metadata: null,
    created_at: '2026-03-01T00:00:00Z',
    updated_at: '2026-03-01T00:00:00Z',
  }
}

describe('catalog', () => {
  it('lists every visible A2A agent with ownership chips and hides harnesses by default', async () => {
    renderApp('/agents')
    await firstCard()
    expect(cards()).toHaveLength(live.length)
    expect(screen.getAllByText('Yours').length).toBeGreaterThan(0)
    expect(screen.getAllByText('Available to you').length).toBeGreaterThan(0)
    await userEvent.click(screen.getByRole('button', { name: /Show coding harnesses/ }))
    await waitFor(() => expect(cards().length).toBeGreaterThan(live.length))
    expect(screen.getAllByText('Coding harness').length).toBeGreaterThan(0)
  })

  it('search filters by name, and a miss offers Clear search', async () => {
    renderApp('/agents')
    await firstCard()
    const box = screen.getByRole('searchbox', { name: 'Search agents' })
    await userEvent.type(box, live[0]!.display_name)
    await waitFor(() => expect(cards()).toHaveLength(1))
    await userEvent.clear(box)
    await userEvent.type(box, 'zzz-no-such-agent')
    expect(await screen.findByText('No agents match this search')).toBeInTheDocument()
    await userEvent.click(screen.getByRole('button', { name: 'Clear search' }))
    await waitFor(() => expect(cards()).toHaveLength(live.length))
  })

  it('"/" focuses the search box', async () => {
    renderApp('/agents')
    await firstCard()
    await userEvent.keyboard('/')
    expect(screen.getByRole('searchbox', { name: 'Search agents' })).toHaveFocus()
  })

  it("the Yours chip keeps only the caller's agents; a tag chip keeps only that tag", async () => {
    renderApp('/agents')
    await firstCard()
    await userEvent.click(screen.getByRole('button', { name: 'Yours', pressed: false }))
    await waitFor(() => expect(screen.queryByText('Available to you')).toBeNull())
    await userEvent.click(screen.getByRole('button', { name: 'Yours', pressed: true }))
    await userEvent.click(screen.getByRole('button', { name: 'ops' }))
    await waitFor(() => expect(cards().length).toBeLessThan(live.length))
    expect(cards().length).toBeGreaterThan(0)
  })

  it('pages past the 100-row server limit', async () => {
    const all = Array.from({ length: 150 }, (_, i) => row(i))
    server.use(
      http.get('/api/agents', ({ request }) => {
        const u = new URL(request.url)
        const offset = Number(u.searchParams.get('offset') ?? 0)
        return HttpResponse.json(
          all.slice(offset, offset + Number(u.searchParams.get('limit') ?? 50)),
        )
      }),
    )
    renderApp('/agents')
    await screen.findByRole('link', { name: /Bulk 0\b/ })
    await waitFor(() => expect(cards()).toHaveLength(150))
  })

  it('a failed later page keeps what loaded and says so', async () => {
    const page1 = Array.from({ length: 100 }, (_, i) => row(i))
    server.use(
      http.get('/api/agents', ({ request }) =>
        Number(new URL(request.url).searchParams.get('offset') ?? 0) === 0
          ? HttpResponse.json(page1)
          : new HttpResponse('boom', { status: 500 }),
      ),
    )
    renderApp('/agents')
    expect(await screen.findByText("Couldn't load all agents.")).toBeInTheDocument()
    expect(cards()).toHaveLength(100)
  })

  it('an empty server shows the three first-run commands with this origin', async () => {
    server.use(http.get('/api/agents', () => HttpResponse.json([])))
    renderApp('/agents')
    expect(await screen.findByText('No agents yet')).toBeInTheDocument()
    expect(screen.getByText(`nasiko connect ${window.location.origin}`)).toBeInTheDocument()
    expect(screen.getByText('nasiko new openai my-agent')).toBeInTheDocument()
    expect(screen.getByText('nasiko deploy ./my-agent')).toBeInTheDocument()
  })
})

describe('your agents', () => {
  const mine = live.filter((_, i) => i < 13 || i > 16)
  const table = async () => within((await screen.findAllByRole('table')).at(-1)!)

  it("asks for the caller's agents only (owner = me.sub)", async () => {
    const reqs = recordRequests()
    try {
      renderApp('/agents/mine')
      await screen.findByRole('heading', { name: /Needs attention/ })
      expect(
        reqs.urls.some(
          (u) => u.pathname === '/api/agents' && u.searchParams.get('owner') === ADMIN_ID,
        ),
      ).toBe(true)
    } finally {
      reqs.stop()
    }
  })

  it('pins the agents that need attention above the tabs', async () => {
    renderApp('/agents/mine')
    const pinned = (await screen.findByRole('heading', { name: /Needs attention/ })).closest(
      'section',
    )!
    expect(within(pinned).getByText(seed.agents[2]!.display_name)).toBeInTheDocument()
    expect(within(pinned).getByText(seed.agents[5]!.display_name)).toBeInTheDocument()
  })

  it('tabs carry counts, filter the rows and exclude harnesses from All', async () => {
    renderApp('/agents/mine')
    const all = await screen.findByRole('tab', { name: /^All/ })
    expect(all).toHaveTextContent(String(mine.length))
    expect(screen.getByText(/Excludes \d+ coding harness/)).toBeInTheDocument()
    await userEvent.click(screen.getByRole('tab', { name: /^Stopped/ }))
    const t = await table()
    await waitFor(() =>
      expect(t.getAllByRole('row').filter((r) => r.querySelector('[data-status]'))).toHaveLength(1),
    )
    expect(t.getByText(seed.agents[9]!.display_name)).toBeInTheDocument()
  })

  it('the crash badge fetches the deployment only when opened; with no recorded reason it says so', async () => {
    const reqs = recordRequests()
    try {
      renderApp('/agents/mine')
      const trigger = (
        await screen.findAllByRole('button', {
          name: `Why ${seed.agents[2]!.display_name} needs attention`,
        })
      )[0]!
      const fetched = () =>
        reqs.urls.some((u) => u.pathname === `/api/agents/${seed.agents[2]!.id}/deployment`)
      expect(fetched()).toBe(false)
      await userEvent.click(trigger)
      expect(await screen.findByText(/The server recorded no reason/)).toBeInTheDocument()
      expect(screen.queryByText(/OOMKilled/)).toBeNull()
      expect(fetched()).toBe(true)
    } finally {
      reqs.stop()
    }
  })

  // Regression: ISSUE-003 (/qa 2026-09-27, .gstack/qa-reports/qa-report-localhost-2026-09-27.md): the stacked
  // (mobile) summary line read "1 turns".
  it('the turn count says "1 turn" and "2 turns"', async () => {
    server.use(
      http.get('/api/agents', ({ request }) =>
        new URL(request.url).searchParams.get('offset') === '0'
          ? HttpResponse.json([row(1), row(2)])
          : HttpResponse.json([]),
      ),
      http.get('/api/observability/finops/dashboard', () =>
        HttpResponse.json({
          data: {
            agents: [
              {
                agent_id: row(1).id,
                agent_name: row(1).display_name,
                operations: 1,
                total_cost: 0.01,
              },
              {
                agent_id: row(2).id,
                agent_name: row(2).display_name,
                operations: 2,
                total_cost: 0.02,
              },
            ],
          },
        }),
      ),
    )
    renderApp('/agents/mine')
    expect(await screen.findByText('1 turn')).toBeInTheDocument()
    expect(screen.getByText('2 turns')).toBeInTheDocument()
    expect(screen.queryByText('1 turns')).toBeNull()
  })

  it('a crafted link cannot show a "Deleted …" notice (it travels in history state)', async () => {
    renderApp('/agents/mine?deleted=prod-payments&errors=Session+compromised')
    await screen.findByRole('heading', { name: /Needs attention/ })
    expect(screen.queryByText(/Deleted prod-payments/)).toBeNull()
    expect(screen.queryByText(/Session compromised/)).toBeNull()
  })
})
