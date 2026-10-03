/**
 * The chat rail and page layout: a failed list, a filter with no match, paging (Load more, an
 * open chat past the first page), the mobile sheet, and Chat's full-height layout.
 */
import { screen, waitFor, within } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { http, HttpResponse } from 'msw'
import { afterEach, describe, expect, it, vi } from 'vitest'
import { renderApp } from '@/test/renderApp'
import { server } from '@/test/setup'
import { clearDrafts } from './drafts'
import { clearChatRegistry } from './registry'
import type { ChatMessage, ChatSessionRow } from './types'

const FAKE = '5eedc000-0000-4000-8000-00000000f001'

interface MockAgent {
  id: string
  name: string
  status: string
  tags: string[]
}

async function runningAgent(): Promise<MockAgent> {
  for (let offset = 0; offset < 1000; offset += 100) {
    const rows = (await (
      await fetch(new URL(`/api/agents?limit=100&offset=${offset}`, location.origin))
    ).json()) as MockAgent[]
    const hit = rows.find((a) => a.status === 'running' && !a.tags.includes('coding-agent'))
    if (hit) return hit
    if (rows.length < 100) break
  }
  throw new Error('no running agent in the mock seed')
}

const iso = (hoursAgo: number) => new Date(Date.now() - hoursAgo * 3_600_000).toISOString()
const message = (
  id: string,
  role: ChatMessage['role'],
  content: string,
  hoursAgo: number,
): ChatMessage => ({ id, session_id: FAKE, role, content, timestamp: iso(hoursAgo) })
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

afterEach(() => {
  vi.unstubAllGlobals()
  clearChatRegistry()
  clearDrafts()
})

describe('the rail list', () => {
  it('the rail says when the list fails, and when a filter matches nothing', async () => {
    server.use(
      http.get('/api/chat/sessions', () => new HttpResponse('boom', { status: 500 }), {
        once: true,
      }),
    )
    renderApp('/chat')
    const rail = await screen.findByRole('navigation', { name: 'Chats' })
    expect(await within(rail).findByText("Couldn't load your chats.")).toBeInTheDocument()
    await userEvent.click(within(rail).getByRole('button', { name: /Retry/ }))
    await waitFor(() => expect(within(rail).queryByText("Couldn't load your chats.")).toBeNull())
    // The filter sits behind the header's Search button (v1c §5.3).
    await userEvent.click(within(rail).getByRole('button', { name: 'Search chats' }))
    await userEvent.type(within(rail).getByLabelText('Filter chats'), 'zz-no-such-chat-zz')
    expect(within(rail).getByText("No loaded chats match 'zz-no-such-chat-zz'")).toBeInTheDocument()
  })
})

describe('rail paging', () => {
  const pagedList = (agent: MockAgent) => {
    const other = sessionRow({
      session_id: '5eedc000-0000-4000-8000-00000000f0aa',
      title: 'First page chat',
      agent_id: agent.id,
      agent_name: agent.name,
    })
    const target = sessionRow({
      title: 'Second page chat',
      agent_id: agent.id,
      agent_name: agent.name,
    })
    const cursors: (string | null)[] = []
    server.use(
      http.get('/api/chat/sessions', ({ request }) => {
        const cursor = new URL(request.url).searchParams.get('cursor')
        cursors.push(cursor)
        return HttpResponse.json(
          cursor
            ? { data: [target], has_more: false, next_cursor: null, prev_cursor: null }
            : { data: [other], has_more: true, next_cursor: 'p1', prev_cursor: null },
        )
      }),
      http.get(`/api/chat/sessions/${FAKE}/messages`, () =>
        HttpResponse.json({
          data: [message('u1', 'user', 'q', 2), message('a1', 'assistant', 'paged answer', 1.9)],
          has_more: false,
          hitl: [],
        }),
      ),
    )
    return cursors
  }

  it('an open chat whose row is on a later page finds it and resolves the agent', async () => {
    const cursors = pagedList(await runningAgent())
    renderApp(`/chat/${FAKE}`)
    expect(await screen.findByRole('button', { name: 'Second page chat' })).toBeInTheDocument()
    expect(await screen.findByLabelText(/^Message /)).toBeInTheDocument()
    expect(cursors).toContain('p1')
  })

  it('Load more fetches the next page of chats', async () => {
    pagedList(await runningAgent())
    renderApp('/chat')
    const rail = await screen.findByRole('navigation', { name: 'Chats' })
    await within(rail).findByText('First page chat')
    expect(within(rail).queryByText('Second page chat')).toBeNull()
    await userEvent.click(within(rail).getByRole('button', { name: 'Load more' }))
    expect(await within(rail).findByText('Second page chat')).toBeInTheDocument()
    expect(within(rail).queryByRole('button', { name: 'Load more' })).toBeNull()
  })
})

describe('layout', () => {
  it('on a narrow screen the rail is a sheet behind the Chats button', async () => {
    vi.stubGlobal('matchMedia', (query: string) => ({
      matches: false,
      media: query,
      addEventListener: () => {},
      removeEventListener: () => {},
    }))
    renderApp('/chat')
    await screen.findByRole('heading', { name: 'Start a chat' })
    expect(screen.queryByRole('navigation', { name: 'Chats' })).toBeNull()
    await userEvent.click(screen.getByRole('button', { name: 'Chats' }))
    const sheet = await screen.findByRole('dialog')
    expect(within(sheet).getByRole('navigation', { name: 'Chats' })).toBeInTheDocument()
    await userEvent.click(within(sheet).getByRole('link', { name: /New chat/ }))
    await waitFor(() => expect(screen.queryByRole('dialog')).toBeNull())
  })

  it('Chat fills the viewport under the nav; other pages keep the padded column', async () => {
    const { router } = renderApp('/chat')
    await screen.findByRole('heading', { name: 'Start a chat' })
    expect(document.querySelector('main')).toHaveClass('min-h-0', 'flex-1')
    expect(document.querySelector('main')!.parentElement).toHaveClass('h-dvh')
    expect(screen.getByRole('link', { name: 'Chat' })).toHaveAttribute('href', '/chat')
    await router.navigate({ to: '/agents' })
    await waitFor(() => expect(document.querySelector('main')).toHaveClass('max-w-page'))
    expect(document.querySelector('main')!.parentElement).toHaveClass('min-h-screen')
  })
})
