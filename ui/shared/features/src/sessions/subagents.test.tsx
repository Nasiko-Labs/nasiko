/**
 * Subagents (coding-agent import plan R15) on the Sessions list, the session trace and the ledger. The mock's recorded
 * harness chat c004 ran two subagents (mocks/subagents.ts), spawned by the two `Task` calls on its third turn.
 */
import { screen, waitFor, within } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { http, HttpResponse } from 'msw'
import { afterEach, describe, expect, it, onTestFinished } from 'vitest'
import { copy } from '@nasiko/features/observability/copy'
import { configureMocks } from '@nasiko/mocks/handlers'
import { SUBAGENT_PARENT } from '@nasiko/mocks/subagents'
import { now, seed, setupPinnedSeed } from '@nasiko/testing/pinnedSeed'
import { renderApp } from '@nasiko/testing/renderApp'
import { recordRequests, server } from '@nasiko/testing/setup'

setupPinnedSeed()
afterEach(() =>
  configureMocks({ seed, now, loggedIn: true, variant: null, persona: null, chatVariants: [] }),
)

const s = copy.subagents
const EXPLORE = '5eedc000-0000-4000-8000-0000000c004a'
const REVIEW = '5eedc000-0000-4000-8000-0000000c004b'
/** Marks a test's own read of the core mock, which its override lets through. */
const PASS = 'x-test-passthrough'
const R15_KEYS = [
  'parent_session_id',
  'subagent_count',
  'subagents',
  'spawn_tool_call_id',
  'subagent_type',
]

const parentRow = async () => {
  const list = await screen.findByRole('list', { name: 'Sessions' })
  const link = within(list)
    .getAllByRole('link')
    .find((a) => a.getAttribute('href')?.startsWith(`/sessions/${SUBAGENT_PARENT}`))
  if (!link) throw new Error('no c004 row')
  return link
}
const expandButton = (link: HTMLElement) =>
  link.parentElement!.querySelector('button[aria-expanded]') as HTMLElement
const childReads = (urls: URL[]) =>
  urls.filter((u) => u.pathname.endsWith('/session/list') && u.searchParams.has('parent'))

/** Answers every observability read as a server from before R15: none of its fields anywhere. */
function olderServer() {
  const strip = (v: unknown): unknown =>
    Array.isArray(v)
      ? v.map(strip)
      : v && typeof v === 'object'
        ? Object.fromEntries(
            Object.entries(v)
              .filter(([k]) => !R15_KEYS.includes(k))
              .map(([k, x]) => [k, strip(x)]),
          )
        : v
  server.use(
    http.get('/api/observability/session/:id', async ({ request }) => {
      // The inner read falls through to the core mock.
      if (request.headers.get(PASS)) return undefined
      const url = new URL(request.url)
      // A pre-R15 server ignores `parent=` and answers its usual list; the UI must never ask, though.
      url.searchParams.delete('parent')
      const res = await fetch(new Request(url, { headers: { [PASS]: '1' } }))
      if (!res.ok) return res
      return HttpResponse.json(strip(await res.json()) as Record<string, unknown>)
    }),
  )
}

describe('the Sessions list', () => {
  it('a parent row shows "2 subagents"; its children are read only once it is expanded', async () => {
    const rec = recordRequests()
    onTestFinished(() => rec.stop())
    const user = userEvent.setup()
    renderApp('/sessions?live=paused')
    const link = await parentRow()
    // Drawn in both the narrow and the wide row layouts (CSS picks one).
    expect(within(link).getAllByText(s.chip(2)).length).toBeGreaterThan(0)
    expect(link.getAttribute('aria-label')).toContain(s.chip(2))
    expect(childReads(rec.urls)).toHaveLength(0)
    // The subagent sessions themselves are not top-level rows.
    const ids = within(await screen.findByRole('list', { name: 'Sessions' }))
      .getAllByRole('link')
      .map((a) => a.getAttribute('href') ?? '')
    expect(ids.some((h) => h.includes(EXPLORE) || h.includes(REVIEW))).toBe(false)

    await user.click(expandButton(link))
    const runs = await screen.findByRole('list', { name: s.listLabel(2) })
    expect(childReads(rec.urls).map((u) => u.searchParams.get('parent'))).toEqual([SUBAGENT_PARENT])
    const items = within(runs).getAllByRole('listitem')
    expect(items).toHaveLength(2)
    const explore = within(runs).getByRole('link', { name: 'Explore' })
    expect(explore.getAttribute('href')).toContain(`/sessions/${EXPLORE}`)
    expect(within(runs).getByRole('link', { name: 'general-purpose' })).toHaveAttribute(
      'href',
      expect.stringContaining(`/sessions/${REVIEW}`),
    )
    // Explore: 3 turns, 18.4K tokens, $0.08 (16K in + 2.4K out at $3 / $15 per M), 1 m 35 s.
    const row = explore.closest('li')!
    expect(row).toHaveTextContent('3')
    expect(row).toHaveTextContent('$0.08')
    expect(row).toHaveTextContent('1 m 35 s')
  })

  it("with the session's own cost unknown, only the subagents' cost is shown, labelled", async () => {
    renderApp('/sessions?live=paused')
    const link = await parentRow()
    // c004's own cost is the DB-only row's null: nothing is added to "—".
    expect(within(link).getAllByText(s.onlyCost('$0.13')).length).toBeGreaterThan(0)
    expect(within(link).queryByText(/incl\. subagents/)).toBeNull()
  })

  it('"incl. subagents" sits beside the own cost, which stays the main figure', async () => {
    server.use(
      http.get('/api/observability/session/list', async ({ request }) => {
        if (request.headers.get(PASS) || new URL(request.url).searchParams.has('parent'))
          return undefined
        const res = await fetch(new Request(request.url, { headers: { [PASS]: '1' } }))
        const body = (await res.json()) as {
          data: { sessions: { session_id: string; cost_summary: unknown }[] }
        }
        return HttpResponse.json({
          data: {
            ...body.data,
            sessions: body.data.sessions.map((r) =>
              r.session_id === SUBAGENT_PARENT
                ? { ...r, cost_summary: { total: { cost: 0.5 } } }
                : r,
            ),
          },
        })
      }),
    )
    renderApp('/sessions?live=paused')
    const link = await parentRow()
    await waitFor(() =>
      expect(within(link).getAllByText(s.inclCost('$0.63')).length).toBeGreaterThan(0),
    )
    // The cost column still reads the session's own $0.50.
    expect(within(link).getAllByText('$0.50').length).toBeGreaterThan(0)
  })

  it('an older server: no chip, and expanding never asks for children', async () => {
    olderServer()
    const rec = recordRequests()
    onTestFinished(() => rec.stop())
    const user = userEvent.setup()
    renderApp('/sessions?live=paused')
    const link = await parentRow()
    expect(within(link).queryByText(s.chip(2))).toBeNull()
    await user.click(expandButton(link))
    expect(await screen.findByText('Tokens · turns')).toBeInTheDocument()
    expect(screen.queryByRole('list', { name: /^Subagents/ })).toBeNull()
    expect(childReads(rec.urls)).toHaveLength(0)
  })
})

describe('the session pages', () => {
  it("a parent's trace page lists its subagents, each linking to its own session", async () => {
    renderApp(`/sessions/${SUBAGENT_PARENT}`)
    const panel = (await screen.findByRole('heading', { name: `${s.title} · 2` })).closest(
      'section',
    ) as HTMLElement
    expect(within(panel).getByRole('link', { name: 'Explore' })).toHaveAttribute(
      'href',
      expect.stringContaining(`/sessions/${EXPLORE}`),
    )
    expect(within(panel).getByRole('link', { name: 'general-purpose' })).toBeInTheDocument()
    expect(screen.queryByText(s.spawnedBy)).toBeNull()
  })

  it("a subagent's page links back to the session that spawned it", async () => {
    renderApp(`/sessions/${EXPLORE}`)
    const line = (await screen.findByText(s.spawnedBy)).closest('p') as HTMLElement
    const back = within(line).getByRole('link')
    expect(back.getAttribute('href')).toContain(`/sessions/${SUBAGENT_PARENT}`)
    // The parent's title, once its detail reads.
    await waitFor(() => expect(back).toHaveTextContent('Refactor billing webhook'))
    expect(line).toHaveTextContent(s.spawnedAs('Explore'))
    expect(screen.queryByRole('heading', { name: /^Subagents/ })).toBeNull()
  })

  it('the ledger marks the calls that spawned subagents, and shows the panel', async () => {
    renderApp(`/sessions/${SUBAGENT_PARENT}?view=ledger`)
    const links = await screen.findAllByRole('link', { name: /subagent this call spawned/ })
    expect(links).toHaveLength(2)
    expect(links.map((l) => l.textContent)).toEqual([s.spawnedStep, s.spawnedStep])
    expect(links[0]!.getAttribute('href')).toContain(`/sessions/${EXPLORE}`)
    expect(links[1]!.getAttribute('href')).toContain(`/sessions/${REVIEW}`)
    expect(screen.getByRole('heading', { name: `${s.title} · 2` })).toBeInTheDocument()
  })

  it('an older server: the trace page has no subagent section or parent link', async () => {
    olderServer()
    renderApp(`/sessions/${SUBAGENT_PARENT}`)
    expect(await screen.findByRole('link', { name: /Ledger/ })).toBeInTheDocument()
    expect(screen.queryByRole('heading', { name: /^Subagents/ })).toBeNull()
    expect(screen.queryByText(s.spawnedBy)).toBeNull()
  })
})
