import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { act, fireEvent, render, screen, waitFor } from '@testing-library/react'
import {
  createMemoryHistory,
  createRootRoute,
  createRouter,
  RouterProvider,
} from '@tanstack/react-router'
import { http, HttpResponse } from 'msw'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { recordRequestBodies, server } from '@nasiko/testing/setup'
import { agentKeys } from '@nasiko/features/agents/api'
import { routerKeys } from '@nasiko/features/router/api'
import { clearSeededDeploys, setSeededDeploy } from '@nasiko/features/onboarding/api'
import { TryAgentRun } from './components/TryAgentRun'
import { copy } from './copy'
import {
  readDone,
  readFigures,
  savedPercent,
  showTryRun,
  TRY_RUN_AGENT,
  writeDone,
  writeFigures,
} from './tryRun'

const t = copy.tryRun
const AGENT_ID = '8f7421ed-1ebb-4f17-9170-f60f6a9c5595'

describe('try-a-run rules', () => {
  it('shows with no activity and between the runs, never in a workspace with spend', () => {
    expect(showTryRun([], 0)).toBe(true)
    expect(showTryRun([], 12)).toBe(false)
    expect(showTryRun([], null)).toBe(false)
    expect(showTryRun(['baseline'], 1)).toBe(true)
    expect(showTryRun(['baseline', 'optimized'], 12)).toBe(false)
    expect(showTryRun(['baseline', 'optimized'], null)).toBe(false)
    expect(showTryRun(['baseline', 'optimized'], 0)).toBe(true)
  })

  it('keeps finished runs and what they cost per viewer', () => {
    expect(readDone('u1')).toEqual([])
    writeDone('u1', 'baseline')
    writeFigures('u1', 'baseline', { tokens: 1560, costUsd: 0.000448 })
    expect(readDone('u1')).toEqual(['baseline'])
    expect(readFigures('u1')).toEqual({ baseline: { tokens: 1560, costUsd: 0.000448 } })
    expect(readDone('u2')).toEqual([])
    expect(readFigures('u2')).toEqual({})
  })

  it('says how much less the optimized run cost, only when it did cost less', () => {
    const before = { tokens: 1560, costUsd: 0.000448 }
    expect(savedPercent(before, { tokens: 1596, costUsd: 0.000351 })).toBe(22)
    expect(savedPercent(before, { tokens: 1600, costUsd: 0.0005 })).toBeNull()
    expect(savedPercent(undefined, before)).toBeNull()
    expect(savedPercent({ tokens: 0, costUsd: 0 }, before)).toBeNull()
  })
})

/** The frames the server streams for a direct run (A2A 1.0, as recorded from a real run), usage in a working status. */
function runStream(usage: { input: number; output: number; cost: number }) {
  const status = (state: string, parts?: unknown[]) => ({
    statusUpdate: {
      taskId: 't1',
      contextId: 'c1',
      status: {
        state,
        ...(parts ? { message: { messageId: 'm', role: 'ROLE_AGENT', parts } } : {}),
      },
    },
  })
  const frames = [
    status('TASK_STATE_WORKING'),
    {
      artifactUpdate: {
        taskId: 't1',
        contextId: 'c1',
        artifact: { artifactId: 'a1', parts: [{ text: '**Meeting notes** …' }] },
        append: false,
        lastChunk: true,
      },
    },
    status('TASK_STATE_WORKING', [
      {
        data: {
          type: 'usage_meta',
          input_tokens: usage.input,
          output_tokens: usage.output,
          total_tokens: usage.input + usage.output,
          cost_usd: usage.cost,
        },
      },
    ]),
    status('TASK_STATE_COMPLETED'),
  ]
  return new HttpResponse(frames.map((f) => `data: ${JSON.stringify(f)}\n\n`).join(''), {
    headers: { 'Content-Type': 'text/event-stream' },
  })
}

/** The chat a run is saved as: the session it opens and the rows saved into it, as the server answers them. */
function stubChat() {
  server.use(
    http.post('*/api/chat/sessions', async ({ request }) => {
      const body = (await request.json()) as { session_id: string; agent_id?: string }
      return HttpResponse.json({ data: { ...body, title: 'Meeting notes' } }, { status: 201 })
    }),
    http.post('*/api/chat/sessions/:id/messages', async ({ request }) => {
      const body = (await request.json()) as { role: string; content: string }
      return HttpResponse.json({ id: `m-${body.role}`, ...body }, { status: 201 })
    }),
  )
}

function renderCard(opts: { deployed?: boolean; onRan?: () => void } = {}) {
  const qc = new QueryClient({ defaultOptions: { queries: { retry: false, staleTime: Infinity } } })
  qc.setQueryData(
    agentKeys.directory,
    opts.deployed === false ? [] : [{ id: AGENT_ID, name: TRY_RUN_AGENT }],
  )
  const rootRoute = createRootRoute({
    component: () => <TryAgentRun userId="u1" onRan={opts.onRan} />,
  })
  const router = createRouter({
    routeTree: rootRoute,
    history: createMemoryHistory({ initialEntries: ['/'] }),
  })
  render(
    <QueryClientProvider client={qc}>
      <RouterProvider router={router} />
    </QueryClientProvider>,
  )
  return qc
}

describe('TryAgentRun', () => {
  beforeEach(() => stubChat())
  afterEach(() => {
    vi.restoreAllMocks()
    clearSeededDeploys()
  })

  it('runs the agent for real, shows what it cost, then runs it again with Token optimization on', async () => {
    const runs = [
      { input: 1085, output: 475, cost: 0.000448 },
      { input: 1178, output: 418, cost: 0.000351 },
    ]
    server.use(
      http.post('*/api/orchestrator/a2a', () => runStream(runs.shift()!)),
      http.put('*/api/agents/:id', ({ params }) =>
        HttpResponse.json({ id: params.id, name: TRY_RUN_AGENT, compress_enabled: true }),
      ),
    )
    const rec = recordRequestBodies()
    const onRan = vi.fn()
    renderCard({ onRan })

    fireEvent.click(await screen.findByRole('button', { name: t.baseline }))
    expect(
      await screen.findByText(
        /Your meeting-notes-processor completed the run: 1,560 tokens, \$0\.000448\./,
      ),
    ).toBeInTheDocument()
    expect(screen.getByText(t.updated)).toBeInTheDocument()
    expect(onRan).toHaveBeenCalledTimes(1)
    expect(screen.getByRole('link', { name: t.seeRun }).getAttribute('href')).toMatch(
      /^\/sessions\/[0-9a-f-]{36}$/,
    )
    expect(await screen.findByText(t.nextStep)).toBeInTheDocument()

    fireEvent.click(await screen.findByRole('button', { name: t.optimized }))
    expect(
      await screen.findByText(
        /with Token optimization on: 1,596 tokens, \$0\.000351\. 22% less than the run without it\./,
      ),
    ).toBeInTheDocument()
    expect(onRan).toHaveBeenCalledTimes(2)
    expect(readDone('u1')).toEqual(['baseline', 'optimized'])

    await rec.flush()
    const sends = rec.requests.filter((r) => r.url.pathname === '/api/orchestrator/a2a')
    expect(sends).toHaveLength(2)
    expect(sends[0]?.body).toMatchObject({
      method: 'message/stream',
      params: {
        message: { role: 'ROLE_USER', parts: [{ text: t.request }] },
        metadata: { agent_id: AGENT_ID },
      },
    })
    // Each run is saved as a chat with the agent: the session, the task, then the reply with what it cost.
    const sessions = rec.requests.filter((r) => r.url.pathname === '/api/chat/sessions')
    expect(sessions).toHaveLength(2)
    expect(sessions[0]?.body).toMatchObject({ agent_id: AGENT_ID, first_prompt: t.request })
    const saved = rec.requests.filter((r) => r.url.pathname.endsWith('/messages'))
    expect(saved.map((r) => (r.body as { role: string }).role)).toEqual([
      'user',
      'assistant',
      'user',
      'assistant',
    ])
    expect(saved[1]?.body).toMatchObject({
      role: 'assistant',
      content: '**Meeting notes** …',
      usage: { input_tokens: 1085, output_tokens: 475, cost_usd: 0.000448 },
    })
    // Token optimization is switched on before the second run, and only then.
    const puts = rec.requests.filter((r) => r.method === 'PUT')
    expect(puts.map((r) => [r.url.pathname, r.body])).toEqual([
      [`/api/agents/${AGENT_ID}`, { compress_enabled: true }],
    ])
  })

  it('says why a run failed and offers it again', async () => {
    server.use(
      http.post(
        '*/api/orchestrator/a2a',
        () => new HttpResponse('agent unavailable', { status: 502 }),
      ),
    )
    renderCard()
    fireEvent.click(await screen.findByRole('button', { name: t.baseline }))
    // The server's reason, not the status and raw body (NAS-1624).
    expect(await screen.findByText(/The run didn't finish: agent unavailable/)).toBeInTheDocument()
    expect(screen.getByRole('button', { name: t.retry })).toBeEnabled()
    expect(readDone('u1')).toEqual([])
  })

  it('says the JSON-RPC reason of a refused run, never the raw JSON', async () => {
    server.use(
      http.post('*/api/orchestrator/a2a', () =>
        HttpResponse.json(
          { jsonrpc: '2.0', id: null, error: { code: -32603, message: 'OPENAI_API_KEY not set' } },
          { status: 500 },
        ),
      ),
    )
    renderCard()
    fireEvent.click(await screen.findByRole('button', { name: t.baseline }))
    expect(
      await screen.findByText(/The run didn't finish: OPENAI_API_KEY not set/),
    ).toBeInTheDocument()
    expect(screen.queryByText(/jsonrpc/)).not.toBeInTheDocument()
  })

  it('waits for the agent to be deployed', async () => {
    renderCard({ deployed: false })
    expect(await screen.findByText(t.notDeployed(TRY_RUN_AGENT))).toBeInTheDocument()
    expect(screen.getByRole('button', { name: t.baseline })).toBeDisabled()
  })

  it('says its agent is deploying until the Setup guide deploy runs, then offers the run (NAS-1450)', async () => {
    const title = 'Meeting Notes Processor'
    setSeededDeploy(TRY_RUN_AGENT, { phase: 'deploying', title, startedAt: Date.now() })
    // The row exists seconds into the import, minutes before the agent can run.
    renderCard()
    expect(
      await screen.findByRole('heading', { name: t.deployingTitle(TRY_RUN_AGENT) }),
    ).toBeInTheDocument()
    expect(screen.getByRole('button', { name: t.deploying })).toBeDisabled()
    expect(screen.getByText(t.deployingHint)).toBeInTheDocument()
    expect(screen.queryByRole('button', { name: t.baseline })).toBeNull()
    act(() => setSeededDeploy(TRY_RUN_AGENT, { phase: 'running', title, agentId: AGENT_ID }))
    expect(await screen.findByRole('button', { name: t.baseline })).toBeEnabled()
    expect(screen.getByRole('heading', { name: t.title(TRY_RUN_AGENT) })).toBeInTheDocument()
  })

  it('says why the Setup guide could not deploy its agent (NAS-1450)', async () => {
    setSeededDeploy(TRY_RUN_AGENT, {
      phase: 'failed',
      title: 'Meeting Notes Processor',
      reason: 'cannot reach registry',
    })
    renderCard({ deployed: false })
    expect(
      await screen.findByText(t.deployFailed(TRY_RUN_AGENT, 'cannot reach registry')),
    ).toBeInTheDocument()
    expect(screen.getByRole('link', { name: t.openAgents })).toHaveAttribute('href', '/agents')
    // Not "deploy it from the Setup guide": that is what just failed.
    expect(screen.queryByText(t.notDeployed(TRY_RUN_AGENT))).toBeNull()
  })

  it('offers the optimized run after a refresh once the first run is done', async () => {
    writeDone('u1', 'baseline')
    renderCard()
    // Disabled until the LLM router configs answer.
    await waitFor(() => expect(screen.getByRole('button', { name: t.optimized })).toBeEnabled())
    expect(screen.queryByRole('button', { name: t.baseline })).toBeNull()
    expect(screen.getByText(t.nextStep)).toBeInTheDocument()
  })

  it('asks for the LLM router before the optimized run, and offers the run once one is set up', async () => {
    writeDone('u1', 'baseline')
    server.use(http.get('*/api/llm-configs', () => HttpResponse.json({ data: [] })))
    const qc = renderCard()
    expect(await screen.findByText(t.needsRouter)).toBeInTheDocument()
    expect(screen.getByRole('link', { name: t.setUpRouter })).toHaveAttribute('href', '/router')
    expect(screen.queryByRole('button', { name: t.optimized })).toBeNull()
    expect(screen.queryByText(t.nextStep)).toBeNull()

    // A config saved on /router invalidates the list, as the router page's save does.
    server.use(
      http.get('*/api/llm-configs', () =>
        HttpResponse.json({ data: [{ id: 'c1', name: 'default' }] }),
      ),
    )
    await act(() => qc.invalidateQueries({ queryKey: routerKeys.configs }))
    expect(await screen.findByRole('button', { name: t.optimized })).toBeEnabled()
    expect(screen.getByText(t.nextStep)).toBeInTheDocument()
    expect(screen.queryByRole('link', { name: t.setUpRouter })).toBeNull()
  })

  it('holds Starting your agent until the server answers, then completes', async () => {
    let release: (() => void) | null = null
    server.use(
      http.post('*/api/orchestrator/a2a', async () => {
        await new Promise<void>((r) => (release = r))
        return runStream({ input: 10, output: 5, cost: 0.00001 })
      }),
    )
    renderCard()
    fireEvent.click(await screen.findByRole('button', { name: t.baseline }))
    expect(screen.getByRole('button', { name: t.starting })).toBeDisabled()
    // Held until the request reaches the server, then answered.
    await waitFor(() => expect(release).not.toBeNull())
    release!()
    await waitFor(() =>
      expect(screen.getByRole('button', { name: t.completed })).toBeInTheDocument(),
    )
  })
})
