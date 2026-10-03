/**
 * Chat pieces in isolation (plan §12): Markdown safety and code blocks, RequestCard per kind,
 * target resolution and the chooser, chat kinds, drafts (and their storage limits), announcements,
 * and the scenario-key mirror.
 */
import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import {
  createMemoryHistory,
  createRootRoute,
  createRouter,
  RouterProvider,
} from '@tanstack/react-router'
import { act, fireEvent, render, screen, waitFor } from '@testing-library/react'
import type { ReactNode } from 'react'
import userEvent from '@testing-library/user-event'
import { afterEach, describe, expect, it, vi } from 'vitest'
import { CHAT_SCENARIOS } from '@/mocks/chat'
import type { Agent } from '@/features/agents/types'
import { ApiError } from '@/lib/api/client'
import { Composer } from './components/Composer'
import { Markdown } from './components/Markdown'
import { RequestCard, type RequestActions } from './components/RequestCard'
import { TurnView } from './components/TurnView'
import { ModalAnnouncer, StatusAnnouncer } from './components/StatusAnnouncer'
import { announce } from './announce'
import { ChatError } from './errors'
import { emptyTurn } from './a2aReducer'
import { clearDrafts, readDraft, writeDraft } from './drafts'
import { announcement, chatKind, downloadName } from './format'
import { CHAT_SCENARIO_KEYS } from './scenarioKeys'
import { chooserAgents, resolveAgentParam, resolveChatTarget } from './target'
import { tuning } from './tuning'
import type { LiveTurn } from './turnRegistry'
import type { ChatSessionRow, HitlDto } from './types'

afterEach(() => {
  clearDrafts()
  vi.restoreAllMocks()
})

describe('Markdown', () => {
  it('renders no raw HTML and turns unsafe links into text', () => {
    const { container } = render(
      <Markdown
        text={'<img src=x onerror=alert(1)> [a](javascript:alert(1)) [b](https://example.com)'}
      />,
    )
    expect(container.querySelector('img')).toBeNull()
    expect(screen.queryByRole('link', { name: 'a' })).toBeNull()
    expect(screen.getByText('a')).toBeInTheDocument()
    const b = screen.getByRole('link', { name: 'b' })
    expect(b).toHaveAttribute('target', '_blank')
    expect(b).toHaveAttribute('rel', 'noopener noreferrer')
  })

  it('never loads an image from a reply (zero-click leak); it becomes a labelled link', () => {
    const { container } = render(
      <Markdown text={'![chart](https://example.com/p.png?d=secret) ![](javascript:x)'} />,
    )
    expect(container.querySelector('img')).toBeNull()
    expect(screen.getByRole('link', { name: 'Image: chart' })).toHaveAttribute(
      'href',
      'https://example.com/p.png?d=secret',
    )
    expect(screen.getByText('Image')).toBeInTheDocument()
  })

  it('code blocks get Copy and a download name from the language', () => {
    render(<Markdown text={'```python\nprint(1)\n```\n\nand `inline`'} />)
    expect(screen.getByRole('button', { name: 'Copy code' })).toBeInTheDocument()
    expect(screen.getByRole('button', { name: /Download snippet\.py/ })).toBeInTheDocument()
    expect(screen.getByText('inline').tagName).toBe('CODE')
    expect(downloadName('')).toBe('snippet.txt')
    expect(downloadName('Dockerfile')).toBe('Dockerfile')
  })

  it('tables scroll inside their own wrapper', () => {
    const { container } = render(<Markdown text={'| a | b |\n|---|---|\n| 1 | 2 |'} />)
    expect(container.querySelector('div.overflow-x-auto > table')).not.toBeNull()
  })
})

/** RequestCard links the agent (AgentLink needs the query client and a router). */
async function renderInApp(ui: ReactNode) {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } })
  const router = createRouter({
    routeTree: createRootRoute({ component: () => <>{ui}</> }),
    history: createMemoryHistory({ initialEntries: ['/'] }),
  })
  const out = render(
    <QueryClientProvider client={client}>
      <RouterProvider router={router} />
    </QueryClientProvider>,
  )
  await screen.findByTestId(/request-(card|receipt)/)
  return out
}

const request = (over: Partial<HitlDto> = {}): HitlDto => ({
  id: 'h1',
  kind: 'input_required',
  status: 'pending',
  resume_status: 'not_started',
  question: { message: 'Which region?', options: [{ label: 'eu' }, { label: 'us' }] },
  human_response: null,
  execution: {
    origin: 'direct_chat',
    agent_id: 'a1',
    task_id: null,
    context_id: 's1',
    chat_session_id: null,
    maf_execution_id: null,
    maf_step_index: null,
  },
  allowed_actions: ['answer', 'cancel'],
  expires_at: '2026-10-04T00:00:00Z',
  created_at: '2026-09-27T00:00:00Z',
  resolved_at: null,
  ...over,
})

function actions(over: Partial<RequestActions> = {}): RequestActions {
  return {
    resolve: vi.fn(async (id: string) => request({ id, status: 'resolved' })),
    cancel: vi.fn(async (id: string) => request({ id, status: 'canceled' })),
    onDone: vi.fn(),
    ...over,
  }
}

describe('RequestCard', () => {
  it('an option answers with its label', async () => {
    const a = actions()
    await renderInApp(<RequestCard request={request()} agentName="ops" actions={a} />)
    await userEvent.click(screen.getByRole('button', { name: 'eu' }))
    expect(a.resolve).toHaveBeenCalledWith('h1', { answer: 'eu' })
    await waitFor(() => expect(a.onDone).toHaveBeenCalled())
  })

  it('tool approval offers once, session scope and reject, and says arguments are missing', async () => {
    const a = actions()
    await renderInApp(
      <RequestCard
        request={request({
          kind: 'tool_approval',
          question: { tool_name: 'send_email' },
          allowed_actions: ['approve', 'reject', 'cancel'],
        })}
        agentName="ops"
        actions={a}
      />,
    )
    expect(screen.getByText("This server doesn't send the tool's arguments.")).toBeInTheDocument()
    await userEvent.click(
      screen.getByRole('button', { name: 'Always allow send_email in this chat' }),
    )
    expect(a.resolve).toHaveBeenCalledWith('h1', { decision: 'approve', scope: 'session' })
  })

  it('sign-in is a real link (opens inside the click), names its host, then confirms', async () => {
    const a = actions({ resolve: vi.fn(async () => request({ kind: 'auth_required' })) })
    await renderInApp(
      <RequestCard
        request={request({
          kind: 'auth_required',
          question: { provider: 'GitHub', auth_url: 'https://github.com/login' },
        })}
        agentName="ops"
        actions={a}
      />,
    )
    const link = screen.getByRole('link', { name: 'Sign in to GitHub' })
    expect(link).toHaveAttribute('href', 'https://github.com/login')
    expect(link).toHaveAttribute('target', '_blank')
    expect(link).toHaveAttribute('rel', 'noopener noreferrer')
    expect(screen.getByText('Opens github.com')).toBeInTheDocument()
    link.addEventListener('click', (e) => e.preventDefault())
    await userEvent.click(link)
    expect(a.resolve).toHaveBeenCalledWith('h1', { auth_action: 'start' })
    await userEvent.click(await screen.findByRole('button', { name: "I've signed in" }))
    expect(a.resolve).toHaveBeenLastCalledWith('h1', { auth_action: 'confirm' })
  })

  it('a javascript: sign-in URL is never a link', async () => {
    await renderInApp(
      <RequestCard
        request={request({
          kind: 'auth_required',
          question: { provider: 'X', auth_url: 'javascript:alert(1)' },
        })}
        agentName="ops"
        actions={actions({ resolve: vi.fn(async () => request()) })}
      />,
    )
    expect(screen.queryByRole('link')).toBeNull()
    await userEvent.click(screen.getByRole('button', { name: 'Sign in to X' }))
    await screen.findByRole('button', { name: "I've signed in" })
  })

  it('409 says Handled elsewhere', async () => {
    const a = actions({
      resolve: vi.fn(async () => {
        throw new ApiError(409, 'request is expired', '/api/hitl/h1/resolve', 'conflict')
      }),
    })
    await renderInApp(<RequestCard request={request()} agentName="ops" actions={a} />)
    await userEvent.click(screen.getByRole('button', { name: 'us' }))
    expect(await screen.findByText('Handled elsewhere.')).toBeInTheDocument()
  })

  it('multi-select: Submit waits for a choice or text, then sends the picks and the custom answer', async () => {
    const a = actions()
    await renderInApp(
      <RequestCard
        request={request({
          question: {
            message: 'Which regions?',
            options: [{ label: 'eu' }, { label: 'us' }],
            multi_select: true,
            allow_custom_input: true,
          },
        })}
        agentName="ops"
        actions={a}
      />,
    )
    const submit = screen.getByRole('button', { name: 'Submit' })
    expect(submit).toBeDisabled()
    await userEvent.click(screen.getByRole('checkbox', { name: 'us' }))
    await waitFor(() => expect(submit).toBeEnabled())
    await userEvent.click(screen.getByRole('checkbox', { name: 'us' }))
    await waitFor(() => expect(submit).toBeDisabled())
    await userEvent.click(screen.getByRole('checkbox', { name: 'eu' }))
    await userEvent.type(screen.getByLabelText('Something else'), '  apac ')
    await userEvent.click(submit)
    await waitFor(() =>
      expect(a.resolve).toHaveBeenCalledWith('h1', { answer: ['eu'], custom_answer: 'apac' }),
    )
  })

  it('a typed answer sends the trimmed text; blank never submits', async () => {
    const a = actions()
    await renderInApp(
      <RequestCard
        request={request({ question: { message: 'Why?' } })}
        agentName="ops"
        actions={a}
      />,
    )
    const submit = screen.getByRole('button', { name: 'Submit' })
    await userEvent.type(screen.getByLabelText('Why?'), '   ')
    expect(submit).toBeDisabled()
    await userEvent.type(screen.getByLabelText('Why?'), 'because ')
    await waitFor(() => expect(submit).toBeEnabled())
    await userEvent.click(submit)
    await waitFor(() => expect(a.resolve).toHaveBeenCalledWith('h1', { answer: 'because' }))
  })

  it('a receipt has no buttons even though allowed_actions still lists them', async () => {
    await renderInApp(
      <RequestCard
        request={request({ status: 'resolved', human_response: { answer: 'eu' } })}
        agentName="ops"
        actions={actions()}
      />,
    )
    expect(screen.getByTestId('request-receipt')).toHaveTextContent('Answered')
    expect(screen.queryByRole('button')).toBeNull()
  })
})

const agent = (over: Partial<Agent>): Agent =>
  ({
    id: '11111111-1111-4111-8111-111111111111',
    name: 'ops',
    status: 'running',
    tags: [],
    metadata: {},
    skills: [],
    ...over,
  }) as Agent
const dir = (agents: Agent[]) => {
  const byId = new Map(agents.map((a) => [a.id, a]))
  const byNameAll = new Map<string, Agent[]>()
  for (const a of agents) byNameAll.set(a.name, [...(byNameAll.get(a.name) ?? []), a])
  return { loaded: true, byId, byNameAll }
}

describe('target resolution (§6.11)', () => {
  const ops = agent({})
  const twin = agent({ id: '22222222-2222-4222-8222-222222222222' })
  const harness = agent({
    id: '33333333-3333-4333-8333-333333333333',
    name: 'cc',
    tags: ['coding-agent'],
  })

  it('a UUID, a unique name, a shared name, an unknown value', () => {
    expect(resolveAgentParam(ops.id, dir([ops]))).toEqual({ kind: 'direct', agent: ops })
    expect(resolveAgentParam('ops', dir([ops]))).toEqual({ kind: 'direct', agent: ops })
    expect(resolveAgentParam('ops', dir([ops, twin])).kind).toBe('ambiguous')
    expect(resolveAgentParam('ops-2', dir([ops]))).toEqual({ kind: 'invalid', value: 'ops-2' })
    expect(resolveAgentParam(undefined, dir([ops]))).toEqual({ kind: 'choose' })
  })

  it('never targets a harness', () => {
    expect(resolveAgentParam('cc', dir([harness])).kind).toBe('invalid')
    expect(resolveAgentParam(harness.id, dir([harness])).kind).toBe('invalid')
  })

  const row = (over: Partial<ChatSessionRow>): ChatSessionRow => ({
    session_id: 's',
    agent_id: ops.id,
    title: 't',
    created_at: '',
    updated_at: '',
    ...over,
  })

  it('an existing chat: direct, routed, recorded, removed, still loading', () => {
    expect(resolveChatTarget(row({}), [], true, dir([ops]))).toEqual({ kind: 'direct', agent: ops })
    expect(
      resolveChatTarget(
        row({ agent_id: null, agent_url: '/api/orchestrator/a2a' }),
        [],
        true,
        dir([ops]),
      ),
    ).toEqual({ kind: 'routed' })
    expect(resolveChatTarget(row({ is_coding_agent: true }), [], true, dir([ops]))).toMatchObject({
      kind: 'readonly',
      why: 'recorded',
    })
    // A null agent without a routed marker is a removed agent, never routed.
    expect(resolveChatTarget(row({ agent_id: null }), [], true, dir([ops]))).toMatchObject({
      kind: 'readonly',
      why: 'removed',
    })
    expect(resolveChatTarget(row({ agent_id: twin.id }), [], true, dir([ops]))).toMatchObject({
      kind: 'readonly',
      why: 'removed',
    })
    expect(resolveChatTarget(undefined, [], false, dir([ops]))).toEqual({ kind: 'loading' })
    expect(
      resolveChatTarget(
        undefined,
        [request({ execution: { ...request().execution, agent_id: ops.id } })],
        false,
        dir([ops]),
      ),
    ).toEqual({ kind: 'direct', agent: ops })
    // v1b: a routed create stores agent_url NULL (chat/routes.rs:296-297), so an explicit null is routed.
    expect(chatKind({ agent_id: null, agent_url: null })).toBe('routed')
  })

  it('the chooser offers running A2A agents only, never harnesses or stopped agents', () => {
    const a = (id: string, status: string, tags: string[] = []) =>
      ({ id, name: id, status, tags, skills: [] }) as unknown as Agent
    expect(
      chooserAgents([
        a('run', 'running'),
        a('stop', 'stopped'),
        a('harness', 'running', ['coding-agent']),
      ]).map((x) => x.id),
    ).toEqual(['run'])
    expect(chooserAgents(undefined)).toEqual([])
  })
})

describe('drafts', () => {
  it('are kept per user and chat', () => {
    writeDraft('u1', 's1', 'hello')
    expect(readDraft('u1', 's1')).toBe('hello')
    expect(readDraft('u2', 's1')).toBe('')
    clearDrafts('u1')
    expect(readDraft('u1', 's1')).toBe('')
  })

  it('a draft over DRAFT_MAX_CHARS stays in memory but not in localStorage; storage that throws still keeps the draft', () => {
    const long = 'x'.repeat(tuning.DRAFT_MAX_CHARS + 1)
    writeDraft('u', 'c1', 'short')
    expect(localStorage.getItem('ui-lab:chat-draft:u:c1')).toBe('short')
    writeDraft('u', 'c1', long)
    expect(readDraft('u', 'c1')).toBe(long)
    expect(localStorage.getItem('ui-lab:chat-draft:u:c1')).toBeNull()

    // setup.ts gives each test its own in-memory localStorage.
    vi.spyOn(localStorage, 'setItem').mockImplementation(() => {
      throw new Error('QuotaExceeded')
    })
    expect(() => writeDraft('u', 'c2', 'kept')).not.toThrow()
    expect(readDraft('u', 'c2')).toBe('kept')

    vi.spyOn(localStorage, 'getItem').mockImplementation(() => {
      throw new Error('blocked')
    })
    clearDrafts('u')
    expect(readDraft('u', 'c3')).toBe('')
  })

  it('clearDrafts(user) keeps other users’ drafts', () => {
    writeDraft('a', 'c', 'mine')
    writeDraft('b', 'c', 'theirs')
    clearDrafts('a')
    expect(readDraft('a', 'c')).toBe('')
    expect(readDraft('b', 'c')).toBe('theirs')
  })
})

describe('announcements', () => {
  const turn = (phase: LiveTurn['phase']): LiveTurn => ({
    id: 't',
    sessionId: 's',
    agentId: 'a',
    userText: 'x',
    phase,
    state: emptyTurn(),
    startedAt: 0,
    idle: false,
    stopped: false,
    finalized: false,
    error: null,
    saved: null,
    pendingSave: null,
    frames: [],
    chatMode: 'direct',
    operation: 'send',
    attempt: 'send',
  })

  it('say when a reply completes or a request arrives, never while streaming', () => {
    expect(announcement(turn('streaming'), turn('done'), 'ops')).toBe('Reply from ops complete')
    expect(announcement(turn('streaming'), turn('paused'), 'ops')).toBe('Approval requested by ops')
    expect(announcement(turn('waiting'), turn('streaming'), 'ops')).toBeNull()
    expect(announcement(turn('done'), turn('done'), 'ops')).toBeNull()
  })
})

describe('scenario keys', () => {
  it('mirror CHAT_SCENARIOS exactly', () => {
    expect([...CHAT_SCENARIO_KEYS].sort()).toEqual(Object.keys(CHAT_SCENARIOS).sort())
  })
})

describe('Composer', () => {
  const props = (over: Partial<Parameters<typeof Composer>[0]> = {}) => ({
    agentName: 'ops',
    value: 'hi',
    onChange: vi.fn(),
    onSend: vi.fn(),
    onStop: vi.fn(),
    onGoToRequest: vi.fn(),
    onNewChat: vi.fn(),
    mode: { kind: 'ready' } as const,
    ...over,
  })

  it('Enter sends; Shift+Enter and IME composition Enter do not', () => {
    const p = props()
    render(<Composer {...p} />)
    const box = screen.getByLabelText('Message ops')
    fireEvent.keyDown(box, { key: 'Enter', isComposing: true })
    fireEvent.keyDown(box, { key: 'Enter', shiftKey: true })
    expect(p.onSend).not.toHaveBeenCalled()
    fireEvent.keyDown(box, { key: 'Enter' })
    expect(p.onSend).toHaveBeenCalledOnce()
  })

  it('Enter while locked keeps the draft and sends nothing', () => {
    const p = props({ mode: { kind: 'locked' } })
    render(<Composer {...p} />)
    fireEvent.keyDown(screen.getByLabelText('Message ops'), { key: 'Enter' })
    expect(p.onSend).not.toHaveBeenCalled()
    expect(p.onChange).not.toHaveBeenCalled()
  })

  it('/ focuses the composer unless typing elsewhere', () => {
    render(
      <>
        <input aria-label="other" />
        <Composer {...props()} />
      </>,
    )
    fireEvent.keyDown(window, { key: '/' })
    expect(screen.getByLabelText('Message ops')).toHaveFocus()
    screen.getByLabelText('other').focus()
    fireEvent.keyDown(window, { key: '/' })
    expect(screen.getByLabelText('other')).toHaveFocus()
  })

  it('paused keeps Send visible but disabled, next to Go to request', () => {
    render(<Composer {...props({ mode: { kind: 'paused' } })} />)
    expect(screen.getByRole('button', { name: 'Send' })).toBeDisabled()
    expect(screen.getByRole('button', { name: 'Go to request' })).toBeInTheDocument()
  })

  it('streaming shows Stop receiving in place of Send', async () => {
    const p = props({ mode: { kind: 'streaming' } })
    render(<Composer {...p} />)
    await userEvent.click(screen.getByRole('button', { name: 'Stop receiving' }))
    expect(p.onStop).toHaveBeenCalledOnce()
    expect(screen.queryByRole('button', { name: 'Send' })).toBeNull()
  })
})

describe('pending request pager', () => {
  it('shows one request at a time with Request i of N', async () => {
    const turn = {
      key: 't',
      user: null,
      replies: [],
      requests: [
        request({ id: 'h1', question: { message: 'First?' } }),
        request({ id: 'h2', question: { message: 'Second?' } }),
      ],
    }
    const handlers = {
      viewTrace: vi.fn(),
      refresh: vi.fn(),
      runAgain: vi.fn(),
      saveAgain: vi.fn(),
      discard: vi.fn(),
      tryAgain: vi.fn(),
      editMessage: vi.fn(),
      requests: actions(),
    }
    await renderInApp(
      <TurnView
        turn={turn}
        latest
        agentName="ops"
        agentId={null}
        sessionId="s"
        now={0}
        status={null}
        handlers={handlers}
      />,
    )
    expect(screen.getByText('Request 1 of 2')).toBeInTheDocument()
    expect(screen.getByText('First?')).toBeInTheDocument()
    expect(screen.queryByText('Second?')).toBeNull()
    await userEvent.click(screen.getByRole('button', { name: 'Next' }))
    expect(screen.getByText('Second?')).toBeInTheDocument()
  })
})

describe('/ship review fixes', () => {
  const handlers = () => ({
    viewTrace: vi.fn(),
    refresh: vi.fn(),
    runAgain: vi.fn(),
    saveAgain: vi.fn(),
    discard: vi.fn(),
    tryAgain: vi.fn(),
    editMessage: vi.fn(),
    requests: actions(),
  })
  const reply = (id: string, content: string, timestamp: string) => ({
    id,
    session_id: 's',
    role: 'assistant' as const,
    content,
    timestamp,
  })

  it('a receipt and a reply with the same timestamp show the question first', async () => {
    const at = '2026-09-27T00:00:00Z'
    const turn = {
      key: 't',
      user: null,
      replies: [reply('m1', 'The reply', at)],
      requests: [
        request({
          id: 'h1',
          status: 'resolved',
          question: { message: 'The question?' },
          created_at: at,
          resolved_at: at,
        }),
      ],
    }
    await renderInApp(
      <TurnView
        turn={turn}
        latest
        agentName="ops"
        agentId={null}
        sessionId="s"
        now={0}
        status={null}
        handlers={handlers()}
      />,
    )
    const q = screen.getByTestId('request-receipt')
    const r = screen.getByText('The reply')
    expect(q).toHaveTextContent('The question?')
    expect(q.compareDocumentPosition(r) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy()
  })

  it('the pager keeps keyboard focus on Next at the last request', async () => {
    const turn = {
      key: 't',
      user: null,
      replies: [],
      requests: [
        request({ id: 'h1', question: { message: 'First?' } }),
        request({ id: 'h2', question: { message: 'Second?' } }),
      ],
    }
    await renderInApp(
      <TurnView
        turn={turn}
        latest
        agentName="ops"
        agentId={null}
        sessionId="s"
        now={0}
        status={null}
        handlers={handlers()}
      />,
    )
    const next = screen.getByRole('button', { name: 'Next' })
    await userEvent.click(next)
    expect(next).toHaveAttribute('aria-disabled', 'true')
    expect(next).not.toBeDisabled()
    expect(next).toHaveFocus()
    await userEvent.click(next)
    expect(screen.getByText('Request 2 of 2')).toBeInTheDocument()
  })

  it('a 403 without an agent id offers the Agents page', async () => {
    const live: LiveTurn = {
      id: 't',
      sessionId: 's',
      agentId: 'a',
      userText: 'hi',
      phase: 'error',
      state: emptyTurn(),
      startedAt: 0,
      idle: false,
      stopped: false,
      finalized: false,
      error: new ChatError({
        phase: 'dispatch',
        key: 'forbidden',
        certainty: 'rejected-before-run',
        status: 403,
      }),
      saved: null,
      pendingSave: null,
      frames: [],
      chatMode: 'direct',
      operation: 'send',
      attempt: 'send',
    }
    const ui = (
      <TurnView
        turn={{ key: 't', user: null, replies: [], requests: [], live }}
        latest
        agentName="ops"
        agentId={null}
        sessionId="s"
        now={0}
        status={null}
        handlers={handlers()}
      />
    )
    const router = createRouter({
      routeTree: createRootRoute({ component: () => ui }),
      history: createMemoryHistory({ initialEntries: ['/'] }),
    })
    render(
      <QueryClientProvider client={new QueryClient()}>
        <RouterProvider router={router} />
      </QueryClientProvider>,
    )
    expect(await screen.findByRole('link', { name: 'Agents' })).toHaveAttribute('href', '/agents')
    expect(screen.queryByRole('link', { name: 'Open the agent' })).toBeNull()
  })

  it('the announcer does not re-read an older message when an error clears', () => {
    const { rerender } = render(<StatusAnnouncer live={undefined} agentName="ops" />)
    act(() => announce('Rename failed'))
    expect(screen.getByRole('status')).toHaveTextContent('Rename failed')
    rerender(<StatusAnnouncer live={undefined} agentName="ops" error="Message too long" />)
    expect(screen.getByRole('status')).toHaveTextContent('Message too long')
    rerender(<StatusAnnouncer live={undefined} agentName="ops" error={null} />)
    expect(screen.getByRole('status')).toHaveTextContent('Message too long')
    expect(screen.getByRole('status')).not.toHaveTextContent('Rename failed')
  })

  it('the same announcement twice still changes the region, so it is spoken again', () => {
    render(<StatusAnnouncer live={undefined} agentName="ops" />)
    const region = screen.getByRole('status')
    act(() => announce('Submit failed'))
    const first = region.textContent
    act(() => announce('Submit failed'))
    expect(region.textContent).not.toBe(first)
    expect(region).toHaveTextContent('Submit failed')
  })

  it('a one-off and a turn outcome in the same render are both spoken', () => {
    const turn = (phase: LiveTurn['phase']): LiveTurn => ({
      id: 't',
      sessionId: 's',
      agentId: 'a',
      userText: 'x',
      phase,
      state: emptyTurn(),
      startedAt: 0,
      idle: false,
      stopped: false,
      finalized: false,
      error: null,
      saved: null,
      pendingSave: null,
      frames: [],
      chatMode: 'direct',
      operation: 'send',
      attempt: 'send',
    })
    const { rerender } = render(<StatusAnnouncer live={turn('streaming')} agentName="ops" />)
    act(() => {
      announce('Rename failed')
      rerender(<StatusAnnouncer live={turn('done')} agentName="ops" />)
    })
    expect(screen.getByRole('status')).toHaveTextContent('Rename failed. Reply from ops complete')
  })

  it('a modal speaks one-off announcements while it hides the page', () => {
    render(<ModalAnnouncer />)
    expect(screen.getByRole('status')).toHaveTextContent('')
    act(() => announce('Delete failed'))
    expect(screen.getByRole('status')).toHaveTextContent('Delete failed')
  })

  it('a draft comes back from storage after a reload', () => {
    const store = new Map<string, string>()
    vi.stubGlobal('localStorage', {
      getItem: (k: string) => store.get(k) ?? null,
      setItem: (k: string, v: string) => store.set(k, v),
      removeItem: (k: string) => store.delete(k),
      key: (i: number) => [...store.keys()][i] ?? null,
      get length() {
        return store.size
      },
    })
    try {
      store.set('ui-lab:chat-draft:u1:s1', 'saved before reload')
      expect(readDraft('u1', 's1')).toBe('saved before reload')
    } finally {
      vi.unstubAllGlobals()
    }
  })
})
