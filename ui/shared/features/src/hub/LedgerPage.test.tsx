/**
 * The harness session ledger page (plans/feat-agent-centric.md PR1b; eng "Ledger", "Ledger live floor"; UC2 previews):
 * the session route's `?view=ledger`, and the ways in.
 */
import { cleanup, screen, within } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { http, HttpResponse } from 'msw'
import { afterEach, describe, expect, it } from 'vitest'
import { chatKeys } from '@nasiko/features/chat/api'
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

const C4 = '5eedc000-0000-4000-8000-00000000c004'
const C5 = '5eedc000-0000-4000-8000-00000000c005'
const c = copy.ledger
const ledgerUrl = (id: string) => `/sessions/${id}?view=ledger`
const table = async () => screen.findByRole('table', { name: c.label })

describe('the owner’s ledger', () => {
  it('one row per turn, with tools, the harness cost basis and the running total', async () => {
    renderApp(ledgerUrl(C4))
    const t = await table()
    // A header row and three turns.
    expect(within(t).getAllByRole('row')).toHaveLength(4)
    expect(within(t).getByRole('columnheader', { name: c.estCost })).toBeInTheDocument()
    expect(within(t).getByText('read_file')).toBeInTheDocument()
    expect(screen.getByText(c.totals.turns(3))).toBeInTheDocument()
    expect(screen.getByText(c.totals.complete)).toBeInTheDocument()
  })

  it('calls the harness couldn’t tie to a turn are listed under "Turn not recorded"', async () => {
    renderApp(ledgerUrl(C4))
    const p = (await screen.findByRole('heading', { name: c.turnNotRecorded })).closest(
      'section',
    ) as HTMLElement
    expect(within(p).getByText('telemetry', { exact: false })).toBeInTheDocument()
  })

  // The budgets preview is Enterprise only, so the EE suite asserts it.
  it('previews (context, memory, controls) are labelled, and controls come only from records', async () => {
    renderApp(ledgerUrl(C4))
    const t = await table()
    expect(screen.getAllByText(copy.rail.preview).length).toBeGreaterThan(0)
    expect(
      within(t).getByText(c.controlLine('Block destructive shell commands', 'blocked', 'bash')),
    ).toBeInTheDocument()
  })

  it('the ledger never edits a control: current settings are links to where they are set', async () => {
    renderApp(ledgerUrl(C4))
    const p = (await screen.findByRole('heading', { name: c.controlsNow.title })).closest(
      'section',
    ) as HTMLElement
    expect(within(p).queryAllByRole('button')).toEqual([])
    expect(within(p).queryAllByRole('switch')).toEqual([])
    expect(within(p).getByRole('link', { name: c.controlsNow.budgets })).toBeInTheDocument()
  })

  it('renders from the messages when the trace read fails (the live floor)', async () => {
    configureMocks({ variant: 'ledger-no-trace' })
    renderApp(ledgerUrl(C4))
    expect(within(await table()).getAllByRole('row')).toHaveLength(4)
  })

  it('a metadata-only session says so', async () => {
    renderApp(ledgerUrl(C5))
    expect(await screen.findByText(c.empty)).toBeInTheDocument()
    expect(screen.getByText(c.metadataOnly)).toBeInTheDocument()
  })
})

describe('more than 100 messages', () => {
  it('totals read as a loaded subtotal until the older turns load', async () => {
    const all = Array.from({ length: 150 }, (_, i) => ({
      id: `m${String(i).padStart(4, '0')}`,
      session_id: C4,
      role: i % 2 ? 'assistant' : 'user',
      content: `message ${i}`,
      timestamp: new Date(now() - (200 - i) * 60_000).toISOString(),
    }))
    server.use(
      http.get('/api/chat/sessions/:id/messages', ({ request }) => {
        const before = new URL(request.url).searchParams.get('prev_cursor')
        const end = before ? all.findIndex((m) => m.id === before) : all.length
        const data = all.slice(Math.max(0, end - 100), end)
        return HttpResponse.json({
          data,
          has_more: end > 100,
          next_cursor: null,
          prev_cursor: data[0]?.id ?? null,
          hitl: [],
        })
      }),
    )
    renderApp(ledgerUrl(C4))
    await table()
    expect(screen.getByText(c.totals.subtotal)).toBeInTheDocument()
    expect(screen.getByText(c.totals.turns(50))).toBeInTheDocument()
    await userEvent.click(screen.getByRole('button', { name: c.loadOlder }))
    expect(await screen.findByText(c.totals.complete)).toBeInTheDocument()
    expect(screen.getByText(c.totals.turns(75))).toBeInTheDocument()
  })
})

describe('someone else’s session', () => {
  it('shows metadata from the trace, never messages (owner-only, no superuser bypass)', async () => {
    const list = (await (await fetch('/api/observability/session/list?limit=25')).json()) as {
      data: { sessions: { session_id: string }[] }
    }
    const other = list.data.sessions.find((s) => !s.session_id.startsWith('5eedc'))!
    const rec = recordRequests()
    renderApp(ledgerUrl(other.session_id))
    expect(await screen.findByText(c.notOwner)).toBeInTheDocument()
    expect(screen.queryByRole('table', { name: c.label })).toBeNull()
    rec.stop()
    expect(rec.urls.filter((u) => u.pathname.startsWith('/api/budgets/session'))).toEqual([])
  })
})

describe('the ways in', () => {
  it('the trace page of a harness session offers the ledger', async () => {
    renderApp(`/sessions/${C4}`)
    const link = await screen.findByRole('link', { name: /Ledger/ })
    expect(link.getAttribute('href')).toContain('view=ledger')
  })

  it('a harness owner’s hub lists their recorded sessions, and each opens its ledger', async () => {
    const harness = await agentNamed('admin-claude-code')
    renderApp(`/agents/${harness.id}`)
    const p = (await screen.findByRole('heading', { name: copy.runs.sessions })).closest(
      'section',
    ) as HTMLElement
    expect(await within(p).findByText(copy.runs.harnessWindow)).toBeInTheDocument()
    const links = (await within(p).findAllByRole('link')).filter((l) =>
      /^\/sessions\/./.test(l.getAttribute('href') ?? ''),
    )
    expect(links.length).toBeGreaterThan(0)
    for (const l of links) expect(l.getAttribute('href')).toContain('view=ledger')
    cleanup()
    // The first one's ledger: the owner reads its turns.
    renderApp(links[0]!.getAttribute('href')!)
    expect(within(await table()).getAllByRole('row').length).toBeGreaterThan(1)
    expect(screen.getByRole('columnheader', { name: c.estCost })).toBeInTheDocument()
  })

  it('Trace goes back to the trace', async () => {
    renderApp(ledgerUrl(C4))
    await table()
    const link = screen.getByRole('link', { name: c.trace })
    expect(link.getAttribute('href')).not.toContain('view=ledger')
  })
})

describe('states the review asked for', () => {
  const ownHarnessSession = async () => {
    const harness = await agentNamed('admin-claude-code')
    const list = (await (
      await fetch(`/api/chat/sessions?agent_id=${harness.id}&limit=5`)
    ).json()) as { data: { session_id: string }[] }
    return list.data[0]!.session_id
  }

  it('someone else’s recorded harness session shows no messages (the harness owner-only rule)', async () => {
    const id = await ownHarnessSession()
    configureMocks({ persona: 'sam' })
    renderApp(ledgerUrl(id))
    expect(await screen.findByText(c.notOwner)).toBeInTheDocument()
    expect(screen.queryByRole('button', { name: c.exportCsv })).toBeNull()
  })

  it('a 404 after the ledger loaded withholds it and its export (review: Codex)', async () => {
    const { queryClient } = renderApp(ledgerUrl(C4))
    await table()
    server.use(
      http.get('/api/chat/sessions/:id/messages', () => new HttpResponse(null, { status: 404 })),
    )
    await queryClient.refetchQueries({ queryKey: chatKeys.history(C4) })
    expect(await screen.findByText(c.notOwner)).toBeInTheDocument()
    expect(screen.queryByRole('button', { name: c.exportCsv })).toBeNull()
  })

  it('a failed refresh keeps the whole session, says so and retries (review: Codex, adversarial)', async () => {
    const { queryClient } = renderApp(ledgerUrl(C4))
    await table()
    server.use(
      http.get('/api/chat/sessions/:id/messages', () => new HttpResponse('boom', { status: 400 })),
    )
    await queryClient.refetchQueries({ queryKey: chatKeys.history(C4) })
    expect(await screen.findByText(c.refreshError)).toBeInTheDocument()
    expect(screen.getByText(c.totals.complete)).toBeInTheDocument()
    expect(screen.queryByRole('button', { name: c.loadOlder })).toBeNull()
  })

  it('older turns that fail to load keep the subtotal and say so', async () => {
    const all = Array.from({ length: 150 }, (_, i) => ({
      id: `m${String(i).padStart(4, '0')}`,
      session_id: C4,
      role: i % 2 ? 'assistant' : 'user',
      content: `message ${i}`,
      timestamp: new Date(now() - (200 - i) * 60_000).toISOString(),
    }))
    server.use(
      http.get('/api/chat/sessions/:id/messages', ({ request }) => {
        if (new URL(request.url).searchParams.get('prev_cursor'))
          return new HttpResponse('boom', { status: 400 })
        const data = all.slice(-100)
        return HttpResponse.json({
          data,
          has_more: true,
          next_cursor: null,
          prev_cursor: data[0]!.id,
          hitl: [],
        })
      }),
    )
    renderApp(ledgerUrl(C4))
    await table()
    await userEvent.click(screen.getByRole('button', { name: c.loadOlder }))
    expect(await screen.findByText(c.olderError)).toBeInTheDocument()
    expect(screen.getByText(c.totals.subtotal)).toBeInTheDocument()
    expect(screen.getByText(c.totals.turns(50))).toBeInTheDocument()
  })

  // The budgets panel's own states (failure, absent route, none) are Enterprise only; the EE suite asserts them.

  it('the trace page offers the ledger only on the viewer’s own chat (review: red team)', async () => {
    server.use(
      http.get('/api/chat/sessions/:id/messages', () => new HttpResponse(null, { status: 404 })),
    )
    renderApp(`/sessions/${C4}`)
    await screen.findByRole('link', { name: /Open in TokenOps/ })
    await new Promise((r) => setTimeout(r, 200))
    expect(screen.queryByRole('link', { name: /Ledger/ })).toBeNull()
  })
})

describe('the harness hub’s sessions (review)', () => {
  it('rows are told apart by time and size (every recorded session is "Coding session"), with a Sessions link', async () => {
    const harness = await agentNamed('admin-claude-code')
    renderApp(`/agents/${harness.id}`)
    const p = (await screen.findByRole('heading', { name: copy.runs.sessions })).closest(
      'section',
    ) as HTMLElement
    await within(p).findByText(copy.runs.harnessWindow)
    expect(within(p).queryByText('Coding session')).toBeNull()
    expect(
      within(p).getByRole('link', { name: copy.runs.sessionsAll('admin-claude-code') }),
    ).toBeInTheDocument()
  })

  it('anyone but the owner keeps the session sample and never reads the owner’s chats', async () => {
    const harness = await agentNamed('admin-claude-code')
    configureMocks({ persona: 'sam' })
    const rec = recordRequests()
    renderApp(`/agents/${harness.id}`)
    const p = (await screen.findByRole('heading', { name: copy.runs.sessions })).closest(
      'section',
    ) as HTMLElement
    expect(within(p).getByText(copy.runs.sessionsWindow)).toBeInTheDocument()
    rec.stop()
    expect(
      rec.urls.filter((u) => u.pathname === '/api/chat/sessions' && u.searchParams.has('agent_id')),
    ).toEqual([])
  })
})
