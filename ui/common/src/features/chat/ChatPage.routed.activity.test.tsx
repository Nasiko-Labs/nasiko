/**
 * What a routed reply shows about the agents it used (v1b §5.7, §5.8, §5.11, NC-2): attribution
 * from the flows fallback (its cap, only on screen, its failure), plain text for model strings,
 * one announcement per outcome, and the `?debug=turn` metric. Every test asserts the client never
 * POSTed an assistant row.
 */
import { screen, waitFor, within } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { http, HttpResponse } from 'msw'
import { afterEach, beforeEach, describe, expect, it } from 'vitest'
import {
  artifact,
  status,
  subStatus,
  toolCall,
  toolResult,
  traceMeta,
  usageMeta,
  type MockFrame,
} from '@/mocks/chat'
import { configureChatMock } from '@/mocks/chatStore'
import { installIntersectionObserver } from '@/test/intersection'
import { renderApp } from '@/test/renderApp'
import { recordRequestBodies, server } from '@/test/setup'
import { clearDrafts } from './drafts'
import { clearChatRegistry } from './registry'
import type { ChatMessage, ChatSessionRow } from './types'

const CHAT = '5eedc000-0000-4000-8000-0000000fa001'
const XSS = '<img src=x onerror="window.__pwned=1">'

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
const msg = (
  id: string,
  role: 'user' | 'assistant',
  content: string,
  i: number,
  traceId: string | null = null,
): ChatMessage =>
  ({
    id,
    session_id: CHAT,
    role,
    content,
    timestamp: new Date(Date.parse('2026-09-20T10:00:00Z') + i * 1000).toISOString(),
    trace_id: traceId,
  }) as ChatMessage

describe('flows fallback (§5.8, E-A6)', () => {
  function routedChat(replies: number) {
    const messages = Array.from({ length: replies }, (_, i) => [
      msg(`u${i}`, 'user', `q ${i}`, 2 * i),
      msg(`a${i}`, 'assistant', `answer ${i}`, 2 * i + 1, `5eedf${String(i).padStart(27, '0')}`),
    ]).flat()
    server.use(
      http.get('/api/chat/sessions', () =>
        HttpResponse.json({ data: [row()], has_more: false, next_cursor: null, prev_cursor: null }),
      ),
      http.get(`/api/chat/sessions/${CHAT}/messages`, () =>
        HttpResponse.json({
          data: messages,
          has_more: false,
          next_cursor: null,
          prev_cursor: messages[0]!.id,
          hitl: [],
        }),
      ),
    )
  }

  it('50 routed replies start at most 4 flows requests at once, and only once on screen', async () => {
    const io = installIntersectionObserver()
    try {
      routedChat(50)
      let inFlight = 0
      let peak = 0
      let total = 0
      server.use(
        http.get('/api/flows/:id', async () => {
          inFlight++
          total++
          peak = Math.max(peak, inFlight)
          // Server latency, so the requests overlap: the scenario, not a wait for the page.
          await new Promise((r) => setTimeout(r, 20))
          inFlight--
          return HttpResponse.json({
            flow: {},
            steps: [
              {
                step_order: 1,
                depth: 1,
                agent_name: 'seed-agent',
                caller_agent_name: 'orchestrator',
                status: 'completed',
              },
            ],
          })
        }),
      )
      renderApp(`/chat/${CHAT}`)
      await screen.findByText('answer 49')
      // Proves an absence (no flows request before the rows are on screen).
      await new Promise((r) => setTimeout(r, 50))
      expect(total).toBe(0)
      io.showAll()
      await waitFor(
        () =>
          expect(
            screen.getAllByText('Answered by the Orchestrator, using seed-agent'),
          ).toHaveLength(50),
        { timeout: 5000 },
      )
      expect(total).toBe(50)
      expect(peak).toBeLessThanOrEqual(4)
      expect(peak).toBeGreaterThan(0)
    } finally {
      io.restore()
    }
  }, 15_000)

  it('a flows failure says Agent details unavailable with Retry, and attribution stays plain', async () => {
    routedChat(1)
    server.use(
      http.get('/api/flows/:id', () => new HttpResponse('internal error', { status: 500 })),
    )
    renderApp(`/chat/${CHAT}`)
    expect(await screen.findByText('Agent details unavailable')).toBeInTheDocument()
    expect(screen.getByText('Answered by the Orchestrator')).toBeInTheDocument()
    server.use(
      http.get('/api/flows/:id', () =>
        HttpResponse.json({
          flow: {},
          steps: [
            {
              step_order: 1,
              depth: 1,
              agent_name: 'seed-agent',
              caller_agent_name: 'orchestrator',
              status: 'completed',
            },
          ],
        }),
      ),
    )
    await userEvent.click(screen.getByRole('button', { name: 'Retry' }))
    expect(
      await screen.findByText('Answered by the Orchestrator, using seed-agent'),
    ).toBeInTheDocument()
  })
})

describe('plain text and announcements (§5.11, §6)', () => {
  it('HTML in ?agent=, a title, a sub_status, an excerpt and a request question renders inert', async () => {
    const w = window as unknown as { __pwned?: number }
    renderApp(`/chat?agent=${encodeURIComponent(XSS)}`)
    expect(
      await screen.findByText(
        new RegExp(`'${XSS.replace(/[.*+?^${}()|[\]\\]/g, '\\$&')}' isn't an agent`),
      ),
    ).toBeInTheDocument()
    document.body.innerHTML = ''
    const frames: MockFrame[] = [
      { data: traceMeta('5eedf0000000000000000000000000x1') },
      { data: toolCall('seed-agent', 1) },
      { data: subStatus('seed-agent', XSS) },
      { data: toolResult('seed-agent', 1, true, XSS) },
      { data: artifact('ok', { lastChunk: true }) },
      { data: usageMeta({ duration_ms: 1 }) },
      { data: status('TASK_STATE_COMPLETED') },
    ]
    server.use(
      http.post('/api/orchestrator/a2a', async () => {
        const { sseResponse } = await import('@/mocks/chat')
        return sseResponse(frames)
      }),
    )
    const { router } = renderApp('/chat?auto=1')
    await userEvent.type(await screen.findByLabelText('Ask the Orchestrator'), `${XSS}{Enter}`)
    await waitFor(() => expect(router.state.location.pathname).toMatch(/^\/chat\/[0-9a-f-]{36}$/))
    await userEvent.click(await screen.findByRole('button', { name: /Activity · 1 agent/ }))
    const agentRow = within(screen.getByTestId('activity'))
      .getAllByRole('button')
      .find((b) => b.getAttribute('aria-expanded') === 'false')!
    await userEvent.click(agentRow)
    expect(screen.getByTestId('activity-excerpt').textContent).toContain('<img')
    expect(document.querySelector('img[src="x"]')).toBeNull()
    expect(w.__pwned).toBeUndefined()
  })

  it('the announcer speaks once for the outcome, never per chip', async () => {
    configureChatMock({ scenario: 'routed-multi-agent' })
    renderApp('/chat?auto=1')
    await userEvent.type(await screen.findByLabelText('Ask the Orchestrator'), 'go{Enter}')
    await screen.findByText(/^Answered by the Orchestrator, using /)
    const spoken = screen
      .getAllByRole('status')
      .map((s) => s.textContent?.trim())
      .filter(Boolean)
    expect(spoken).toContain('Reply from the Orchestrator complete')
    expect(spoken.join(' ')).not.toMatch(/Asking|Completed|agent/)
  })
})

describe('?debug=turn on a routed chat (NC-2)', () => {
  it('shows the routed metric over the settled replies', async () => {
    configureChatMock({ scenario: 'routed-plain' })
    renderApp('/chat?auto=1&debug=turn')
    await userEvent.type(await screen.findByLabelText('Ask the Orchestrator'), 'metric{Enter}')
    const line = await screen.findByTestId('routed-metric', {}, { timeout: 8000 })
    await waitFor(() => expect(screen.queryByTestId('routed-live')).toBeNull(), { timeout: 8000 })
    await waitFor(() =>
      expect(screen.getByTestId('routed-metric')).toHaveTextContent(
        'routed metric · 1 turn · 0 agents 0 · 1 agent 1 · 2+ agents 0 · empty 0',
      ),
    )
    void line
  })
})
