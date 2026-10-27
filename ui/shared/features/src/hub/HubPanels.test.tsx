/**
 * The hub's PR1c sections on the agent's Overview (plans/feat-agent-centric.md PR1c; design "Hub section order",
 * "Needs you on the hub", "Layer cards"; CEO R2-5, R2-6, R2-7): Needs you, Recent runs, the layer cards, and the facts
 * that moved to Details.
 */
import { cleanup, screen, waitFor, within } from '@testing-library/react'
import { http, HttpResponse } from 'msw'
import { afterEach, describe, expect, it } from 'vitest'
import { configureMocks } from '@nasiko/mocks/handlers'
import { now, seed, setupPinnedSeed } from '@nasiko/testing/pinnedSeed'
import { agentNamed } from '@nasiko/testing/agents'
import { renderApp } from '@nasiko/testing/renderApp'
import { recordRequests, server } from '@nasiko/testing/setup'
import { copy } from './copy'

setupPinnedSeed()
afterEach(() =>
  configureMocks({ seed, now, loggedIn: true, variant: null, persona: null, chatVariants: [] }),
)

const agent = seed.agents[0]!
const url = (q = '') => `/agents/${agent.id}${q}`
const panel = async (name: string) =>
  (await screen.findByRole('heading', { name })).closest('section') as HTMLElement

describe('section order', () => {
  it('Needs you, the sentence, the window bar, the lanes, recent runs, then the layer cards', async () => {
    renderApp(url())
    await screen.findByText(copy.lanes.peakP95)
    const order = [
      await panel(copy.needs.title),
      screen.getByText(/^Spent |^No spend/),
      screen.getByRole('group', { name: copy.keyboard }),
      await panel(copy.runs.sessions),
      screen.getByRole('region', { name: copy.cards.title }),
    ]
    for (let i = 1; i < order.length; i++)
      expect(
        order[i - 1]!.compareDocumentPosition(order[i]!) & Node.DOCUMENT_POSITION_FOLLOWING,
      ).toBeTruthy()
  })
})

describe('Needs you', () => {
  it('states what it checked, and its verdict or rows', async () => {
    renderApp(url())
    const p = await panel(copy.needs.title)
    expect(await within(p).findByText(/^Checked /)).toHaveTextContent(copy.needs.partRequests)
  })

  it('a crashed agent needs action, rated on the last 7 days', async () => {
    renderApp(`/agents/${seed.agents[2]!.id}`)
    const p = await panel(copy.needs.title)
    expect(await within(p).findByText(copy.needs.statusAction)).toBeInTheDocument()
    expect(within(p).getByText(copy.needs.ratedOn)).toBeInTheDocument()
  })

  it('this agent’s pending requests show with a way to answer them', async () => {
    configureMocks({ chatVariants: ['waiting'] })
    renderApp(url())
    const p = await panel(copy.needs.title)
    expect(await within(p).findByText(/waiting for you$/)).toBeInTheDocument()
  })

  it('a failed requests read is its own line with Retry, never the verdict', async () => {
    configureMocks({ chatVariants: ['pending-fail'] })
    renderApp(url())
    const p = await panel(copy.needs.title)
    expect(await within(p).findByText(copy.needs.failed.requests)).toBeInTheDocument()
    expect(within(p).queryByText(copy.needs.clear)).toBeNull()
  })
})

describe('Needs you, per viewer (review)', () => {
  const pending = (agentId: string, id: string) => ({
    id,
    status: 'pending',
    kind: 'input',
    created_at: new Date(now() - 60_000).toISOString(),
    question: { message: 'Approve?' },
    execution: {
      origin: 'agent_proxy',
      agent_id: agentId,
      chat_session_id: null,
      context_id: null,
    },
  })

  it('a member counts their pending requests for this agent only', async () => {
    configureMocks({ persona: 'sam' })
    server.use(
      http.get('/api/hitl/pending', () =>
        HttpResponse.json({
          data: [
            pending(agent.id, 'r1'),
            pending(agent.id, 'r2'),
            pending(seed.agents[1]!.id, 'r3'),
          ],
        }),
      ),
    )
    renderApp(url())
    const p = await panel(copy.needs.title)
    expect(await within(p).findByText(copy.needs.requests(2))).toBeInTheDocument()
    // No chat proves where to answer them: no Answer link.
    expect(within(p).queryByRole('link', { name: copy.needs.answer })).toBeNull()
  })

  it('a member’s visit never reads the fleet-wide spend series (it isn’t ACL-scoped; review: Codex P1)', async () => {
    configureMocks({ persona: 'sam' })
    const rec = recordRequests()
    renderApp(url())
    await panel(copy.needs.title)
    await waitFor(() => expect(screen.queryByText(/^Checked /)).not.toBeNull())
    rec.stop()
    const fleetSeries = rec.urls.filter(
      (u) => u.pathname.endsWith('/finops/spend-timeseries') && !u.searchParams.get('agent_id'),
    )
    expect(fleetSeries).toEqual([])
  })

  it('a stopped agent isn’t "rated": its health is not among what was checked', async () => {
    const all: { id: string; status: string }[] = []
    for (let offset = 0; ; offset += 100) {
      const page = (await (await fetch(`/api/agents?limit=100&offset=${offset}`)).json()) as {
        id: string
        status: string
      }[]
      all.push(...page)
      if (page.length < 100) break
    }
    const stopped = all.find((a) => a.status === 'stopped')!
    expect(stopped).toBeDefined()
    renderApp(`/agents/${stopped.id}`)
    const p = await panel(copy.needs.title)
    const line = await within(p).findByText(/^Checked /)
    expect(line).not.toHaveTextContent(copy.needs.partStatus)
  })
})

describe('Recent runs', () => {
  it('reads one bounded session sample from at most 7 days back', async () => {
    const rec = recordRequests()
    renderApp(url())
    await panel(copy.runs.sessions)
    await waitFor(() =>
      expect(rec.urls.some((u) => u.pathname.endsWith('/session/list'))).toBe(true),
    )
    rec.stop()
    const reads = rec.urls.filter(
      (u) => u.pathname.endsWith('/session/list') && u.searchParams.get('limit') === '25',
    )
    expect(reads.length).toBeGreaterThan(0)
    const start = Date.parse(reads[0]!.searchParams.get('start_time')!)
    expect(now() - start).toBeLessThanOrEqual(7 * 86_400_000)
  })

  it('an empty sessions list says what it checked and links to Sessions', async () => {
    renderApp(url())
    const p = await panel(copy.runs.sessions)
    expect(
      await within(p).findByRole('link', { name: copy.runs.sessionsAll(agent.name) }),
    ).toBeInTheDocument()
    await waitFor(() =>
      expect(
        within(p).queryByText(/^None in the latest \d+ sessions checked\.$/) ??
          within(p)
            .queryAllByRole('link')
            .find((l) => /^\/sessions\/./.test(l.getAttribute('href') ?? '')),
      ).toBeTruthy(),
    )
  })

  it('lists only flows this agent started, and says so when none are in the window', async () => {
    configureMocks({ variant: 'flows-empty' })
    renderApp(url())
    const p = await panel(copy.runs.flows)
    expect(await within(p).findByText(copy.runs.flowsNone)).toBeInTheDocument()
  })
})

describe('flows errors', () => {
  it('a failed flows read shows its error with Retry, never "none"', async () => {
    server.use(http.get('/api/flows', () => new HttpResponse('internal error', { status: 500 })))
    renderApp(url())
    const p = await panel(copy.runs.flows)
    expect(
      await within(p).findByText(copy.runs.flowsError, {}, { timeout: 15_000 }),
    ).toBeInTheDocument()
    expect(within(p).queryByText(copy.runs.flowsNone)).toBeNull()
  })
})

describe('layer cards', () => {
  // Budget is its own section, not a layer card (B5). That it renders at all is Enterprise only, so the EE suite
  // asserts it; here we only hold that it is never a card.
  it('the owner sees Routing, Tools and Optimization', async () => {
    renderApp(url())
    const cards = await screen.findByRole('region', { name: copy.cards.title })
    for (const t of [copy.cards.routing, copy.cards.tools, copy.cards.optimization])
      expect(
        await within(cards).findByRole('heading', { name: new RegExp(`^${t}`) }),
      ).toBeInTheDocument()
    expect(within(cards).queryByRole('heading', { name: /budget/i })).toBeNull()
  })

  it('a coding harness’s owner sees Tools and Optimization, never Routing or Budget', async () => {
    const harness = await agentNamed('admin-claude-code')
    const rec = recordRequests()
    renderApp(`/agents/${harness.id}`)
    const cards = await screen.findByRole('region', { name: copy.cards.title })
    // The owner's Token optimization switch sits in the Optimization card, with its own heading.
    await within(cards).findByRole('heading', { name: copy.cards.tools })
    const titles = within(cards)
      .getAllByRole('heading')
      .map((h) => h.textContent)
    expect(titles).toContain(copy.cards.optimization)
    expect(titles).not.toContain(copy.cards.routing)
    expect(titles.filter((t) => /budget/i.test(t ?? ''))).toEqual([])
    rec.stop()
    expect(rec.urls.filter((u) => u.pathname.endsWith('/llm-config'))).toEqual([])
  })

  it('another viewer sees only Optimization, and never reads routing or tools', async () => {
    configureMocks({ persona: 'sam' })
    const rec = recordRequests()
    renderApp(url())
    const cards = await screen.findByRole('region', { name: copy.cards.title })
    expect(
      within(cards)
        .getAllByRole('heading')
        .map((h) => h.textContent),
    ).toEqual([copy.cards.optimization])
    rec.stop()
    expect(rec.urls.filter((u) => u.pathname.endsWith('/llm-config'))).toEqual([])
    expect(rec.urls.filter((u) => u.pathname.includes('/api/mcp/agents/'))).toEqual([])
  })
})

describe('the facts moved to Details', () => {
  it('Overview no longer carries them', async () => {
    renderApp(url())
    await screen.findByText(copy.lanes.peakP95)
    expect(screen.queryByRole('heading', { name: 'What it does' })).toBeNull()
    expect(screen.queryByRole('heading', { name: 'Technical details' })).toBeNull()
  })

  it('Details carries the active version, routing, what it does and the technical details', async () => {
    cleanup()
    renderApp(url('?tab=details'))
    for (const name of ['Active version', 'What it does', 'Technical details'])
      expect(await screen.findByRole('heading', { name })).toBeInTheDocument()
    expect(screen.getByText(`nasiko chat --agent ${agent.id} "hello"`)).toBeInTheDocument()
  })

  it('Activity no longer lists recent sessions (the hub does)', async () => {
    renderApp(url('?tab=activity'))
    await screen.findByRole('heading', { name: 'Last 24 hours' })
    expect(screen.queryByRole('heading', { name: 'Recent sessions' })).toBeNull()
  })
})
