/**
 * Chat v1a flows end to end against the `chat` mock group (plan §12): choose (a shared name, a
 * stopped agent, an unknown ?mock=), direct send, streamed reply saved once, lost reply, read-only
 * routed chat, 404, requests and resume, save failure, delete guard.
 */
import { screen, waitFor, within } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { http, HttpResponse } from 'msw'
import { afterEach, describe, expect, it, vi } from 'vitest'
import { chatMockRows, configureChatMock } from '@/mocks/chatStore'
import { renderApp } from '@/test/renderApp'
import { server } from '@/test/setup'
import { clearDrafts } from './drafts'
import { clearChatRegistry } from './registry'

const DONE = '5eedc000-0000-4000-8000-00000000c001'
const LOST = '5eedc000-0000-4000-8000-00000000c002'
const ROUTED = '5eedc000-0000-4000-8000-00000000c003'

interface MockAgent {
  id: string
  name: string
  display_name?: string | null
  status: string
  tags: string[]
}

async function agentWhere(pick: (a: MockAgent) => boolean): Promise<MockAgent> {
  for (let offset = 0; offset < 1000; offset += 100) {
    const res = await fetch(new URL(`/api/agents?limit=100&offset=${offset}`, location.origin))
    const rows = (await res.json()) as MockAgent[]
    const hit = rows.find(pick)
    if (hit) return hit
    if (rows.length < 100) break
  }
  throw new Error('no such agent in the mock seed')
}

async function allAgents(): Promise<MockAgent[]> {
  const out: MockAgent[] = []
  for (let offset = 0; offset < 1000; offset += 100) {
    const res = await fetch(new URL(`/api/agents?limit=100&offset=${offset}`, location.origin))
    const rows = (await res.json()) as MockAgent[]
    out.push(...rows)
    if (rows.length < 100) break
  }
  return out
}

async function runningAgent(): Promise<MockAgent> {
  return agentWhere((a) => a.status === 'running' && !a.tags.includes('coding-agent'))
}

async function messages(sessionId: string) {
  const res = await fetch(new URL(`/api/chat/sessions/${sessionId}/messages`, location.origin))
  return ((await res.json()) as { data: { role: string; content: string }[] }).data
}

afterEach(() => {
  vi.useRealTimers()
  vi.unstubAllEnvs()
  vi.restoreAllMocks()
  clearChatRegistry()
  clearDrafts()
})

describe('/chat', () => {
  it('without an agent shows Start a chat, the Orchestrator and running agents to choose from (v1c §5.4)', async () => {
    renderApp('/chat')
    expect(await screen.findByRole('heading', { name: 'Start a chat' })).toBeInTheDocument()
    const list = await screen.findByRole('region', { name: 'Choose where to send' })
    const rows = await within(list).findAllByRole('button')
    expect(rows[0]).toHaveTextContent('Orchestrator')
    expect(rows.length).toBeGreaterThan(1)
    expect(rows.length).toBeLessThanOrEqual(6)
    // The composer is there, but Send waits for a target (UC1, DS3).
    expect(screen.getByLabelText('Message')).toBeInTheDocument()
    expect(screen.getByRole('button', { name: 'Send' })).toHaveAttribute('aria-disabled', 'true')
  })

  it('an unknown ?agent= shows a banner and never sends', async () => {
    renderApp('/chat?agent=no-such-agent')
    expect(
      await screen.findByText(/'no-such-agent' isn't an agent you can use/),
    ).toBeInTheDocument()
    await userEvent.type(screen.getByLabelText('Message'), 'hello')
    expect(screen.getByRole('button', { name: 'Send' })).toBeDisabled()
  })

  it('a name resolves to the agent', async () => {
    const agent = await runningAgent()
    renderApp(`/chat?agent=${encodeURIComponent(agent.name)}`)
    expect(
      await screen.findByRole('heading', {
        level: 1,
        name: agent.display_name?.trim() || agent.name,
      }),
    ).toBeInTheDocument()
  })

  it('a name several agents share lists them to pick from', async () => {
    const all = await allAgents()
    const [a, b] = all.filter((x) => x.status === 'running' && !x.tags.includes('coding-agent'))
    const shared = all.map((x) => (x.id === b!.id ? { ...x, name: a!.name } : x))
    server.use(
      http.get('/api/agents', ({ request }) => {
        const p = new URL(request.url).searchParams
        if (p.get('owner')) return undefined
        const offset = Number(p.get('offset') ?? 0)
        return HttpResponse.json(shared.slice(offset, offset + Number(p.get('limit') ?? 100)))
      }),
    )
    renderApp(`/chat?agent=${encodeURIComponent(a!.name)}`)
    expect(
      await screen.findByRole('heading', {
        name: `Several agents are called '${a!.name}'. Pick one:`,
      }),
    ).toBeInTheDocument()
    const hrefs = screen
      .getAllByRole('link')
      .map((l) => l.getAttribute('href') ?? '')
      .filter((h) => h.startsWith('/chat?agent='))
    expect(hrefs).toEqual(expect.arrayContaining([`/chat?agent=${a!.id}`, `/chat?agent=${b!.id}`]))
    expect(screen.queryByLabelText(/^Message/)).toBeNull()
  })

  it('a stopped agent shows why it cannot reply; Send stays aria-disabled and Enter says why (v1c E16)', async () => {
    const agent = await agentWhere(
      (a) => a.status !== 'running' && !a.tags.includes('coding-agent'),
    )
    const creates: string[] = []
    server.events.on('request:start', ({ request }) => {
      if (request.method === 'POST' && new URL(request.url).pathname === '/api/chat/sessions')
        creates.push(request.url)
    })
    renderApp(`/chat?agent=${agent.id}`)
    expect(await screen.findAllByText(/isn't running, so it can't reply\./)).not.toHaveLength(0)
    expect(screen.getByRole('link', { name: 'Open the agent' })).toBeInTheDocument()
    await userEvent.type(screen.getByLabelText(/^Message/), 'hello{Enter}')
    expect(screen.getByRole('button', { name: 'Send' })).toHaveAttribute('aria-disabled', 'true')
    expect(document.getElementById('chat-composer-hint')).toHaveTextContent(
      /isn't running\. Start it on Agents\./,
    )
    await waitFor(() =>
      expect(
        screen
          .getAllByRole('status')
          .map((el) => el.textContent)
          .join('|'),
      ).toMatch(/isn't running\. Start it on Agents\./),
    )
    expect(screen.getByLabelText(/^Message/)).toHaveValue('hello')
    expect(creates).toHaveLength(0)
    server.events.removeAllListeners()
  })

  it('an unknown ?mock= shows a dev banner listing the valid names, and nothing in production (§5.15, DX-7)', async () => {
    const warn = vi.spyOn(console, 'warn').mockImplementation(() => undefined)
    renderApp('/chat?mock=nope')
    const banner = await screen.findByTestId('unknown-scenario')
    expect(banner).toHaveTextContent(
      "Unknown mock scenario 'nope'. Using the default. Valid: direct-plain",
    )
    expect(banner).toHaveTextContent('routed-plain')
    expect(warn).toHaveBeenCalledWith(expect.stringContaining("unknown mock entry 'nope'"))
    document.body.innerHTML = ''
    vi.stubEnv('DEV', false)
    renderApp('/chat?mock=nope')
    await screen.findByRole('heading', { name: 'Start a chat' })
    expect(screen.queryByTestId('unknown-scenario')).toBeNull()
  })

  it('first send creates the chat, streams the reply and saves it once', async () => {
    const agent = await runningAgent()
    const { router } = renderApp(`/chat?agent=${agent.id}`)
    const box = await screen.findByLabelText(/^Message /)
    await userEvent.type(box, 'What changed today?{Enter}')
    await waitFor(() => expect(router.state.location.pathname).toMatch(/^\/chat\/[0-9a-f-]{36}$/))
    const sessionId = router.state.location.pathname.split('/')[2]!
    // The create response has no updated_at: the list row still shows a time (live smoke, 2026-09-27).
    const rowLink = screen
      .getAllByRole('link')
      .find((l) => l.getAttribute('href')?.startsWith(`/chat/${sessionId}`))!
    expect(rowLink).not.toHaveTextContent('—')
    expect(
      within(await screen.findByTestId('transcript')).getByText('What changed today?'),
    ).toBeInTheDocument()
    await waitFor(async () =>
      expect((await messages(sessionId)).filter((m) => m.role === 'assistant')).toHaveLength(1),
    )
    expect(chatMockRows().some((r) => r.session_id === sessionId)).toBe(true)
    expect(await screen.findByRole('button', { name: 'Copy' })).toBeInTheDocument()
    // The composer is ready again and kept focus.
    await waitFor(() => expect(screen.getByLabelText(/^Message /)).toHaveValue(''))
  })
})

describe('/chat/$sessionId', () => {
  it('renders a saved reply as Markdown with a code block', async () => {
    renderApp(`/chat/${DONE}`)
    expect(await screen.findByText('Three incidents')).toBeInTheDocument()
    expect(screen.getByRole('button', { name: 'Copy code' })).toBeInTheDocument()
    expect(screen.getByRole('button', { name: /Download snippet\.sh/ })).toBeInTheDocument()
    expect(screen.getByTestId('usage-chip')).toHaveTextContent('908 tokens')
  })

  it('View trace opens the waterfall in a sheet; header totals sum the loaded replies', async () => {
    renderApp(`/chat/${DONE}`)
    expect(await screen.findByText('Three incidents')).toBeInTheDocument()
    expect(screen.getAllByTestId('chat-totals')[0]).toHaveTextContent('908 tokens · $0.0021')
    await userEvent.click(screen.getByRole('button', { name: 'View trace' }))
    const sheet = await screen.findByRole('dialog', { name: 'Trace for this reply' })
    expect(await within(sheet).findByRole('tree')).toBeInTheDocument()
    expect(
      within(sheet).getByRole('link', { name: 'Open full trace' }).getAttribute('href'),
    ).toContain(`/sessions/${DONE}?trace=`)
  })

  it('a trace that is not in Tempo yet retries, then says so with Retry', async () => {
    let calls = 0
    server.use(
      http.get('/api/observability/trace/:id', () => {
        calls++
        return new HttpResponse('not found', { status: 404 })
      }),
    )
    vi.useFakeTimers({ shouldAdvanceTime: true })
    const user = userEvent.setup({ advanceTimers: vi.advanceTimersByTime })
    renderApp(`/chat/${DONE}`)
    await user.click(await screen.findByRole('button', { name: 'View trace' }))
    expect(await screen.findByText(/Looking for the trace/)).toBeInTheDocument()
    for (let i = 0; i < 4; i++) await vi.advanceTimersByTimeAsync(10_000)
    expect(await screen.findByText('Trace not in yet')).toBeInTheDocument()
    expect(calls).toBe(5)
  })

  it('an old unanswered message says so and Run again asks first', async () => {
    renderApp(`/chat/${LOST}`)
    expect(
      await screen.findByText('No reply has been saved for this message yet.'),
    ).toBeInTheDocument()
    await userEvent.click(screen.getByRole('button', { name: 'Run again' }))
    expect(
      await screen.findByRole('alertdialog', { name: 'Run this message again?' }),
    ).toBeInTheDocument()
  })

  it('a routed chat is writable (v1b G-17)', async () => {
    renderApp(`/chat/${ROUTED}`)
    expect(await screen.findByLabelText('Ask the Orchestrator')).toBeInTheDocument()
    expect(await screen.findByTestId('identity-chip')).toHaveTextContent('Orchestrator')
    expect(screen.queryByRole('button', { name: 'Stop receiving' })).toBeNull()
  })

  it('an unknown chat shows the missing state', async () => {
    renderApp('/chat/5eedc000-0000-4000-8000-0000000000ff')
    expect(await screen.findByText("This chat was deleted or isn't yours.")).toBeInTheDocument()
  })

  it('a request pauses the turn; answering resumes and the server saves the reply', async () => {
    configureChatMock({ scenario: 'hitl-options' })
    renderApp(`/chat/${DONE}`)
    const box = await screen.findByLabelText(/^Message /)
    await userEvent.type(box, 'Deploy it{Enter}')
    const card = await screen.findByTestId('request-card')
    expect(screen.getByRole('button', { name: 'Go to request' })).toBeInTheDocument()
    const option = within(card)
      .getAllByRole('button')
      .find((b) => !/Dismiss/.test(b.textContent ?? ''))!
    await userEvent.click(option)
    // The live block hands over to the saved row: wait for the text, not one node that may be swapped out.
    await waitFor(() => expect(screen.getByText(/^Thanks\. Continuing with/)).toBeInTheDocument())
    await waitFor(async () =>
      expect(
        (await messages(DONE)).filter(
          (m) => m.role === 'assistant' && m.content.startsWith('Thanks.'),
        ),
      ).toHaveLength(1),
    )
  })

  it('a failed save offers Save again and blocks the next send', async () => {
    configureChatMock({ saveFails: true })
    renderApp(`/chat/${DONE}`)
    const box = await screen.findByLabelText(/^Message /)
    await userEvent.type(box, 'One more{Enter}')
    expect(await screen.findByText('This reply may already be saved.')).toBeInTheDocument()
    expect(screen.getByRole('button', { name: 'Save again' })).toBeInTheDocument()
    await userEvent.type(screen.getByLabelText(/^Message /), 'next')
    expect(screen.getByRole('button', { name: 'Send' })).toBeDisabled()
  })

  it('delete is blocked once the chat has request history', async () => {
    configureChatMock({ scenario: 'hitl-options' })
    renderApp(`/chat/${DONE}`)
    await userEvent.type(await screen.findByLabelText(/^Message /), 'Deploy it{Enter}')
    await screen.findByTestId('request-card')
    await userEvent.click(screen.getByRole('button', { name: 'More chat actions' }))
    const item = await screen.findByRole('menuitem', { name: /Delete/ })
    expect(item).toHaveAttribute('data-disabled')
    expect(item).toHaveTextContent("Chats with approval history can't be deleted yet.")
  })
})

describe('review fixes (page)', () => {
  it('a failed delete keeps the chat and the dialog, and says why', async () => {
    server.use(
      http.delete(
        '/api/chat/sessions/:id',
        () => new HttpResponse('internal error', { status: 500 }),
      ),
    )
    renderApp(`/chat/${DONE}`)
    await waitFor(() => expect(screen.getByText('Three incidents')).toBeInTheDocument())
    await userEvent.click(screen.getByRole('button', { name: 'More chat actions' }))
    await userEvent.click(await screen.findByRole('menuitem', { name: 'Delete' }))
    const dialog = await screen.findByRole('alertdialog')
    await userEvent.click(within(dialog).getByRole('button', { name: 'Delete' }))
    // Shown in the dialog body and spoken by the dialog's own announcer (the page is hidden behind it).
    await waitFor(() =>
      expect(
        within(screen.getByRole('alertdialog')).getByText(/Couldn't delete this chat/, {
          selector: 'p',
        }),
      ).toBeInTheDocument(),
    )
    expect(within(screen.getByRole('alertdialog')).getByRole('status')).toHaveTextContent(
      /Couldn't delete this chat/,
    )
    expect(screen.getByText('Three incidents')).toBeInTheDocument()
  })

  it('rename keeps the list row’s agent (PUT returns a bare ChatSession); Esc cancels without saving', async () => {
    const puts: string[] = []
    server.events.on('request:start', ({ request }) => {
      if (request.method === 'PUT') puts.push(request.url)
    })
    renderApp(`/chat/${DONE}`)
    const title = await screen.findByRole('button', { name: 'Summarise last week’s incidents' })
    await userEvent.click(title)
    const input = screen.getByLabelText('Chat title')
    await userEvent.type(input, ' x{Escape}')
    expect(puts).toHaveLength(0)
    await userEvent.click(
      await screen.findByRole('button', { name: 'Summarise last week’s incidents' }),
    )
    await userEvent.clear(screen.getByLabelText('Chat title'))
    await userEvent.type(screen.getByLabelText('Chat title'), 'Incidents recap{Enter}')
    await waitFor(() =>
      expect(screen.getByRole('button', { name: 'Incidents recap' })).toBeInTheDocument(),
    )
    const rail = screen.getByRole('navigation', { name: 'Chats' })
    const row = within(rail)
      .getAllByRole('link')
      .find((l) => l.getAttribute('href')?.startsWith(`/chat/${DONE}`))!
    expect(row).toHaveTextContent('Incidents recap')
    // The row names the agent by its directory display name (v1c §5.2, v1a ISSUE-011).
    const agentId = chatMockRows().find((r) => r.session_id === DONE)!.agent_id!
    const a = await agentWhere((x) => x.id === agentId)
    expect(row).toHaveTextContent(a.display_name?.trim() || a.name)
    server.events.removeAllListeners()
  })

  it('a create failure removes the placeholder row, gives the text back, and Try again reuses the session id', async () => {
    const creates: string[] = []
    let fail = true
    server.use(
      http.post('/api/chat/sessions', async ({ request }) => {
        const body = (await request.clone().json()) as { session_id: string }
        creates.push(body.session_id)
        if (fail) return new HttpResponse('internal error', { status: 500 })
        return undefined
      }),
    )
    const agent = await runningAgent()
    renderApp(`/chat?agent=${agent.id}`)
    await userEvent.type(await screen.findByLabelText(/^Message /), 'will fail{Enter}')
    await waitFor(() => expect(screen.getByText('Not sent')).toBeInTheDocument())
    expect(screen.getByLabelText(/^Message /)).toHaveValue('will fail')
    const rail = screen.getByRole('navigation', { name: 'Chats' })
    expect(within(rail).queryByText('will fail')).toBeNull()
    fail = false
    await userEvent.click(screen.getByRole('button', { name: 'Try again' }))
    await waitFor(() => expect(creates).toHaveLength(2))
    expect(creates[1]).toBe(creates[0])
  })

  it('a paused turn unlocks once its request is dismissed', async () => {
    configureChatMock({ scenario: 'hitl-options' })
    renderApp(`/chat/${DONE}`)
    await userEvent.type(await screen.findByLabelText(/^Message /), 'Deploy it{Enter}')
    await screen.findByTestId('request-card')
    expect(screen.getByRole('button', { name: 'Send' })).toBeDisabled()
    await userEvent.click(screen.getByRole('button', { name: 'Dismiss request' }))
    await waitFor(() => expect(screen.queryByRole('button', { name: 'Go to request' })).toBeNull())
    await userEvent.type(screen.getByLabelText(/^Message /), 'next')
    expect(screen.getByRole('button', { name: 'Send' })).toBeEnabled()
  })

  it('a 400 offers Edit and send, which puts the message back', async () => {
    server.use(
      http.post('/api/orchestrator/a2a', () =>
        HttpResponse.json(
          { jsonrpc: '2.0', id: null, error: { code: -32602, message: 'bad params' } },
          { status: 400 },
        ),
      ),
    )
    renderApp(`/chat/${DONE}`)
    await userEvent.type(await screen.findByLabelText(/^Message /), 'bad one{Enter}')
    await userEvent.click(await screen.findByRole('button', { name: 'Edit and send' }))
    expect(screen.getByLabelText(/^Message /)).toHaveValue('bad one')
    expect(screen.getByLabelText(/^Message /)).toHaveFocus()
  })

  it('the transcript has no live regions of its own; StatusAnnouncer is the only one', async () => {
    renderApp(`/chat/${LOST}`)
    await screen.findByText('No reply has been saved for this message yet.')
    const transcript = screen.getByTestId('transcript')
    expect(
      transcript.querySelectorAll('[role="status"], [role="alert"], [aria-live]'),
    ).toHaveLength(0)
  })

  it('focus lands in the composer after the first send moves to the chat', async () => {
    const agent = await runningAgent()
    const { router } = renderApp(`/chat?agent=${agent.id}`)
    await userEvent.type(await screen.findByLabelText(/^Message /), 'hello{Enter}')
    await waitFor(() => expect(router.state.location.pathname).toMatch(/^\/chat\/[0-9a-f-]{36}$/))
    await waitFor(() => expect(screen.getByLabelText(/^Message /)).toHaveFocus())
  })
})

describe('TraceSheet states', () => {
  const openTrace = async () => {
    renderApp(`/chat/${DONE}`)
    await userEvent.click(await screen.findByRole('button', { name: 'View trace' }))
    return screen.findByRole('dialog', { name: 'Trace for this reply' })
  }

  it('503 says Tempo is not configured', async () => {
    server.use(
      http.get(
        '/api/observability/trace/:id',
        () => new HttpResponse('tempo not configured', { status: 503 }),
      ),
    )
    const sheet = await openTrace()
    await waitFor(() =>
      expect(within(sheet).getByRole('button', { name: /Retry/ })).toBeInTheDocument(),
    )
  })

  it('403 stops without retrying', async () => {
    let calls = 0
    server.use(
      http.get('/api/observability/trace/:id', () => {
        calls++
        return new HttpResponse('forbidden', { status: 403 })
      }),
    )
    const sheet = await openTrace()
    await waitFor(() =>
      expect(within(sheet).getByText("You can't see this trace.")).toBeInTheDocument(),
    )
    expect(calls).toBe(1)
  })

  it('404 then found renders the waterfall', async () => {
    let calls = 0
    server.use(
      http.get('/api/observability/trace/:id', () =>
        ++calls === 1 ? new HttpResponse('not found', { status: 404 }) : undefined,
      ),
    )
    vi.useFakeTimers({ shouldAdvanceTime: true })
    renderApp(`/chat/${DONE}`)
    const user = userEvent.setup({ advanceTimers: vi.advanceTimersByTime })
    await user.click(await screen.findByRole('button', { name: 'View trace' }))
    await vi.advanceTimersByTimeAsync(2_500)
    expect(await screen.findByRole('tree')).toBeInTheDocument()
  })
})
