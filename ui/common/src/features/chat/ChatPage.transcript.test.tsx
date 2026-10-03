/**
 * The transcript of a direct chat: read-only notes, older pages and Jump to latest, file chips,
 * turn order and tool chips once a reply is saved, and the live turn's details (debug frames, the
 * Waiting timer, the pending poll).
 */
import { fireEvent, screen, waitFor, within } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { http, HttpResponse } from 'msw'
import { afterEach, describe, expect, it, vi } from 'vitest'
import { configureChatMock } from '@/mocks/chatStore'
import { renderApp } from '@/test/renderApp'
import { server } from '@/test/setup'
import { clearDrafts } from './drafts'
import { clearChatRegistry } from './registry'
import type { ChatMessage, ChatSessionRow } from './types'

const DONE = '5eedc000-0000-4000-8000-00000000c001'
const FAKE = '5eedc000-0000-4000-8000-00000000f001'

interface MockAgent {
  id: string
  name: string
  status: string
  tags: string[]
}

async function allAgents(): Promise<MockAgent[]> {
  const out: MockAgent[] = []
  for (let offset = 0; offset < 1000; offset += 100) {
    const rows = (await (
      await fetch(new URL(`/api/agents?limit=100&offset=${offset}`, location.origin))
    ).json()) as MockAgent[]
    out.push(...rows)
    if (rows.length < 100) break
  }
  return out
}
const running = async () =>
  (await allAgents()).filter((a) => a.status === 'running' && !a.tags.includes('coding-agent'))

const iso = (hoursAgo: number) => new Date(Date.now() - hoursAgo * 3_600_000).toISOString()
const message = (
  id: string,
  role: ChatMessage['role'],
  content: string,
  hoursAgo: number,
  extra: Partial<ChatMessage> = {},
): ChatMessage => ({ id, session_id: FAKE, role, content, timestamp: iso(hoursAgo), ...extra })
const sessionRow = (over: Partial<ChatSessionRow>): ChatSessionRow =>
  ({
    session_id: FAKE,
    user_id: 'u',
    agent_id: null,
    agent_url: null,
    title: 'Fake chat',
    created_at: iso(5),
    updated_at: iso(4),
    agent_name: null,
    is_coding_agent: false,
    ...over,
  }) as ChatSessionRow

/** Serve `rows` as the chat list (one page) and `pages(prevCursor)` as FAKE's history. */
function fakeChat(
  rows: ChatSessionRow[],
  pages: (prev: string | null) => Response | Record<string, unknown>,
) {
  server.use(
    http.get('/api/chat/sessions', () =>
      HttpResponse.json({ data: rows, has_more: false, next_cursor: null, prev_cursor: null }),
    ),
    http.get(`/api/chat/sessions/${FAKE}/messages`, ({ request }) => {
      const r = pages(new URL(request.url).searchParams.get('prev_cursor'))
      return r instanceof Response ? r : HttpResponse.json(r)
    }),
  )
}

/** The saved (history) rendering has its own Copy button next to the reply. */
const savedReply = (text: string | RegExp) =>
  waitFor(
    () => {
      const reply = screen.getByText(text)
      expect(
        within(reply.closest('div.space-y-2') as HTMLElement).getByRole('button', {
          name: 'Copy',
        }),
      ).toBeInTheDocument()
    },
    { timeout: 5000 },
  )

afterEach(() => {
  vi.useRealTimers()
  clearChatRegistry()
  clearDrafts()
})

describe('read-only chats', () => {
  it('a recorded harness chat says so, links to Harnesses and has no composer', async () => {
    fakeChat([sessionRow({ is_coding_agent: true, agent_id: 'harness-1' })], () => ({
      data: [message('u1', 'user', 'refactor', 3)],
      has_more: false,
      hitl: [],
    }))
    renderApp(`/chat/${FAKE}`)
    expect(await screen.findByText('Recorded from a coding harness.')).toBeInTheDocument()
    expect(screen.getByRole('link', { name: 'View in Harnesses →' })).toHaveAttribute(
      'href',
      '/harnesses',
    )
    expect(screen.getAllByText('Recorded').length).toBeGreaterThan(0)
    expect(screen.queryByLabelText(/^Message/)).toBeNull()
  })

  it('a chat whose agent was removed says so and has no composer', async () => {
    fakeChat(
      [sessionRow({ agent_id: '5eedc000-0000-4000-8000-0000000dead0', agent_name: 'gone-agent' })],
      () => ({ data: [message('u1', 'user', 'hi', 3)], has_more: false, hitl: [] }),
    )
    renderApp(`/chat/${FAKE}`)
    expect(await screen.findByText("This chat's agent was removed.")).toBeInTheDocument()
    expect(screen.queryByLabelText(/^Message/)).toBeNull()
  })
})

describe('transcript', () => {
  it('Load older: a failed page says so, Retry loads it above the newer turns', async () => {
    const [agent] = await running()
    let olderCalls = 0
    fakeChat([sessionRow({ agent_id: agent!.id, agent_name: agent!.name })], (prev) => {
      if (!prev)
        return {
          data: [
            message('u2', 'user', 'newer question', 2),
            message('a2', 'assistant', 'newer answer', 1.9),
          ],
          has_more: true,
          prev_cursor: 'u2',
          next_cursor: null,
          hitl: [],
        }
      if (++olderCalls === 1) return new HttpResponse('boom', { status: 500 })
      return {
        data: [
          message('u1', 'user', 'older question', 4),
          message('a1', 'assistant', 'older answer', 3.9),
        ],
        has_more: false,
        prev_cursor: 'u1',
        next_cursor: null,
        hitl: [],
      }
    })
    renderApp(`/chat/${FAKE}`)
    await screen.findByText('newer answer')
    await userEvent.click(screen.getByRole('button', { name: 'Load older messages' }))
    expect(await screen.findByText("Couldn't load older messages.")).toBeInTheDocument()
    await userEvent.click(
      within(screen.getByTestId('transcript')).getByRole('button', { name: /Retry/ }),
    )
    const older = await screen.findByText('older answer')
    expect(
      older.compareDocumentPosition(screen.getByText('newer answer')) &
        Node.DOCUMENT_POSITION_FOLLOWING,
    ).toBeTruthy()
    expect(screen.queryByRole('button', { name: 'Load older messages' })).toBeNull()
  })

  it('scrolling up offers Jump to latest, which scrolls back and hides itself', async () => {
    const [agent] = await running()
    fakeChat([sessionRow({ agent_id: agent!.id, agent_name: agent!.name })], () => ({
      data: [message('u1', 'user', 'q', 2), message('a1', 'assistant', 'answer text', 1.9)],
      has_more: false,
      hitl: [],
    }))
    renderApp(`/chat/${FAKE}`)
    await screen.findByText('answer text')
    const el = screen.getByTestId('transcript')
    Object.defineProperty(el, 'scrollHeight', { value: 2000, configurable: true })
    Object.defineProperty(el, 'clientHeight', { value: 500, configurable: true })
    const scrollTo = vi.fn()
    el.scrollTo = scrollTo as never
    el.scrollTop = 0
    fireEvent.scroll(el)
    await userEvent.click(await screen.findByRole('button', { name: 'Jump to latest' }))
    expect(scrollTo).toHaveBeenCalledWith({ top: 2000, behavior: 'smooth' })
    expect(screen.queryByRole('button', { name: 'Jump to latest' })).toBeNull()
  })

  it('a saved reply shows its files as download chips', async () => {
    const [agent] = await running()
    fakeChat([sessionRow({ agent_id: agent!.id, agent_name: agent!.name })], () => ({
      data: [
        message('u1', 'user', 'q', 2),
        message('a1', 'assistant', 'here you go', 1.9, {
          file_parts: [{ id: 'f/1', name: 'report.pdf', size: 2048 }],
          has_file_parts: true,
        }),
      ],
      has_more: false,
      hitl: [],
    }))
    renderApp(`/chat/${FAKE}`)
    const chip = await screen.findByRole('link', { name: /report\.pdf · 2 KB/ })
    expect(chip).toHaveAttribute('href', '/api/chat/files/f%2F1/download')
    expect(chip).toHaveAttribute('download')
  })
})

describe('a saved turn', () => {
  it('puts the "Answered" receipt before the resumed reply (ISSUE-003)', async () => {
    configureChatMock({ scenario: 'hitl-options' })
    renderApp(`/chat/${DONE}`)
    await userEvent.type(await screen.findByLabelText(/^Message /), 'Deploy it{Enter}')
    const card = await screen.findByTestId('request-card')
    await userEvent.click(
      within(card)
        .getAllByRole('button')
        .find((b) => !/Dismiss/.test(b.textContent ?? ''))!,
    )
    // Wait for the saved reply (history), not the live stream's copy.
    await savedReply(/^Thanks\. Continuing with/)
    const reply = screen.getByText(/^Thanks\. Continuing with/)
    const receipt = screen.getByTestId('request-receipt')
    // DOCUMENT_POSITION_FOLLOWING: the reply comes after the receipt.
    expect(receipt.compareDocumentPosition(reply) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy()
  })

  it('a saved reply keeps its tools chip (ISSUE-001, UI side)', async () => {
    configureChatMock({ scenario: 'direct-steps' })
    renderApp(`/chat/${DONE}`)
    await userEvent.type(await screen.findByLabelText(/^Message /), 'go{Enter}')
    await savedReply('Found three matches.')
    expect(screen.getByRole('button', { name: /^1 tool/ })).toBeInTheDocument()
  })
})

describe('live turn details', () => {
  it('?debug=turn shows the raw frames with the phase and count', async () => {
    configureChatMock({ scenario: 'failed' })
    renderApp(`/chat/${DONE}?debug=turn`)
    await userEvent.type(await screen.findByLabelText(/^Message /), 'go{Enter}')
    expect(await screen.findByText(/Raw frames \(debug\) · error · [1-9]\d*/)).toBeInTheDocument()
  })

  it('a slow agent shows the elapsed seconds, then the still-working note', async () => {
    server.use(
      http.post(
        '/api/orchestrator/a2a',
        () =>
          new HttpResponse(new ReadableStream({ start() {} }), {
            headers: { 'Content-Type': 'text/event-stream' },
          }),
      ),
    )
    vi.useFakeTimers({ shouldAdvanceTime: true })
    const user = userEvent.setup({ advanceTimers: vi.advanceTimersByTime })
    renderApp(`/chat/${DONE}`)
    await user.type(await screen.findByLabelText(/^Message /), 'take your time{Enter}')
    expect(await screen.findByText(/^Waiting for /)).toBeInTheDocument()
    expect(screen.queryByText(/^\d+s$/)).toBeNull()
    await vi.advanceTimersByTimeAsync(9_000)
    expect(await screen.findByText(/^\d+s$/)).toBeInTheDocument()
    expect(screen.queryByText(/^Still working\./)).toBeNull()
    await vi.advanceTimersByTimeAsync(22_000)
    expect(await screen.findByText(/^Still working\./)).toBeInTheDocument()
  })

  it('a pending request polls history every PENDING_POLL_MS', async () => {
    configureChatMock({ scenario: 'hitl-options' })
    vi.useFakeTimers({ shouldAdvanceTime: true })
    const user = userEvent.setup({ advanceTimers: vi.advanceTimersByTime })
    let gets = 0
    server.events.on('request:start', ({ request }) => {
      if (request.method === 'GET' && request.url.includes(`/sessions/${DONE}/messages`)) gets++
    })
    try {
      renderApp(`/chat/${DONE}`)
      await user.type(await screen.findByLabelText(/^Message /), 'Deploy it{Enter}')
      await screen.findByTestId('request-card')
      await waitFor(() => expect(screen.getAllByTestId('request-card').length).toBeGreaterThan(0))
      await vi.advanceTimersByTimeAsync(2_000)
      const before = gets
      await vi.advanceTimersByTimeAsync(30_000)
      await waitFor(() => expect(gets).toBeGreaterThan(before))
    } finally {
      server.events.removeAllListeners()
    }
  })
})
