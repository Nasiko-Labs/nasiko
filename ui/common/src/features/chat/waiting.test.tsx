/**
 * Chat v1c M4, the Waiting queue (plans/feat-chat-v1c.md §5.9, §7 tests 5, 7, 14, the Waiting rows of 13 and
 * axe for the Waiting screens).
 */
import type { QueryClient } from '@tanstack/react-query'
import { screen, waitFor, within } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import axe from 'axe-core'
import { http, HttpResponse } from 'msw'
import { afterEach, describe, expect, it, vi } from 'vitest'
import { configureChatMock } from '@/mocks/chatStore'
import { configureMocks } from '@/mocks/handlers'
import { renderApp } from '@/test/renderApp'
import { server } from '@/test/setup'
import { pendingInterval } from './api'
import { copy } from './copy'
import { clearDrafts } from './drafts'
import { unwrapPending } from './normalize'
import { matchPending, requestPreview } from './pending'
import { clearChatRegistry } from './registry'
import { tuning } from './tuning'
import type { HitlDto } from './types'

const C6 = '5eedc000-0000-4000-8000-00000000c006'
const C7 = '5eedc000-0000-4000-8000-00000000c007'
const C8 = '5eedc000-0000-4000-8000-00000000c008'

afterEach(() => {
  clearChatRegistry()
  clearDrafts()
  vi.unstubAllGlobals()
})

// ─── matchPending (test 5) ────────────────────────────────────────────────────

let n = 0
const req = (
  over: Partial<HitlDto['execution']> & { origin: string },
  created = `2026-03-20T10:0${n % 10}:00Z`,
): HitlDto => ({
  id: `h${++n}`,
  kind: 'input_required',
  status: 'pending',
  resume_status: 'not_started',
  question: { message: 'q' } as HitlDto['question'],
  human_response: null,
  execution: {
    agent_id: 'a',
    task_id: null,
    context_id: null,
    chat_session_id: null,
    maf_execution_id: null,
    maf_step_index: null,
    ...over,
  },
  allowed_actions: [],
  expires_at: '',
  created_at: created,
  resolved_at: null,
})

describe('matchPending (test 5)', () => {
  it('places by the index, then chat_session_id, then a loaded direct_chat context_id (/review D2)', () => {
    const byIndex = req({ origin: 'mcp_tool' }, '2026-03-20T08:00:00Z')
    const bySession = req(
      { origin: 'orchestrator', chat_session_id: 'c-routed', context_id: 'sub-ctx' },
      '2026-03-20T09:00:00Z',
    )
    const byContext = req({ origin: 'direct_chat', context_id: 'c-direct' }, '2026-03-20T10:00:00Z')
    const unloadedContext = req(
      { origin: 'direct_chat', context_id: 'c-elsewhere' },
      '2026-03-20T10:30:00Z',
    )
    const proxy = req({ origin: 'agent_proxy', context_id: 'c-proxy' }, '2026-03-20T11:00:00Z')
    const routedContextOnly = req(
      { origin: 'orchestrator', context_id: 'sub-ctx' },
      '2026-03-20T12:00:00Z',
    )
    const maf = req({ origin: 'maf', maf_execution_id: 'x' }, '2026-03-20T13:00:00Z')
    const got = matchPending(
      [byIndex, bySession, byContext, unloadedContext, proxy, routedContextOnly, maf],
      new Set(['c-direct', 'c-proxy']),
      new Map([[byIndex.id, 'c-tool']]),
      false,
    )
    expect(got.chats.map((c) => [c.sessionId, c.source])).toEqual([
      ['c-tool', 'index'],
      ['c-routed', 'chat_session_id'],
      ['c-direct', 'context_id'],
    ])
    // An unloaded context could be no chat at all; a proxy's context is the outside caller's, even when it
    // names a loaded chat; a routed request's context is the sub-agent's; maf rows have no chat. All outside.
    expect(got.outside).toBe(4)
    expect(got.dropped).toBe(0)
  })

  it('a superuser sees only provable matches: the index, or a loaded row by chat_session_id or direct_chat context_id', () => {
    const indexed = req({ origin: 'mcp_tool' }, '2026-03-20T08:00:00Z')
    const loaded = req({ origin: 'orchestrator', chat_session_id: 'mine' }, '2026-03-20T09:00:00Z')
    const notLoaded = req(
      { origin: 'orchestrator', chat_session_id: 'someone-elses' },
      '2026-03-20T09:30:00Z',
    )
    const proxyLoaded = req({ origin: 'agent_proxy', context_id: 'mine' }, '2026-03-20T09:40:00Z')
    const directLoaded = req({ origin: 'direct_chat', context_id: 'mine' }, '2026-03-20T09:50:00Z')
    const maf = req({ origin: 'maf' }, '2026-03-20T10:00:00Z')
    const got = matchPending(
      [indexed, loaded, notLoaded, proxyLoaded, directLoaded, maf],
      new Set(['mine']),
      new Map([[indexed.id, 'tool-chat']]),
      true,
    )
    expect(got.chats.map((c) => [c.sessionId, c.requests.map((r) => r.id)])).toEqual([
      ['tool-chat', [indexed.id]],
      ['mine', [loaded.id, directLoaded.id]],
    ])
    expect(got.dropped).toBe(3)
    expect(got.outside).toBe(0)
  })

  it('one row per chat, oldest request first; skips anything not pending', () => {
    const late = req({ origin: 'direct_chat', chat_session_id: 'a' }, '2026-03-20T12:00:00Z')
    const early = req({ origin: 'direct_chat', chat_session_id: 'a' }, '2026-03-20T09:00:00Z')
    const other = req({ origin: 'direct_chat', chat_session_id: 'b' }, '2026-03-20T10:00:00Z')
    const done = {
      ...req({ origin: 'direct_chat', chat_session_id: 'c' }),
      status: 'resolved' as const,
    }
    const got = matchPending([late, other, early, done], new Set(), new Map(), false)
    expect(got.chats.map((c) => [c.sessionId, c.requests.length])).toEqual([
      ['a', 2],
      ['b', 1],
    ])
    expect(got.chats[0]!.requests[0]!.id).toBe(early.id)
  })

  it('a preview is one plain line: Markdown and HTML never render (E16)', () => {
    expect(
      requestPreview({
        question: {
          message: 'Run **rm -rf** <img src=x onerror=alert(1)> `now`?\n\nyes',
        } as HitlDto['question'],
      }),
    ).toBe('Run rm -rf now? yes')
  })
})

// ─── The poller (test 7, E6) ──────────────────────────────────────────────────

describe('the pending poller (test 7)', () => {
  const polls = () => {
    const at: number[] = []
    server.events.on('request:start', ({ request }) => {
      if (new URL(request.url).pathname === '/api/hitl/pending') at.push(performance.now())
    })
    return at
  }

  it('backs off 30 → 60 → 120 s after failures (1×, 2×, 4× the poll interval), and a success resets it', () => {
    expect([0, 1, 2, 3, 9].map(pendingInterval)).toEqual([
      30_000, 60_000, 120_000, 120_000, 120_000,
    ])
  })

  /** The one pending query and the wait its refetchInterval asks for now. */
  const pendingQuery = (queryClient: QueryClient) => {
    const all = queryClient.getQueryCache().findAll({ queryKey: ['chat', 'pending'] })
    expect(all).toHaveLength(1)
    const q = all[0]!
    const interval = q.observers[0]?.options.refetchInterval
    return { q, next: () => (typeof interval === 'function' ? interval(q as never) : interval) }
  }

  it('one poll loop for the chat layout, however often the rail remounts; nothing while the tab is hidden', async () => {
    const poll = tuning.PENDING_POLL_MS
    tuning.PENDING_POLL_MS = 60
    const at = polls()
    try {
      const { router, queryClient } = renderApp(`/chat/${C6}`)
      await screen.findByRole('navigation', { name: 'Chats' })
      // /chat and /chat/$id are sibling routes: each move remounts the layout and its rail. One query key, one loop.
      await router.navigate({ to: '/chat', search: {} as never })
      await screen.findByTestId('new-chat')
      await router.navigate({
        to: '/chat/$sessionId',
        params: { sessionId: C6 },
        search: {} as never,
      })
      await screen.findByRole('navigation', { name: 'Chats' })
      // Each observer keeps its own interval timer, so one observer is one loop.
      await waitFor(() => expect(pendingQuery(queryClient).q.getObserversCount()).toBe(1))
      const before = at.length
      await waitFor(() => expect(at.length).toBeGreaterThan(before), { timeout: 2000 })
      Object.defineProperty(document, 'visibilityState', {
        configurable: true,
        get: () => 'hidden',
      })
      document.dispatchEvent(new Event('visibilitychange'))
      const hiddenAt = at.length
      // Proves an absence (no polling while hidden) over several poll intervals.
      await new Promise((r) => setTimeout(r, 300))
      expect(at.length - hiddenAt).toBeLessThanOrEqual(1)
    } finally {
      tuning.PENDING_POLL_MS = poll
      Object.defineProperty(document, 'visibilityState', {
        configurable: true,
        get: () => 'visible',
      })
      document.dispatchEvent(new Event('visibilitychange'))
      server.events.removeAllListeners()
    }
  })

  it('failures space the polls out, and the first success brings the interval back', async () => {
    let fail = true
    server.use(
      http.get('/api/hitl/pending', () =>
        fail
          ? HttpResponse.json({ error: 'x', correlation_id: 'c' }, { status: 500 })
          : HttpResponse.json({ data: [] }),
      ),
    )
    const { queryClient } = renderApp('/chat')
    await screen.findByRole('navigation', { name: 'Chats' })
    const refetch = async () => {
      await queryClient.refetchQueries({ queryKey: ['chat', 'pending'] }).catch(() => undefined)
      return pendingQuery(queryClient).next()
    }
    await waitFor(() => expect(pendingQuery(queryClient).q.state.status).toBe('error'))
    // Each poll's own failure sets the next wait: 2× then 4× the interval, then it stays at 4×.
    expect(pendingQuery(queryClient).next()).toBe(pendingInterval(1))
    expect(await refetch()).toBe(pendingInterval(2))
    expect(await refetch()).toBe(pendingInterval(3))
    fail = false
    expect(await refetch()).toBe(pendingInterval(0))
  })
})

// ─── The Waiting view (test 14) ───────────────────────────────────────────────

const rail = () => screen.getByRole('navigation', { name: 'Chats' })
const waitingRows = () => within(rail()).queryAllByTestId('waiting-row')

async function openWaiting(label: RegExp | string = /^Waiting/) {
  const r = await screen.findByRole('navigation', { name: 'Chats' })
  await userEvent.click(await within(r).findByRole('radio', { name: label }))
}

describe('the Waiting view (test 14)', () => {
  it("as the superuser admin: three chats, oldest first, with a muted line for requests it can't prove are its own", async () => {
    configureChatMock({ waiting: true })
    renderApp('/chat')
    await openWaiting('Waiting (3)')
    await waitFor(() => expect(waitingRows()).toHaveLength(3))
    expect(waitingRows().map((r) => r.getAttribute('href'))).toEqual([
      `/chat/${C8}`,
      `/chat/${C7}`,
      `/chat/${C6}`,
    ])
    expect(waitingRows()[0]).toHaveTextContent('Rotate the API keys')
    expect(waitingRows()[0]).toHaveTextContent('2 requests')
    expect(waitingRows()[2]).toHaveTextContent('Migrate the billing tables first?')
    expect(within(rail()).getByTestId('superuser-dropped')).toHaveTextContent(
      "Requests from other people's chats aren't shown here.",
    )
    expect(within(rail()).queryByTestId('outside-chat')).toBeNull()
  })

  it("a normal user sees the outside-Chat count; opening the tool request's chat places it through the index", async () => {
    configureMocks({ superuser: false })
    configureChatMock({ waiting: true })
    const { router } = renderApp('/chat')
    await openWaiting('Waiting (3)')
    expect(await within(rail()).findByTestId('outside-chat')).toHaveTextContent(
      "2 requests couldn't be linked to a chat.",
    )
    expect(within(rail()).queryByTestId('superuser-dropped')).toBeNull()
    await router.navigate({
      to: '/chat/$sessionId',
      params: { sessionId: C7 },
      search: {} as never,
    })
    await waitFor(() =>
      expect(within(rail()).getByTestId('outside-chat')).toHaveTextContent(
        "1 request couldn't be linked to a chat.",
      ),
    )
    expect(waitingRows().find((r) => r.getAttribute('href') === `/chat/${C7}`)).toHaveTextContent(
      '2 requests',
    )
  })

  it('a chat not loaded in the rail is "Untitled chat" for a normal user; a superuser drops it', async () => {
    configureMocks({ superuser: false })
    configureChatMock({ waiting: true, manyChats: true })
    renderApp('/chat')
    await openWaiting('Waiting (4+)')
    await waitFor(() => expect(waitingRows()).toHaveLength(4))
    expect(waitingRows()[0]).toHaveTextContent('Untitled chat')
    expect(waitingRows()[0]).toHaveTextContent('Ship the old export too?')
    document.body.innerHTML = ''
    clearChatRegistry()
    configureMocks({ superuser: true })
    renderApp('/chat')
    await openWaiting('Waiting (3+)')
    await waitFor(() => expect(waitingRows()).toHaveLength(3))
    expect(waitingRows().some((r) => /Untitled chat/.test(r.textContent ?? ''))).toBe(false)
  })

  it('the radiogroup works by keyboard; filters keep the query; no match says so', async () => {
    configureChatMock({ waiting: true })
    renderApp('/chat')
    const r = await screen.findByRole('navigation', { name: 'Chats' })
    const chats = await within(r).findByRole('radio', { name: 'Chats' })
    await within(r).findByRole('radio', { name: 'Waiting (3)' })
    chats.focus()
    await userEvent.keyboard('{ArrowRight}')
    expect(within(r).getByRole('radio', { name: 'Waiting (3)' })).toHaveAttribute(
      'aria-checked',
      'true',
    )
    await userEvent.click(within(r).getByRole('button', { name: 'Search chats' }))
    await userEvent.type(within(r).getByLabelText('Filter chats'), 'nothing-like-this')
    expect(within(r).getByText("No waiting chats match 'nothing-like-this'.")).toBeInTheDocument()
    await userEvent.click(within(r).getByRole('radio', { name: 'Chats' }))
    expect(within(r).getByLabelText('Filter chats')).toHaveValue('nothing-like-this')
  })

  it('rows in Chats carry the waiting pill first in the slot, and say so', async () => {
    configureChatMock({ waiting: true })
    renderApp('/chat')
    const r = await screen.findByRole('navigation', { name: 'Chats' })
    await within(r).findByRole('radio', { name: 'Waiting (3)' })
    const row = within(r)
      .getAllByTestId('rail-row')
      .find((x) => x.getAttribute('href') === `/chat/${C8}`)!
    expect(within(row).getByTestId('row-indicator').querySelector('[data-mark]')).toHaveAttribute(
      'data-mark',
      'waiting',
    )
    expect(within(row).getByTestId('row-indicator')).toHaveTextContent('2')
    expect(row).toHaveAccessibleName(/waiting for you/)
  })

  it('no pending requests: the zero state; a cold failure: count-less Waiting with the error and Retry', async () => {
    configureChatMock({ waiting: false })
    configureMocks({ chatVariants: ['pending-fail'] })
    renderApp('/chat')
    await openWaiting('Waiting')
    const error = await within(rail()).findByTestId('waiting-error')
    expect(error).toHaveTextContent("Couldn't load waiting requests")
    configureMocks({ chatVariants: [] })
    await userEvent.click(within(error).getByRole('button', { name: /Retry/ }))
    expect(
      await within(rail()).findByText('No waiting requests in your loaded chats.'),
    ).toBeInTheDocument()
  })

  it('a failed poll after a success keeps the rows and says when they were last checked', async () => {
    const poll = tuning.PENDING_POLL_MS
    tuning.PENDING_POLL_MS = 60
    try {
      configureChatMock({ waiting: true })
      configureMocks({ chatVariants: ['waiting', 'pending-flaky'] })
      renderApp('/chat')
      await openWaiting('Waiting (3)')
      expect(
        await within(rail()).findByTestId('waiting-stale', {}, { timeout: 2000 }),
      ).toHaveTextContent(/^Last checked /)
      expect(waitingRows()).toHaveLength(3)
    } finally {
      tuning.PENDING_POLL_MS = poll
    }
  })

  it('opening a routed request from Waiting focuses its card', async () => {
    configureChatMock({ waiting: true })
    const { router } = renderApp('/chat')
    await openWaiting('Waiting (3)')
    const row = await waitFor(() =>
      waitingRows().find((r) => r.getAttribute('href') === `/chat/${C6}`)!,
    )
    await userEvent.click(row)
    await waitFor(() => expect(router.state.location.pathname).toBe(`/chat/${C6}`))
    const card = await screen.findByTestId('request-card')
    await waitFor(() => expect(card).toHaveFocus())
    expect(card.id).toMatch(/^request-/)
  })

  it("when a focused row's request is answered, focus moves to the next row", async () => {
    configureChatMock({ waiting: true })
    const { queryClient } = renderApp('/chat')
    await openWaiting('Waiting (3)')
    await waitFor(() => expect(waitingRows()).toHaveLength(3))
    const [first, second] = waitingRows()
    first!.focus()
    const pending = (await (await fetch(new URL('/api/hitl/pending', location.origin))).json()) as {
      data: HitlDto[]
    }
    const c8 = pending.data.filter((h) => h.execution.chat_session_id === C8)
    for (const h of c8)
      await fetch(new URL(`/api/hitl/${h.id}/resolve`, location.origin), {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ answer: 'Yes' }),
      })
    await queryClient.invalidateQueries({ queryKey: ['chat', 'pending'] })
    await waitFor(() => expect(waitingRows()).toHaveLength(2))
    expect(second).toHaveFocus()
  })

  it('a request answered before its chat opens: the note says so, and a later move in the chat clears it', async () => {
    configureChatMock({ waiting: true })
    const { router } = renderApp('/chat')
    await openWaiting('Waiting (3)')
    const row = await waitFor(() =>
      waitingRows().find((r) => r.getAttribute('href') === `/chat/${C8}`)!,
    )
    // Answered elsewhere; the rail hasn't polled since.
    const pending = (await (await fetch(new URL('/api/hitl/pending', location.origin))).json()) as {
      data: HitlDto[]
    }
    for (const h of pending.data.filter((p) => p.execution.chat_session_id === C8))
      await fetch(new URL(`/api/hitl/${h.id}/resolve`, location.origin), {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ answer: 'Yes' }),
      })
    await userEvent.click(row)
    expect(await screen.findByTestId('request-gone')).toHaveTextContent(copy.requestGone)
    await router.navigate({
      to: '/chat/$sessionId',
      params: { sessionId: C8 },
      search: { debug: 'turn' } as never,
    })
    await waitFor(() => expect(screen.queryByTestId('request-gone')).toBeNull())
  })

  it("Go to request focuses the oldest pending request's card, found by its request id (E5)", async () => {
    configureChatMock({ waiting: true })
    renderApp(`/chat/${C7}`)
    await screen.findByTestId('request-card')
    const page = (await (
      await fetch(new URL(`/api/chat/sessions/${C7}/messages?limit=50`, location.origin))
    ).json()) as { hitl: HitlDto[] }
    const oldest = page.hitl
      .filter((h) => h.status === 'pending')
      .sort((a, b) => a.created_at.localeCompare(b.created_at))[0]!
    await userEvent.click(screen.getByRole('button', { name: copy.goToRequest }))
    await waitFor(() => expect(document.getElementById(`request-${oldest.id}`)).toHaveFocus())
  })

  it('a malformed pending row never breaks the page: a non-string question has no preview, rows without an id or execution go', async () => {
    server.use(
      http.get('/api/hitl/pending', () =>
        HttpResponse.json({
          data: [
            {
              ...req({ origin: 'direct_chat', chat_session_id: C6 }),
              question: { message: { nested: true } },
            },
            { ...req({ origin: 'direct_chat', chat_session_id: C6 }), question: 42 },
            { id: 'no-execution', status: 'pending', created_at: '2026-03-20T10:00:00Z' },
            null,
            'junk',
          ],
        }),
      ),
    )
    configureChatMock({ waiting: true })
    renderApp('/chat')
    await screen.findByRole('navigation', { name: 'Chats' })
    await openWaiting(/^Waiting/)
    await waitFor(() => expect(waitingRows()).toHaveLength(1))
    expect(waitingRows()[0]).toHaveTextContent('2 requests')
  })

  it('unwrapPending keeps well-formed rows and defuses the question', () => {
    const ok = req({ origin: 'direct_chat', chat_session_id: 'c' })
    expect(unwrapPending({ data: [ok] })).toEqual([ok])
    expect(
      unwrapPending({ data: [{ ...ok, question: { message: 7, options: ['a'] } }] })[0]!.question,
    ).toEqual({ message: undefined, options: ['a'] })
    expect(unwrapPending({ data: [{ ...ok, question: 'text' }] })[0]!.question).toBeNull()
    expect(
      unwrapPending({
        data: [
          { ...ok, execution: null },
          { ...ok, created_at: 5 },
          { ...ok, id: 1 },
        ],
      }),
    ).toEqual([])
    expect(unwrapPending(null)).toEqual([])
    expect(unwrapPending({ data: 'x' })).toEqual([])
  })

  it('the phone trigger names the waiting chats', async () => {
    vi.stubGlobal('matchMedia', (query: string) => ({
      matches: false,
      media: query,
      addEventListener: () => {},
      removeEventListener: () => {},
    }))
    configureChatMock({ waiting: true })
    renderApp('/chat')
    expect(await screen.findByRole('button', { name: 'Chats, 3 waiting' })).toBeInTheDocument()
  })

  it('the trigger name covers its four combinations', () => {
    expect([
      copy.chatsTrigger(0, 0),
      copy.chatsTrigger(2, 0),
      copy.chatsTrigger(0, 1),
      copy.chatsTrigger(2, 1),
    ]).toEqual(['Chats', 'Chats, 2 waiting', 'Chats, 1 new reply', 'Chats, 2 waiting, 1 new reply'])
  })
})

describe('axe (test 16, M4 screens)', () => {
  const check = async () => {
    const result = await axe.run(document.body, {
      rules: { 'color-contrast': { enabled: false }, region: { enabled: false } },
    })
    expect(
      result.violations.map(
        (v) => `${v.id}: ${v.nodes.map((node) => node.target.join(' ')).join(', ')}`,
      ),
    ).toEqual([])
  }
  it('the Waiting list, and a chat with a pending request', async () => {
    configureChatMock({ waiting: true })
    const { router } = renderApp('/chat')
    await openWaiting('Waiting (3)')
    await waitFor(() => expect(waitingRows()).toHaveLength(3))
    await check()
    await router.navigate({
      to: '/chat/$sessionId',
      params: { sessionId: C6 },
      search: {} as never,
    })
    await screen.findByTestId('request-card')
    await check()
  })
})
