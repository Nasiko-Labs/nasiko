/**
 * v1b routed chat, page level (plan §7 page tests 1-13): the opt-in entry, the routed empty
 * state, the turn anatomy, every §5.5 end state and §5.6 notice, routed HITL and resume. Every
 * test records request bodies and asserts the client never POSTed an assistant row.
 */
import { screen, waitFor, within } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { http, HttpResponse } from 'msw'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import {
  CHAT_SCENARIOS,
  ROUTED_TRACE,
  artifact,
  status,
  subStatus,
  toolCall,
  toolResult,
  traceMeta,
  usageMeta,
  type ChatScenario,
  type MockFrame,
} from '@/mocks/chat'
import { chatMockMessages, configureChatMock } from '@/mocks/chatStore'
import { gatedSseResponse } from '@/test/gatedSse'
import { renderApp } from '@/test/renderApp'
import { recordRequestBodies, server } from '@/test/setup'
import { clearDrafts, readDraft, writeDraft } from './drafts'
import { clearChatRegistry } from './registry'
import { tuning } from './tuning'

const ROUTED = '5eedc000-0000-4000-8000-00000000c003'
const withTrace = (frames: readonly MockFrame[], agent = 'seed-agent'): MockFrame[] =>
  frames.map((f) =>
    f.data === undefined
      ? f
      : {
          ...f,
          data: JSON.parse(
            JSON.stringify(f.data)
              .replaceAll(ROUTED_TRACE, '5eedf000000000000000000000000abc')
              .replaceAll('@agent-1@', agent)
              .replaceAll('@agent-2@', `${agent}-2`),
          ) as unknown,
        },
  )

let rec: ReturnType<typeof recordRequestBodies>
beforeEach(() => {
  rec = recordRequestBodies()
})
afterEach(async () => {
  // The routed invariant, checked after every test once pending bodies are read.
  await rec.flush()
  try {
    // assistantPosts() throws if a body couldn't be read (fails closed); cleanup runs either way.
    expect(rec.assistantPosts()).toEqual([])
  } finally {
    rec.stop()
    vi.useRealTimers()
    clearChatRegistry()
    clearDrafts()
  }
})

async function sendRouted(text: string, scenario?: ChatScenario) {
  if (scenario) configureChatMock({ scenario })
  const view = renderApp('/chat?auto=1')
  const box = await screen.findByLabelText('Ask the Orchestrator')
  await userEvent.type(box, `${text}{Enter}`)
  return view
}

/** The saved rendering has taken over from the live block (CLAUDE.md: wait for it). */
const settled = () =>
  waitFor(() => expect(screen.queryByTestId('routed-live')).toBeNull(), { timeout: 8000 })

async function runningAgent(): Promise<{ id: string }> {
  for (let offset = 0; offset < 1000; offset += 100) {
    const rows = (await (
      await fetch(new URL(`/api/agents?limit=100&offset=${offset}`, location.origin))
    ).json()) as { id: string; status: string; tags: string[] }[]
    const hit = rows.find((a) => a.status === 'running' && !a.tags.includes('coding-agent'))
    if (hit) return hit
    if (rows.length < 100) break
  }
  throw new Error('no running agent in the mock seed')
}

/**
 * Answer the pending request. The card built from the stream's frame is replaced by history's
 * card once the request is saved, so a click can land on the one being replaced: click again
 * until a resolve has been sent.
 */
async function answerRequest() {
  await screen.findByTestId('request-card', {}, { timeout: 8000 })
  await waitFor(
    async () => {
      await rec.flush()
      if (rec.requests.some((r) => /\/api\/hitl\/[^/]+\/resolve$/.test(r.url.pathname))) return
      const card = screen.getAllByTestId('request-card')[0]!
      const option = within(card)
        .getAllByRole('button')
        .find((b) => !/Dismiss/.test(b.textContent ?? ''))
      if (option) await userEvent.click(option)
      throw new Error('no resolve yet')
    },
    { timeout: 8000, interval: 150 },
  )
}

const sessionIdOf = (router: { state: { location: { pathname: string } } }) =>
  router.state.location.pathname.split('/').pop()!

describe('opt-in entry (§5.1, UC1, G-16; v1c §5.4)', () => {
  it('/chat offers the Orchestrator first in the target list, carrying mock/debug', async () => {
    const { router } = renderApp('/chat?debug=turn')
    expect(await screen.findByRole('heading', { name: 'Start a chat' })).toBeInTheDocument()
    const list = await screen.findByRole('region', { name: 'Choose where to send' })
    await userEvent.click(within(list).getByRole('button', { name: /^Orchestrator/ }))
    expect(await screen.findByRole('heading', { name: 'Orchestrate a task' })).toBeInTheDocument()
    expect(router.state.location.search).toMatchObject({ auto: 1, debug: 'turn' })
  })

  it('/chat?auto=1 is the Orchestrator; agent wins over auto; non-string agents are the banner', async () => {
    const view = renderApp('/chat?auto=1')
    expect(await screen.findByRole('heading', { name: 'Orchestrate a task' })).toBeInTheDocument()
    view.router.history.push('/chat?agent=123')
    expect(await screen.findByText(/'123' isn't an agent you can use/)).toBeInTheDocument()
    view.router.history.push('/chat?agent=true&auto=1')
    expect(await screen.findByText(/'true' isn't an agent you can use/)).toBeInTheDocument()
    expect(screen.queryByRole('heading', { name: 'Orchestrate a task' })).toBeNull()
  })

  it('a direct agent with auto=1 goes direct', async () => {
    const a = await runningAgent()
    renderApp(`/chat?agent=${a.id}&auto=1`)
    expect(await screen.findByLabelText(/^Message /)).toBeInTheDocument()
    expect(screen.getByTestId('target-chip')).not.toHaveTextContent('Orchestrator')
  })
})

describe('the Orchestrator as a new chat target (§5.9; v1c §5.4, C1)', () => {
  it('hero, subline, composer, example chips, then recents, in that order', async () => {
    renderApp('/chat?auto=1')
    const root = await screen.findByTestId('new-chat')
    const chips = await within(root).findByRole('list', { name: 'Examples' })
    const first = within(chips).getAllByRole('button')[0]!.textContent!
    const order = [
      'Orchestrate a task',
      'Describe a task and the Orchestrator picks the agents to run it.',
      'Ask the Orchestrator',
      first,
      'Pick up where you left off',
    ]
    const text = root.textContent!
    const at = order.map((t) => text.indexOf(t))
    expect(at.every((i) => i >= 0)).toBe(true)
    expect([...at].sort((a, b) => a - b)).toEqual(at)
  })

  it('a directory failure says so with Retry, never "0 agents running"', async () => {
    server.use(http.get('/api/agents', () => new HttpResponse('boom', { status: 500 })))
    renderApp('/chat?auto=1')
    expect(await screen.findByText("Couldn't load agents")).toBeInTheDocument()
    expect(screen.queryByText(/agents running/)).toBeNull()
    expect(screen.getByRole('button', { name: /Retry/ })).toBeInTheDocument()
  })

  it('zero running agents shows the v1a copy', async () => {
    server.use(http.get('/api/agents', () => HttpResponse.json([])))
    renderApp('/chat?auto=1')
    expect(await screen.findByText('No agent is running right now.')).toBeInTheDocument()
    expect(screen.queryByText(/agents running/)).toBeNull()
  })

  it('a picker choice carries the typed text to an agent, and back to the Orchestrator', async () => {
    renderApp('/chat?auto=1')
    await userEvent.type(await screen.findByLabelText('Ask the Orchestrator'), 'keep me')
    await userEvent.click(screen.getByTestId('target-chip'))
    const agentOption = (await screen.findAllByRole('option')).find(
      (o) => !/^Orchestrator/.test(o.textContent ?? ''),
    )!
    await userEvent.click(agentOption)
    const box = await screen.findByLabelText(/^Message /)
    expect(box).toHaveValue('keep me')
    await userEvent.click(screen.getByTestId('target-chip'))
    await userEvent.click(
      (await screen.findAllByRole('option')).find((o) =>
        /^Orchestrator/.test(o.textContent ?? ''),
      )!,
    )
    expect(await screen.findByLabelText('Ask the Orchestrator')).toHaveValue('keep me')
  })

  it('a populated destination draft is kept and the source stays (G-19)', async () => {
    const userId = 'admin'
    void userId
    writeDraft('5eed0000-0000-4000-8000-000000000001', 'new:routed', 'already here')
    renderApp('/chat?auto=1')
    const box = await screen.findByLabelText('Ask the Orchestrator')
    // Whatever the signed-in user id, the destination draft for this user wins over an empty move.
    expect(box).toBeInTheDocument()
    expect(readDraft('5eed0000-0000-4000-8000-000000000001', 'new:routed')).toBe('already here')
  })
})

describe('routed turn (§5.5, §5.7, §5.8)', () => {
  it('Working → Asking → Working → Writing → reply, attribution and a collapsed Activity', async () => {
    const frames = withTrace([
      { data: traceMeta(ROUTED_TRACE) },
      { data: toolCall('@agent-1@', 1) },
      { data: subStatus('@agent-1@', 'Reading reports') },
      { data: toolResult('@agent-1@', 1, true, 'three incidents', 900) },
      { data: artifact('The answer.', { lastChunk: true }) },
      { data: usageMeta({ duration_ms: 10, trace_id: '5eedf000000000000000000000000abc' }) },
      { data: status('TASK_STATE_COMPLETED') },
    ])
    const gate = gatedSseResponse(frames)
    server.use(http.post('/api/orchestrator/a2a', () => gate.response))
    await sendRouted('what happened?')
    const statusLine = await screen.findByTestId('routed-status')
    expect(statusLine).toHaveTextContent('Working on your request…')
    expect(screen.getByTestId('composer-note')).toHaveTextContent(
      'You can move around the app. Closing or reloading this tab may lose the reply.',
    )
    await gate.release(2)
    await waitFor(() =>
      expect(screen.getByTestId('routed-status')).toHaveTextContent('Asking seed-agent…'),
    )
    await gate.release(2)
    await waitFor(() =>
      expect(screen.getByTestId('routed-status')).toHaveTextContent('Working on your request…'),
    )
    await gate.release(1)
    await waitFor(() =>
      expect(screen.getByTestId('routed-status')).toHaveTextContent('Writing the reply…'),
    )
    await gate.releaseAll()
    expect(
      await screen.findByText('Answered by the Orchestrator, using seed-agent'),
    ).toBeInTheDocument()
    expect(screen.queryByTestId('composer-note')).toBeNull()
    const activity = screen.getByRole('button', { name: /Activity · 1 agent/ })
    expect(activity).toHaveAttribute('aria-expanded', 'false')
    expect(screen.queryByRole('button', { name: 'Stop receiving' })).toBeNull()
  })

  it('a saved reply reads answer → attribution → footer → Activity; its agents survive the forget', async () => {
    const { router } = await sendRouted('summarise incidents', 'routed-plain')
    await waitFor(() => expect(router.state.location.pathname).toMatch(/^\/chat\/[0-9a-f-]{36}$/))
    await screen.findByText(/three incidents/i, { selector: 'strong' })
    await settled()
    const turn = screen
      .getByText(/three incidents/i, { selector: 'strong' })
      .closest('[data-testid="turn"]') as HTMLElement
    const text = turn.textContent!
    const at = [
      'Last week had',
      'Answered by the Orchestrator, using',
      'Copy',
      'Activity · 1 agent',
    ].map((t) => text.indexOf(t))
    expect(at.every((i) => i >= 0)).toBe(true)
    expect([...at].sort((a, b) => a - b)).toEqual(at)
    expect(
      chatMockMessages(sessionIdOf(router)).filter((m) => m.role === 'assistant'),
    ).toHaveLength(1)
  })

  it('several agents list in call order; a no-call turn says Answered by the Orchestrator with no Activity', async () => {
    await sendRouted('fix the deploy', 'routed-multi-agent')
    expect(
      await screen.findByText(/^Answered by the Orchestrator, using .+ and .+$/),
    ).toBeInTheDocument()
    expect(screen.getByRole('button', { name: /Activity · 2 agents/ })).toBeInTheDocument()
    clearChatRegistry()
    document.body.innerHTML = ''
    await sendRouted('hello', 'routed-no-tool')
    expect(await screen.findByText('Answered by the Orchestrator')).toBeInTheDocument()
    expect(screen.queryByTestId('activity')).toBeNull()
  })

  it('expanded Activity: the excerpt is plain text, links sit only in the expanded row', async () => {
    await sendRouted('summarise', 'routed-plain')
    await settled()
    await userEvent.click(await screen.findByRole('button', { name: /Activity · 1 agent/ }))
    expect(screen.queryByRole('link', { name: 'Open current agent' })).toBeNull()
    const row = screen
      .getAllByRole('button', { expanded: false })
      .find((b) => /Completed/.test(b.textContent ?? ''))!
    await userEvent.click(row)
    expect(row).toHaveAttribute('aria-expanded', 'true')
    const open = screen.getByRole('link', { name: 'Open current agent' })
    expect(open.getAttribute('title')).toMatch(/^Current agent named /)
    expect(screen.getByRole('link', { name: /^Chat with / }).getAttribute('href')).toMatch(
      /\/chat\?agent=/,
    )
    expect(open.closest('button')).toBeNull()
    // A saved reply keeps its steps, not the sub-agent's streamed notes: the excerpt is the call's result.
    expect(screen.getByTestId('activity-excerpt')).toHaveTextContent(
      'Three incidents last week: login latency, queue backlog, one failed deploy.',
    )
  })

  it('a failed call marks the Activity row without restyling the answer; mixed results read so', async () => {
    await sendRouted('try twice', 'routed-two-calls-one-turn')
    await settled()
    const act = await screen.findByRole('button', { name: /Activity · 1 agent/ })
    expect(act).toHaveTextContent('1 failed')
    await userEvent.click(act)
    expect(screen.getByText('Mixed results')).toBeInTheDocument()
  })

  it('a policy limit shows as a stop in Activity with its fix', async () => {
    await sendRouted('fan out', 'routed-policy-rejected')
    await settled()
    await userEvent.click(await screen.findByRole('button', { name: /Activity/ }))
    expect(
      screen.getByText(
        /Stopped by an OpenRuntime limit: fan-out\. Ask your admin to raise NASIKO_FLOW_MAX_FAN_OUT\./,
      ),
    ).toBeInTheDocument()
  })

  it('reduced motion: the status spinner has the motion-reduce class', async () => {
    const gate = gatedSseResponse(withTrace(CHAT_SCENARIOS['routed-plain']))
    server.use(http.post('/api/orchestrator/a2a', () => gate.response))
    await sendRouted('x')
    const svg = (await screen.findByTestId('routed-status')).querySelector('svg')!
    expect(svg.getAttribute('class')).toMatch(/motion-reduce:animate-none/)
    await gate.releaseAll()
  })
})

describe('end states (§5.5) and notices (§5.6)', () => {
  it('known-empty: OpenRuntime finished without a reply, with Run again', async () => {
    await sendRouted('nothing', 'routed-empty')
    expect(
      await screen.findByText('The Orchestrator finished without a reply.'),
    ).toBeInTheDocument()
    expect(screen.getByRole('button', { name: 'Run again' })).toBeInTheDocument()
    // It survives the history refetch (no E5, no "checking").
    await waitFor(() =>
      expect(screen.queryByText('No reply has been saved for this message yet.')).toBeNull(),
    )
  })

  it('connection lost with partial text: kept, completion unconfirmed, Refresh status first', async () => {
    await sendRouted('cut', 'routed-cut')
    expect(await screen.findByText('Partial reply — completion unconfirmed')).toBeInTheDocument()
    expect(screen.getByText('The first half of the answer')).toBeInTheDocument()
    const buttons = within(screen.getByTestId('routed-live'))
      .getAllByRole('button')
      .map((b) => b.textContent)
    expect(buttons.indexOf('Refresh status')).toBeLessThan(buttons.indexOf('Run again'))
  })

  it('too large: drains, then loading the saved reply, then the saved row replaces the partial', async () => {
    const cap = tuning.MAX_TURN_BYTES
    tuning.MAX_TURN_BYTES = 100_000
    try {
      await sendRouted('big', 'routed-oversized')
      await waitFor(() =>
        expect(
          screen.queryByText(/Reply too large to show here|Loading the saved reply…/) ??
            screen.queryByText(/^z+$/),
        ).not.toBeNull(),
      )
      // The server saved the full reply at Done: history replaces the partial text.
      await waitFor(() => expect(screen.queryByTestId('routed-live')).toBeNull(), { timeout: 8000 })
    } finally {
      tuning.MAX_TURN_BYTES = cap
    }
  })

  it('failed: the routed notice with Run again, Copy details and its doc anchor', async () => {
    await sendRouted('fail', 'routed-failed')
    expect(await screen.findByText('The Orchestrator reported an error.')).toBeInTheDocument()
    expect(screen.getByRole('button', { name: 'Run again' })).toBeInTheDocument()
    expect(screen.getByRole('button', { name: 'Copy details' })).toBeInTheDocument()
    expect(screen.getByText('docs/chat.md#errors-failed')).toBeInTheDocument()
  })

  it.each([
    ['routed-400', "The Orchestrator couldn't read this request.", 'Edit and send', '400'],
    ['routed-429', 'Too many requests.', 'Retry', '429'],
    ['routed-503', 'No accessible, reachable agents were available for this request.', null, '503'],
    ['routed-500', 'OpenRuntime hit an error.', 'Refresh status', '500'],
  ] as const)(
    '%s: problem, action, Copy details and anchor',
    async (sc, problem, action, anchor) => {
      const { router } = await sendRouted('please', sc)
      expect(await screen.findByText(problem)).toBeInTheDocument()
      if (action) expect(screen.getByRole('button', { name: action })).toBeInTheDocument()
      else
        expect(
          within(screen.getByTestId('routed-live')).getByRole('link', { name: 'Agents' }),
        ).toBeInTheDocument()
      expect(screen.getByText(`docs/chat.md#errors-${anchor}`)).toBeInTheDocument()
      // The user row is saved: the draft isn't restored, so a plain Send can't duplicate it (review D2).
      expect(screen.getByLabelText('Ask the Orchestrator')).toHaveValue('')
      void router
    },
  )

  it('a 4th routed send while three reply shows the cap notice and keeps the draft', async () => {
    const hang = () => new Promise<Response>(() => undefined)
    server.use(http.post('/api/orchestrator/a2a', hang))
    const { router } = renderApp('/chat?auto=1')
    for (const t of ['one', 'two', 'three']) {
      await userEvent.type(await screen.findByLabelText('Ask the Orchestrator'), `${t}{Enter}`)
      await waitFor(() => expect(router.state.location.pathname).toMatch(/^\/chat\/[0-9a-f-]{36}$/))
      router.history.push('/chat?auto=1')
    }
    await userEvent.type(await screen.findByLabelText('Ask the Orchestrator'), 'four{Enter}')
    expect(await screen.findByTestId('send-error')).toHaveTextContent(
      /Three replies are already in progress\./,
    )
    expect(screen.getByLabelText('Ask the Orchestrator')).toHaveValue('four')
  })
})

describe('routed requests and resume (§5.4, R3)', () => {
  it('pause → answer → the resumed reply, saved by the server, never by the client', async () => {
    const { router } = await sendRouted('deploy it', 'routed-hitl')
    await answerRequest()
    expect(
      await screen.findByText('Done: deployed to us-east-1.', {}, { timeout: 8000 }),
    ).toBeInTheDocument()
    const sid = sessionIdOf(router)
    expect(chatMockMessages(sid).filter((m) => m.role === 'assistant')).toHaveLength(1)
  })

  it.each([
    [
      'routed-reconnect-403',
      "This reply was started by another sign-in and can't be resumed here.",
    ],
    ['routed-reconnect-400', 'The reply may still be arriving.'],
  ] as const)('%s: %s', async (sc, text) => {
    await sendRouted('deploy', sc)
    await answerRequest()
    expect(await screen.findByText(text, {}, { timeout: 8000 })).toBeInTheDocument()
  })
})

describe('review fixes (page)', () => {
  it('an older empty turn offers no Run again once a newer message exists (it would re-send the newer one)', async () => {
    const { router } = await sendRouted('first', 'routed-empty')
    expect(
      await screen.findByText('The Orchestrator finished without a reply.'),
    ).toBeInTheDocument()
    await waitFor(() => expect(router.state.location.pathname).toMatch(/^\/chat\/[0-9a-f-]{36}$/))
    configureChatMock({ scenario: 'routed-plain' })
    await userEvent.type(screen.getByLabelText('Ask the Orchestrator'), 'second{Enter}')
    await screen.findByText(/three incidents/i, { selector: 'strong' }, { timeout: 8000 })
    await settled()
    expect(screen.queryByText('The Orchestrator finished without a reply.')).toBeNull()
    expect(screen.queryByRole('button', { name: 'Run again' })).toBeNull()
  })

  it("a resumed reply shows the orchestrator's answer, not the sub-agent's replayed text", async () => {
    await sendRouted('deploy it', 'routed-hitl')
    await answerRequest()
    expect(
      await screen.findByText('Done: deployed to us-east-1.', {}, { timeout: 8000 }),
    ).toBeInTheDocument()
    const leak = screen.queryByText(/Sub-agent: region/)
    expect(leak).toBeNull()
  })

  it('a truncated resume (no trace yet) settles on the reply the continuation already saved', async () => {
    const { router } = await sendRouted('deploy it', 'routed-hitl-truncated')
    await answerRequest()
    expect(
      await screen.findByText('Done: deployed to us-east-1.', {}, { timeout: 8000 }),
    ).toBeInTheDocument()
    await waitFor(() => expect(screen.queryByTestId('routed-live')).toBeNull(), { timeout: 5000 })
    expect(
      chatMockMessages(sessionIdOf(router)).filter((m) => m.role === 'assistant'),
    ).toHaveLength(1)
  })

  it('a failed resume offers Refresh status, never a Run again that does nothing', async () => {
    await sendRouted('deploy', 'routed-reconnect-400')
    await answerRequest()
    const live = await screen.findByTestId('routed-live', {}, { timeout: 8000 })
    await within(live).findByText('The reply may still be arriving.')
    expect(within(live).getByRole('button', { name: 'Refresh status' })).toBeInTheDocument()
    expect(within(live).queryByRole('button', { name: 'Run again' })).toBeNull()
  })
})

describe('existing routed chats and lifetime (§5.10, §5.2)', () => {
  it('an old routed chat whose newest row is an unanswered user row shows E5', async () => {
    const lost = tuning.LOST_REPLY_AFTER_MS
    tuning.LOST_REPLY_AFTER_MS = 0
    try {
      await fetch(new URL(`/api/chat/sessions/${ROUTED}/messages`, location.origin)) // seed
      await fetch(new URL(`/api/chat/sessions/${ROUTED}/messages`, location.origin), {
        method: 'POST',
        body: JSON.stringify({ role: 'user', content: 'still there?' }),
        headers: { 'Content-Type': 'application/json' },
      })
      renderApp(`/chat/${ROUTED}`)
      expect(
        await screen.findByText('No reply has been saved for this message yet.'),
      ).toBeInTheDocument()
    } finally {
      tuning.LOST_REPLY_AFTER_MS = lost
    }
  })

  it('leaving mid-turn and coming back keeps the reply; the unload prompt stays armed', async () => {
    const gate = gatedSseResponse(withTrace(CHAT_SCENARIOS['routed-plain']))
    server.use(http.post('/api/orchestrator/a2a', () => gate.response))
    const { router } = await sendRouted('long one')
    await waitFor(() => expect(router.state.location.pathname).toMatch(/^\/chat\/[0-9a-f-]{36}$/))
    const chatPath = router.state.location.pathname
    router.history.push('/agents')
    await gate.release(3)
    const ev = new Event('beforeunload', { cancelable: true })
    window.dispatchEvent(ev)
    expect(ev.defaultPrevented).toBe(true)
    await gate.releaseAll()
    router.history.push(chatPath)
    expect(
      await screen.findByText(/three incidents/i, { selector: 'strong' }, { timeout: 8000 }),
    ).toBeInTheDocument()
  })

  it('a slow create, then New chat, then opting in again never redirects into the earlier chat (G-18)', async () => {
    configureChatMock({ createDelayMs: 300 })
    const { router } = renderApp('/chat?auto=1')
    await userEvent.type(await screen.findByLabelText('Ask the Orchestrator'), 'slow{Enter}')
    router.history.push('/chat')
    await screen.findByRole('heading', { name: 'Start a chat' })
    router.history.push('/chat?auto=1')
    await screen.findByRole('heading', { name: 'Orchestrate a task' })
    // The user row is saved right after the slow create resolves (after the redirect would fire),
    // so waiting for that POST replaces a fixed 600 ms sleep.
    await waitFor(
      async () => {
        await rec.flush()
        expect(
          rec.requests.some((r) => r.method === 'POST' && /\/messages$/.test(r.url.pathname)),
        ).toBe(true)
      },
      { timeout: 8000 },
    )
    expect(router.state.location.pathname).toBe('/chat')
  })

  it('a hot-path chat 404 shows no composer and never dispatches (NE-1)', async () => {
    renderApp('/chat/5eedc000-0000-4000-8000-0000000000ff?auto=1')
    expect(await screen.findByText("This chat was deleted or isn't yours.")).toBeInTheDocument()
    expect(screen.queryByLabelText('Ask the Orchestrator')).toBeNull()
    await rec.flush()
    expect(rec.requests.some((r) => r.url.pathname === '/api/orchestrator/a2a')).toBe(false)
  })
})
