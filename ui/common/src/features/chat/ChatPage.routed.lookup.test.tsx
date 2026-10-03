/**
 * Opening a chat the rail hasn't loaded (v1b §5.1, EN-6, NE-5): the lookup pages past the rail,
 * caches what it found, and at its cap or on a failed page says it couldn't tell, never "removed".
 * Every test asserts the client never POSTed an assistant row.
 */
import { screen } from '@testing-library/react'
import { http, HttpResponse } from 'msw'
import { afterEach, beforeEach, describe, expect, it } from 'vitest'
import { renderApp } from '@/test/renderApp'
import { recordRequestBodies, server } from '@/test/setup'
import { clearDrafts } from './drafts'
import { clearChatRegistry } from './registry'
import type { ChatMessage, ChatSessionRow } from './types'

const CHAT = '5eedc000-0000-4000-8000-0000000fa001'

let rec: ReturnType<typeof recordRequestBodies>
beforeEach(() => {
  rec = recordRequestBodies()
})
afterEach(async () => {
  await rec.flush()
  try {
    // assistantPosts() throws if a body couldn't be read (fails closed); cleanup runs either way.
    expect(rec.assistantPosts()).toEqual([])
  } finally {
    rec.stop()
    clearChatRegistry()
    clearDrafts()
  }
})

const row = (over: Partial<ChatSessionRow> = {}): ChatSessionRow => ({
  session_id: CHAT,
  agent_id: null,
  agent_url: null,
  title: 'An old routed chat',
  created_at: '2026-09-20T10:00:00Z',
  updated_at: '2026-09-20T10:00:00Z',
  agent_name: null,
  is_coding_agent: false,
  ...over,
})
const msg = (id: string, role: 'user' | 'assistant', content: string, i: number): ChatMessage =>
  ({
    id,
    session_id: CHAT,
    role,
    content,
    timestamp: new Date(Date.parse('2026-09-20T10:00:00Z') + i * 1000).toISOString(),
    trace_id: null,
  }) as ChatMessage
const filler = (n: number, offset: number): ChatSessionRow[] =>
  Array.from({ length: n }, (_, i) =>
    row({
      session_id: `5eedc000-0000-4000-8000-${String(offset + i).padStart(12, '0')}`,
      title: `chat ${offset + i}`,
      agent_id: 'a',
      agent_url: '/api/agents/a',
    }),
  )

/** A list that never has the chat in the rail's pages (limit 50), and has it on `hitPage` of the lookup (limit 100), or never. */
function paging(hitPage: number | null) {
  server.use(
    http.get('/api/chat/sessions', ({ request }) => {
      const u = new URL(request.url)
      const limit = Number(u.searchParams.get('limit'))
      const page = Number(u.searchParams.get('cursor') ?? '0')
      const data =
        limit === 100 && page === hitPage
          ? [...filler(99, page * 1000), row()]
          : filler(limit, page * 1000)
      return HttpResponse.json({
        data,
        has_more: true,
        next_cursor: String(page + 1),
        prev_cursor: null,
      })
    }),
  )
  server.use(
    http.get(`/api/chat/sessions/${CHAT}/messages`, () =>
      HttpResponse.json({
        data: [msg('u1', 'user', 'hello', 0), msg('a1', 'assistant', 'Hi there.', 1)],
        has_more: false,
        next_cursor: null,
        prev_cursor: 'u1',
        hitl: [],
      }),
    ),
  )
}

describe('chat lookup beyond the rail (§5.1, EN-6, NE-5)', () => {
  it('finds a routed chat past the rail’s 5 pages; a second open makes no lookup requests', async () => {
    paging(7)
    renderApp(`/chat/${CHAT}`)
    expect(
      await screen.findByLabelText('Ask the Orchestrator', {}, { timeout: 8000 }),
    ).toBeInTheDocument()
    expect(screen.getByTestId('identity-chip')).toHaveTextContent('Orchestrator')
    const lookups = () =>
      rec.requests.filter(
        (r) => r.url.pathname === '/api/chat/sessions' && r.url.searchParams.get('limit') === '100',
      ).length
    await rec.flush()
    // It starts after the rail's own 5 pages (cursor 5), so pages 5, 6 and 7.
    expect(lookups()).toBe(3)
    document.body.innerHTML = ''
    renderApp(`/chat/${CHAT}`)
    expect(
      await screen.findByLabelText('Ask the Orchestrator', {}, { timeout: 8000 }),
    ).toBeInTheDocument()
    await rec.flush()
    expect(lookups()).toBe(3)
  })

  it('at the lookup cap it never says removed: it says it couldn’t tell, keeps history and locks Send', async () => {
    paging(null)
    renderApp(`/chat/${CHAT}`)
    expect(
      await screen
        .findByText(
          "Couldn't find this chat's details",
          { selector: '[role="note"]' },
          { timeout: 5000 },
        )
        .catch(() => screen.findAllByText("Couldn't find this chat's details").then((x) => x[0]!)),
    ).toBeInTheDocument()
    expect(screen.queryByText("This chat's agent was removed.")).toBeNull()
    expect(screen.getByText('Hi there.')).toBeInTheDocument()
    expect(screen.getByRole('button', { name: 'Send' })).toBeDisabled()
    expect(screen.getByRole('button', { name: 'Retry' })).toBeInTheDocument()
  }, 10_000)
})

describe('chat lookup outcomes (EN-6)', () => {
  it('pages that end without the chat mean it is not yours or gone: the missing state', async () => {
    server.use(
      http.get('/api/chat/sessions', ({ request }) => {
        const u = new URL(request.url)
        const page = Number(u.searchParams.get('cursor') ?? '0')
        // The rail pages (limit 50) run on; the lookup's first page past them is the last.
        const last = u.searchParams.get('limit') === '100'
        return HttpResponse.json({
          data: filler(Number(u.searchParams.get('limit')), page * 1000),
          has_more: !last,
          next_cursor: last ? null : String(page + 1),
          prev_cursor: null,
        })
      }),
      http.get(`/api/chat/sessions/${CHAT}/messages`, () =>
        HttpResponse.json({
          data: [msg('u1', 'user', 'hello', 0)],
          has_more: false,
          next_cursor: null,
          prev_cursor: 'u1',
          hitl: [],
        }),
      ),
    )
    renderApp(`/chat/${CHAT}`)
    expect(
      await screen.findByText("This chat's agent was removed.", {}, { timeout: 5000 }),
    ).toBeInTheDocument()
    expect(screen.queryByText("Couldn't find this chat's details")).toBeNull()
  }, 10_000)

  it('a lookup page that fails is "couldn\'t tell", never removed: history stays, Send locked, Retry', async () => {
    server.use(
      http.get('/api/chat/sessions', ({ request }) => {
        const u = new URL(request.url)
        if (u.searchParams.get('limit') === '100')
          return new HttpResponse('internal error', { status: 500 })
        const page = Number(u.searchParams.get('cursor') ?? '0')
        return HttpResponse.json({
          data: filler(50, page * 1000),
          has_more: true,
          next_cursor: String(page + 1),
          prev_cursor: null,
        })
      }),
      http.get(`/api/chat/sessions/${CHAT}/messages`, () =>
        HttpResponse.json({
          data: [msg('u1', 'user', 'hello', 0), msg('a1', 'assistant', 'Hi there.', 1)],
          has_more: false,
          next_cursor: null,
          prev_cursor: 'u1',
          hitl: [],
        }),
      ),
    )
    renderApp(`/chat/${CHAT}`)
    expect(
      (await screen.findAllByText("Couldn't find this chat's details", {}, { timeout: 5000 }))
        .length,
    ).toBeGreaterThan(0)
    expect(screen.getByText('Hi there.')).toBeInTheDocument()
    expect(screen.getByRole('button', { name: 'Send' })).toBeDisabled()
    expect(screen.getByRole('button', { name: 'Retry' })).toBeInTheDocument()
  }, 10_000)
})
