import { screen, waitFor, within } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { http, HttpResponse } from 'msw'
import { afterEach, describe, expect, it } from 'vitest'
import { copy } from '@/features/observability/copy'
import { TEMPO_MAX_SEARCH_MS, TEMPO_SAFETY_MS } from '@/features/observability/tuning'
import { configureMocks } from '@/mocks/handlers'
import { observabilityData } from '@/mocks/observability'
import { now, seed, setupPinnedSeed } from '@/test/pinnedSeed'
import { renderApp } from '@/test/renderApp'
import { REPLAY_HOLD } from './live'
import { recordRequests, server } from '@/test/setup'

setupPinnedSeed()
afterEach(() => configureMocks({ variant: null }))

const rows = async () =>
  within(await screen.findByRole('list', { name: 'Sessions' })).getAllByRole('link')

describe('Sessions: fleet mode', () => {
  it('lists the window newest first with a status caption and Load more', async () => {
    renderApp('/sessions?live=paused')
    const list = await rows()
    expect(list.length).toBeGreaterThan(20)
    expect(await screen.findByText(/^Status checked for \d+ of \d+$/)).toBeInTheDocument()
    expect(screen.getByRole('button', { name: 'Load more' })).toBeInTheDocument()
    expect(screen.getByRole('button', { name: copy.resumeLive })).toHaveAttribute(
      'aria-pressed',
      'false',
    )
  })

  it("mock replay holds back today's newest sessions while Live runs", async () => {
    renderApp('/sessions?live=paused')
    const paused = (await rows()).length
    screen.getByRole('list', { name: 'Sessions' }).remove()
    renderApp('/sessions')
    await waitFor(async () =>
      expect((await screen.findAllByRole('list', { name: 'Sessions' })).length).toBeGreaterThan(0),
    )
    const live = within(screen.getAllByRole('list', { name: 'Sessions' }).at(-1)!).getAllByRole(
      'link',
    ).length
    expect(live).toBe(paused - REPLAY_HOLD)
  })

  it('lanes filter the list; failing only counts checked rows', { timeout: 20_000 }, async () => {
    const user = userEvent.setup()
    renderApp(`/sessions?live=paused&day=${seed.spikeDate}`)
    const chip = await screen.findByRole('button', { name: /^\d+ failing · \d+ checked/ })
    // Wait for every status check to settle (no trailing "…").
    await waitFor(() => expect(chip).toHaveAccessibleName(/^\d+ failing · \d+ checked$/), {
      timeout: 8000,
    })
    const n = Number(chip.textContent!.match(/(\d+) failing/)![1])
    expect(n).toBeGreaterThanOrEqual(6) // the six PR-review retry storms, at least
    await user.click(chip)
    expect(chip).toHaveAttribute('aria-pressed', 'true')
    const shown = await rows()
    expect(shown).toHaveLength(n)
    expect(shown.every((r) => / · ✕ failed · /.test(r.getAttribute('aria-label') ?? ''))).toBe(true)
  })

  it('an agent carried from TokenOps as a UUID maps to its raw name', async () => {
    const agent = seed.agents.find((a) => a.name === 'seed-code-reviewer')!
    renderApp(`/sessions?live=paused&day=${seed.spikeDate}&agent=${agent.id}`)
    const shown = await rows()
    expect(shown.every((r) => /· Code Reviewer ·/.test(r.getAttribute('aria-label') ?? ''))).toBe(
      true,
    )
  })

  it('provider/model filters are shown as not applied; an unknown agent gets a notice', async () => {
    renderApp('/sessions?live=paused&model=gpt-4o&agent=nobody')
    expect(await screen.findByText(copy.unappliedFilter('Model', 'gpt-4o'))).toBeInTheDocument()
    expect(await screen.findByText(copy.unknownAgentFilter('nobody'))).toBeInTheDocument()
  })
})

describe('Sessions: day mode', () => {
  it('shows the divergence note on a day with non-chat (workflow) spend', async () => {
    const data = observabilityData(seed)
    const day = data.nonChat[data.nonChat.length - 1].started_at.slice(0, 10)
    renderApp(`/sessions?day=${day}`)
    expect(
      await screen.findByText(
        /^Chat sessions only · \$[\d.,]+ of \$[\d.,]+ this day$/,
        {},
        { timeout: 8000 },
      ),
    ).toBeInTheDocument()
  })

  it('no divergence note on the spike day (all chat)', async () => {
    const { queryClient } = renderApp(`/sessions?day=${seed.spikeDate}`)
    await rows()
    await waitFor(() => expect(queryClient.isFetching()).toBe(0), { timeout: 8000 })
    expect(screen.queryByText(/^Chat sessions only/)).toBeNull()
  })

  it('a failing scan page keeps the loaded rows and says so', { timeout: 20_000 }, async () => {
    configureMocks({ variant: 'scan-fail' })
    renderApp(`/sessions?day=${seed.spikeDate}`)
    expect(
      await screen.findByText(/^Scanned \d+ of up to 300; page 2 failed\./, {}, { timeout: 8000 }),
    ).toBeInTheDocument()
    expect(screen.getByRole('button', { name: 'Retry' })).toBeInTheDocument()
  })

  it('Clear day returns to fleet mode', async () => {
    const user = userEvent.setup()
    const { router } = renderApp(`/sessions?day=${seed.spikeDate}&live=paused`)
    await user.click(await screen.findByRole('button', { name: /Clear day/ }))
    await waitFor(() => expect(router.state.location.search).not.toHaveProperty('day'))
    expect(await screen.findByRole('heading', { level: 1, name: 'Sessions' })).toBeInTheDocument()
  })
})

// Regression from the /ship adversarial review (2026-09-26): a past window pages forward to its sessions instead of
// saying "No sessions", and Live is off for it.
describe('Sessions: a window that ended in the past', () => {
  it(
    'a custom past window lists its sessions (paging past newer rows) with Live off',
    { timeout: 30_000 },
    async () => {
      const rec = recordRequests()
      // Pinned "now" is Mar 20: the newest 100 sessions are all after Mar 10, so page 1 filters to nothing.
      renderApp('/sessions?preset=custom&from=2026-03-01&to=2026-03-10')
      const list = await screen.findByRole('list', { name: 'Sessions' }, { timeout: 20_000 })
      expect(rec.urls.filter((u) => u.pathname.endsWith('/session/list')).length).toBeGreaterThan(1)
      rec.stop()
      expect(within(list).getAllByRole('link').length).toBeGreaterThan(0)
      expect(screen.queryByText(copy.emptyTitle)).toBeNull()
      expect(screen.getByText(copy.pastWindow)).toBeInTheDocument()
      expect(screen.queryByRole('button', { name: copy.pauseLive })).toBeNull()
    },
  )
})

describe('Sessions: states', () => {
  it('empty window: says so and offers a wider window', async () => {
    configureMocks({ variant: 'empty' })
    renderApp('/sessions?preset=7d')
    expect(await screen.findByText(copy.emptyTitle)).toBeInTheDocument()
    expect(screen.queryByTestId('page-loader')).toBeNull()
    expect(screen.getByRole('button', { name: 'Show the last 30 days' })).toBeInTheDocument()
  })

  it('filters that match nothing say so and offer to clear them', async () => {
    renderApp('/sessions?live=paused&preset=24h&agent=no-such-agent')
    expect(await screen.findByText(copy.noMatchTitle)).toBeInTheDocument()
    expect(screen.queryByRole('list', { name: 'Sessions' })).toBeNull()
    await userEvent.setup().click(screen.getByRole('button', { name: copy.clearFilters }))
    const list = await screen.findByRole('list', { name: 'Sessions' })
    expect(within(list).getAllByRole('link').length).toBeGreaterThan(0)
  })

  it('rows without trace data still list (each opens its own page), under a note with the fix', async () => {
    configureMocks({ variant: 'tempo-down' })
    renderApp('/sessions?live=paused&preset=24h')
    expect(await screen.findByText(new RegExp(copy.noTraceData.slice(0, 40)))).toBeInTheDocument()
    expect(screen.queryByText(new RegExp(copy.noTraceDataLongWindow.slice(0, 40)))).toBeNull()
    expect((await rows()).length).toBeGreaterThan(0)
  })

  it('a 30d window explains the Tempo search limit, and points to 7d', async () => {
    configureMocks({ variant: 'tempo-down' })
    renderApp('/sessions?live=paused&preset=30d')
    expect(
      await screen.findByText(new RegExp(copy.noTraceDataLongWindow.slice(0, 40))),
    ).toBeInTheDocument()
    expect(screen.getByText(/or pick 7d\./)).toBeInTheDocument()
    expect(screen.queryByText(new RegExp(copy.noTraceData.slice(0, 40)))).toBeNull()
    expect((await rows()).length).toBeGreaterThan(0)
  })

  it("defaults to 7d, sent just inside Tempo's search limit", async () => {
    const seen = recordRequests()
    renderApp('/sessions?live=paused')
    expect((await rows()).length).toBeGreaterThan(0)
    seen.stop()
    const list = seen.urls.find((u) => u.pathname.endsWith('/session/list'))
    const start = Date.parse(list?.searchParams.get('start_time') ?? '')
    const back = now() - start
    expect(back).toBeLessThan(TEMPO_MAX_SEARCH_MS)
    expect(back).toBeGreaterThan(TEMPO_MAX_SEARCH_MS - TEMPO_SAFETY_MS - 60_000)
  })

  it('day mode judges the day it asks for, not the preset (today under 30d is a short search)', async () => {
    configureMocks({ variant: 'tempo-down' })
    renderApp(`/sessions?live=paused&preset=30d&day=${new Date(now()).toISOString().slice(0, 10)}`)
    expect(await screen.findByText(new RegExp(copy.noTraceData.slice(0, 40)))).toBeInTheDocument()
    expect(screen.queryByText(new RegExp(copy.noTraceDataLongWindow.slice(0, 40)))).toBeNull()
  })

  it('server errors show the store-error state with Retry', async () => {
    server.use(
      http.get(
        '/api/observability/session/list',
        () => new HttpResponse('internal error', { status: 500 }),
      ),
    )
    renderApp('/sessions?live=paused')
    expect(await screen.findByText(copy.traceStoreError)).toBeInTheDocument()
    expect(screen.getByRole('button', { name: 'Retry' })).toBeInTheDocument()
  })

  it('a session with no traces reads "unknown", not ok', async () => {
    server.use(
      http.get('/api/observability/session/:id', ({ params }) =>
        params.id === 'list'
          ? undefined
          : HttpResponse.json({
              data: { session: { session_id: params.id, traces: [] } },
              status_code: 200,
              message: 'ok',
            }),
      ),
    )
    renderApp('/sessions?live=paused')
    await rows()
    await waitFor(() =>
      expect(
        within(screen.getByRole('list', { name: 'Sessions' }))
          .getAllByRole('link')[0]
          .getAttribute('aria-label'),
      ).toMatch(/ · \? unknown · /),
    )
  })

  it('row expand shows the token split from the session detail', async () => {
    const user = userEvent.setup()
    renderApp(`/sessions?day=${seed.spikeDate}`)
    const first = (await rows())[0]
    const expand = first.parentElement!.querySelector('button[aria-expanded]') as HTMLButtonElement
    await user.click(expand)
    expect(expand).toHaveAttribute('aria-expanded', 'true')
    expect(await screen.findByText(/in · .* out · \d+ traces$/)).toBeInTheDocument()
  })
})
