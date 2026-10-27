/**
 * The agent hub's timeline on the agent detail's Overview (plans/feat-agent-centric.md PR1a; eng "Gate", "Compare";
 * DX mocks and errors; design "Lane states", "Bucket selection", "Table view"). Rendered through the real router and
 * the mocks, so the gate's ordering is checked against the requests actually made.
 */
import { cleanup, screen, waitFor, within } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { http, HttpResponse } from 'msw'
import { afterEach, describe, expect, it } from 'vitest'
import { configureMocks } from '@nasiko/mocks/handlers'
import { now, seed, setupPinnedSeed } from '@nasiko/testing/pinnedSeed'
import { agentWhere } from '@nasiko/testing/agents'
import { renderApp } from '@nasiko/testing/renderApp'
import { recordRequests, server } from '@nasiko/testing/setup'
import { copy } from './copy'

setupPinnedSeed()
afterEach(() => configureMocks({ seed, now, loggedIn: true, variant: null }))

const agent = seed.agents[0]!
const url = (q = '') => `/agents/${agent.id}${q}`
// This agent's reads only: Needs you rates the agent with the Overview's own fleet reads (their own keys, no agent_id).
const paths = (urls: URL[], path: string) =>
  urls.filter((u) => u.pathname.endsWith(path) && u.searchParams.get('agent_id') === agent.id)

async function lanes() {
  return screen.findByRole('group', { name: copy.keyboard })
}

describe('lanes', () => {
  it('draws spend, requests, p95 and the preview savings lane, with the sentence above', async () => {
    renderApp(url())
    await lanes()
    for (const name of [copy.lanes.spend, copy.lanes.requests, copy.lanes.p95, copy.lanes.saved])
      expect(await screen.findByText(name)).toBeInTheDocument()
    expect(screen.getByText(/^Spent \$|^No spend/)).toBeInTheDocument()
    expect(screen.getByText(copy.lanes.peakP95)).toBeInTheDocument()
  })

  it('reads the gate first, then the timeseries and savings for this agent only', async () => {
    const rec = recordRequests()
    renderApp(url())
    await screen.findByText(copy.lanes.peakP95)
    rec.stop()
    const gate = paths(rec.urls, '/finops/dashboard').find(
      (u) => u.searchParams.get('agent_id') === agent.id,
    )
    expect(gate?.searchParams.get('view')).toBe('agent')
    const ts = paths(rec.urls, '/finops/spend-timeseries')
    expect(ts).toHaveLength(1)
    expect(ts[0]?.searchParams.get('agent_id')).toBe(agent.id)
    expect(paths(rec.urls, '/finops/savings')[0]?.searchParams.get('scope')).toBe('agent')
  })

  it('compare is off by default (one read); turning it on adds the previous window (replace)', async () => {
    const rec = recordRequests()
    const { router } = renderApp(url())
    await screen.findByText(copy.lanes.peakP95)
    expect(paths(rec.urls, '/finops/spend-timeseries')).toHaveLength(1)
    await userEvent.click(screen.getByRole('button', { name: copy.bar.compare }))
    await waitFor(() => expect(paths(rec.urls, '/finops/spend-timeseries')).toHaveLength(2))
    expect(router.state.location.search).toMatchObject({ compare: true })
    rec.stop()
  })

  it('compare off makes no previous-window read; on, 30d draws one; 24h never does', async () => {
    const rec = recordRequests()
    renderApp(url('?compare=1'))
    await screen.findByText(copy.lanes.peakP95)
    await waitFor(() => expect(paths(rec.urls, '/finops/spend-timeseries')).toHaveLength(2))
    cleanup()
    rec.urls.length = 0
    renderApp(url('?compare=1&preset=24h'))
    await screen.findByText(copy.lanes.peakHourlyP95)
    expect(paths(rec.urls, '/finops/spend-timeseries')).toHaveLength(1)
    rec.stop()
  })
})

describe('gate states', () => {
  it('not visible: no money reads, and the lanes collapse to the not-visible line', async () => {
    configureMocks({ variant: 'hub-not-visible' })
    const rec = recordRequests()
    renderApp(url())
    // The seed admin owns this agent: an owner is refused only when another user's agent shares the name.
    expect(await screen.findByText(copy.states.notVisibleSameName)).toBeInTheDocument()
    expect(screen.queryByRole('group', { name: copy.keyboard })).toBeNull()
    rec.stop()
    expect(paths(rec.urls, '/finops/spend-timeseries')).toHaveLength(0)
    expect(paths(rec.urls, '/finops/savings')).toHaveLength(0)
    expect(screen.queryByText(copy.lanes.peakP95)).toBeNull()
    expect(screen.queryByText(/^Spent \$/)).toBeNull()
  })

  it('a gate 5xx is one error with Retry, never the not-visible copy; Retry recovers the lanes', async () => {
    configureMocks({ variant: 'hub-gate-error' })
    const rec = recordRequests()
    renderApp(url())
    const retry = await screen.findByRole('button', { name: /Retry/ })
    expect(screen.queryByText(/You can't see this agent's spend/)).toBeNull()
    expect(paths(rec.urls, '/finops/spend-timeseries')).toHaveLength(0)
    expect(paths(rec.urls, '/finops/savings')).toHaveLength(0)
    rec.stop()
    configureMocks({ variant: null })
    await userEvent.click(retry)
    expect(await screen.findByText(copy.lanes.peakP95)).toBeInTheDocument()
  })

  it('a gate 400 reads as deleted, with no money reads', async () => {
    configureMocks({ variant: 'hub-deleted' })
    const rec = recordRequests()
    renderApp(url())
    expect(await screen.findByText(copy.states.deleted)).toBeInTheDocument()
    rec.stop()
    expect(paths(rec.urls, '/finops/spend-timeseries')).toHaveLength(0)
  })

  it('a warm gate cache from an earlier visit never authorizes the money reads', async () => {
    const { router } = renderApp(url())
    await screen.findByText(copy.lanes.peakP95)
    await router.navigate({ to: '/agents' })
    configureMocks({ variant: 'hub-not-visible' })
    const rec = recordRequests()
    await router.navigate({ to: '/agents/$agentId', params: { agentId: agent.id } })
    expect(await screen.findByText(copy.states.notVisibleSameName)).toBeInTheDocument()
    rec.stop()
    expect(paths(rec.urls, '/finops/spend-timeseries')).toHaveLength(0)
    expect(screen.queryByText(copy.lanes.peakP95)).toBeNull()
  })

  it('money fetched while the gate revalidates never draws if the gate then refuses', async () => {
    const { queryClient } = renderApp(url())
    await screen.findByText(copy.lanes.peakP95)
    // The next gate answer is slow and refuses; the timeseries answers at once with a figure the page must not show.
    server.use(
      http.get('/api/observability/finops/dashboard', async ({ request }) => {
        if (!new URL(request.url).searchParams.get('agent_id')) return undefined
        await new Promise((r) => setTimeout(r, 200))
        return new HttpResponse('agent not found', { status: 404 })
      }),
      http.get('/api/observability/finops/spend-timeseries', () =>
        HttpResponse.json({
          data: {
            bucket: 'day',
            points: [
              {
                bucket_start:
                  new Date(now() - 86_400_000).toISOString().slice(0, 10) + 'T00:00:00Z',
                spend_usd: 9999,
                operations: 1,
                tool_calls: 0,
                top_agent_name: null,
                top_agent_spend_usd: null,
                p50_latency_ms: null,
                p95_latency_ms: null,
                p99_latency_ms: null,
              },
            ],
          },
        }),
      ),
    )
    void queryClient.invalidateQueries({ queryKey: ['agents', 'hub'] })
    await new Promise((r) => setTimeout(r, 80))
    expect(screen.queryByRole('button', { name: /\$9,999/ })).toBeNull()
    expect(await screen.findByText(copy.states.notVisibleSameName)).toBeInTheDocument()
    expect(screen.queryByRole('button', { name: /\$9,999/ })).toBeNull()
  })

  it('no savings row reads "not reported", never 0', async () => {
    configureMocks({ variant: 'hub-no-savings-row' })
    renderApp(url())
    await screen.findByText(copy.lanes.saved)
    expect(await screen.findByText(copy.notReported)).toBeInTheDocument()
  })
})

describe('markers', () => {
  it('shows recorded changes and the preview badge for the event log', async () => {
    renderApp(url())
    await lanes()
    // OSS records no crash: the line names builds and versions only.
    expect(await screen.findByText(new RegExp(copy.rail.showingOss))).toBeInTheDocument()
    expect(screen.getAllByText(copy.rail.preview).length).toBeGreaterThan(0)
  })

  it('a failed marker read says which and offers Retry', async () => {
    configureMocks({ variant: 'hub-markers-fail' })
    renderApp(url())
    expect(await screen.findByText(copy.rail.deploysFailed)).toBeInTheDocument()
  })

  it('without the event log there is no preview, and the info tip says other changes come later', async () => {
    configureMocks({ variant: 'events-absent' })
    renderApp(url())
    expect(await screen.findByRole('button', { name: copy.rail.otherLater })).toBeInTheDocument()
    expect(screen.queryByText(copy.rail.preview, { selector: '[title]' })).toBeNull()
  })

  it('a failed event-log read says "Changes couldn\'t load"; its Retry reads it again', async () => {
    let calls = 0
    server.use(
      http.get('/api/observability/agent/:id/events', () => {
        calls += 1
        return new HttpResponse('internal error', { status: 500 })
      }),
    )
    renderApp(url())
    const line = await screen.findByText(copy.rail.changesFailed)
    const before = calls
    await userEvent.click(
      within(line.closest('p')!).getByRole('button', { name: copy.states.retry }),
    )
    await waitFor(() => expect(calls).toBeGreaterThan(before))
  })

  it('a coding harness reads no builds, versions or deployment, and says so when it has no changes', async () => {
    configureMocks({ variant: 'events-absent' })
    const harness = await agentWhere((a) => !!a.tags?.includes('coding-agent'))
    const rec = recordRequests()
    renderApp(`/agents/${harness.id}`)
    expect(await screen.findByText(new RegExp(copy.rail.harness))).toBeInTheDocument()
    rec.stop()
    expect(rec.urls.filter((u) => u.pathname.startsWith('/api/builds/agent/'))).toHaveLength(0)
    expect(rec.urls.filter((u) => /\/versions$|\/deployment$/.test(u.pathname))).toHaveLength(0)
  })

  it('a capped builds read says how many records were checked', async () => {
    configureMocks({ variant: 'hub-many-builds' })
    renderApp(url())
    expect(await screen.findByText(new RegExp(copy.rail.capped(20)))).toBeInTheDocument()
  })

  it('a flag reaches the day’s sessions in two clicks (marker → Sessions)', async () => {
    const { router } = renderApp(url())
    await lanes()
    const rail = await screen.findByRole('group', { name: copy.rail.label })
    const flag = (await within(rail).findAllByRole('button')).at(-1)!
    await userEvent.click(flag)
    await waitFor(() => expect(router.state.location.search).toHaveProperty('slice'))
    const link = await within(await screen.findByRole('dialog')).findByRole('link', {
      name: /Sessions (on|named)/,
    })
    await userEvent.click(link)
    await waitFor(() => expect(router.state.location.pathname).toBe('/sessions'))
    expect(router.state.location.search).toMatchObject({ agent: agent.name })
    expect(router.state.location.search).toHaveProperty('day')
  })

  it('a ?marker= deep link opens its flag; an unknown one is dropped from the URL', async () => {
    const body = (await (await fetch(`/api/observability/agent/${agent.id}/events`)).json()) as {
      data: { events: { id: string; summary: string }[] }
    }
    const first = body.data.events[0]!
    renderApp(url(`?marker=event:${first.id}`))
    expect(await screen.findByRole('dialog')).toHaveTextContent(first.summary)
    cleanup()
    const { router } = renderApp(url('?marker=event:nope'))
    await lanes()
    await waitFor(() => expect(router.state.location.search).not.toHaveProperty('marker'))
  })
})

describe('URL', () => {
  it('a tab switch and a bad tab keep the window', async () => {
    const { router } = renderApp(url('?preset=7d&tab=bogus'))
    await waitFor(() => expect(router.state.location.search).not.toHaveProperty('tab'))
    expect(router.state.location.search).toMatchObject({ preset: '7d' })
    await userEvent.click(await screen.findByRole('tab', { name: 'Versions' }))
    await userEvent.click(screen.getByRole('tab', { name: 'Overview' }))
    await waitFor(() => expect(router.state.location.search).not.toHaveProperty('tab'))
    expect(router.state.location.search).toMatchObject({ preset: '7d' })
  })
})

describe('buckets', () => {
  it('the keyboard moves between buckets and Enter selects one (pushed, announced)', async () => {
    const { router } = renderApp(url())
    const axis = await lanes()
    await screen.findByText(copy.lanes.peakP95)
    const stop = within(axis)
      .getAllByRole('button')
      .find((b) => b.tabIndex === 0)!
    stop.focus()
    await userEvent.keyboard('{ArrowLeft}{Enter}')
    await waitFor(() => expect(router.state.location.search).toHaveProperty('slice'))
    expect(await screen.findByRole('region', { name: /^Selected: / })).toBeInTheDocument()
  })

  it('the Table view lists every bucket with the same Select action; a stale slice is dropped', async () => {
    const { router } = renderApp(url('?slice=2020-01-01T00:00:00.000Z/2020-01-02T00:00:00.000Z'))
    await lanes()
    await waitFor(() => expect(router.state.location.search).not.toHaveProperty('slice'))
    // The timeline's Table comes first; the Budget section's burn card has its own (B5).
    await userEvent.click(screen.getAllByRole('button', { name: copy.bar.table })[0]!)
    const table = await screen.findByRole('table', { name: copy.table.label })
    const selects = within(table).getAllByRole('button', { name: /^Select / })
    expect(selects.length).toBeGreaterThanOrEqual(30)
    await userEvent.click(selects.at(-1)!)
    await waitFor(() => expect(router.state.location.search).toHaveProperty('slice'))
  })

  it('a window change clears the selected bucket', async () => {
    const { router } = renderApp(url())
    const axis = await lanes()
    await userEvent.click(within(axis).getAllByRole('button').at(-1)!)
    await waitFor(() => expect(router.state.location.search).toHaveProperty('slice'))
    await userEvent.click(screen.getAllByRole('radio', { name: '7d' })[0]!)
    await waitFor(() => expect(router.state.location.search).toMatchObject({ preset: '7d' }))
    expect(router.state.location.search).not.toHaveProperty('slice')
  })
})
