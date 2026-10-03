/**
 * v1b routed turns in the registry (plan §5.2-§5.4, §7 registry tests). Fakes only (plus the
 * mock's own flow rows, through MSW); the page tests drive the same paths through MSW.
 */
import { QueryClient } from '@tanstack/react-query'
import { http, HttpResponse } from 'msw'
import { afterEach, describe, expect, it, vi } from 'vitest'
import { configureChatMock, resetChatMock } from '@/mocks/chatStore'
import { configureMocks } from '@/mocks/handlers'
import { ApiError } from '@/lib/api/client'
import { readSse } from '@/lib/sse'
import {
  CHAT_SCENARIOS,
  ROUTED_TRACE,
  artifact,
  encodeFrame,
  sseResponse,
  status,
  toolCall,
  toolResult,
  traceMeta,
  usageMeta,
  type MockFrame,
} from '@/mocks/chat'
import { gatedSseResponse } from '@/test/gatedSse'
import { recordRequestBodies, server } from '@/test/setup'
import { chatRegistry, clearChatRegistry, disposeForHotReload } from './registry'
import { tuning } from './tuning'
import {
  createTurnRegistry,
  isLivePhase,
  routedSettles,
  stepRouted,
  type ChatDeps,
  type Phase,
  type RoutedEvent,
  type TurnEnd,
  type TurnRegistry,
} from './turnRegistry'
import type { ChatMessage, ChatSessionRow, SaveMessageBody } from './types'

const TRACE = '5eedf000000000000000000000000001'
const withTrace = (frames: readonly MockFrame[], traceId = TRACE): MockFrame[] =>
  frames.map((f) =>
    f.data === undefined
      ? f
      : {
          ...f,
          data: JSON.parse(JSON.stringify(f.data).replaceAll(ROUTED_TRACE, traceId)) as unknown,
        },
  )

function lockManager() {
  const held = new Set<string>()
  return {
    request(
      name: string,
      _opts: { ifAvailable: boolean },
      cb: (lock: { name: string } | null) => unknown,
    ) {
      if (held.has(name)) return Promise.resolve(cb(null))
      held.add(name)
      return Promise.resolve(cb({ name })).finally(() => held.delete(name))
    },
  } as unknown as Pick<LockManager, 'request'>
}

const made: TurnRegistry[] = []

function setup(
  over: Partial<ChatDeps> = {},
  frames: MockFrame[] = withTrace(CHAT_SCENARIOS['routed-plain']),
) {
  let n = 0
  const saved: { sessionId: string; body: SaveMessageBody }[] = []
  const ends: string[] = []
  const deps: ChatDeps = {
    userId: 'u-1',
    createSession: vi.fn(
      async (b) =>
        ({
          session_id: b.session_id,
          agent_id: b.agent_id ?? null,
          agent_url: null,
          title: 'T',
          created_at: '',
        }) as ChatSessionRow,
    ),
    saveMessage: vi.fn(async (sessionId: string, body: SaveMessageBody) => {
      saved.push({ sessionId, body })
      return {
        id: `m-${saved.length}`,
        session_id: sessionId,
        role: body.role,
        content: body.content,
        timestamp: '',
      } as ChatMessage
    }),
    dispatch: vi.fn(async () => sseResponse(frames)),
    replyExists: vi.fn(async () => false),
    onUnauthorized: vi.fn(),
    newId: () => `id-${++n}`,
    now: () => 1000 + n,
    locks: lockManager(),
    warn: vi.fn(),
    onTurnEnd: vi.fn((e) => void ends.push(e.kind)),
    ...over,
  }
  const reg = createTurnRegistry(deps)
  made.push(reg)
  return { deps, saved, ends, reg }
}

const ended = (reg: TurnRegistry, id: string) =>
  vi.waitFor(
    () => {
      const t = reg.get(id)
      if (!t || isLivePhase(t.phase)) throw new Error(`still ${t?.phase}`)
    },
    { timeout: 2000, interval: 1 },
  )

const bodyOf = (deps: ChatDeps, call = 0) =>
  (deps.dispatch as ReturnType<typeof vi.fn>).mock.calls[call]![0] as {
    params: { message: { contextId: string; parts: unknown[] }; metadata: Record<string, unknown> }
  }

afterEach(() => {
  // Each test's own registries too: their timers and hanging dispatches must not outlive it.
  for (const reg of made.splice(0)) reg.clearAll()
  vi.useRealTimers()
  resetChatMock()
  clearChatRegistry()
})

describe('stepRouted: the §5.4 state table', () => {
  const E = {
    createFailed: { type: 'create_failed' },
    rejected: { type: 'dispatch_rejected', resume: false, status: 400 },
    rejected403r: { type: 'dispatch_rejected', resume: true, status: 403 },
    rejected429r: { type: 'dispatch_rejected', resume: true, status: 429 },
    unknown: { type: 'dispatch_unknown', resume: false },
    unknownNetR: { type: 'dispatch_unknown', resume: true },
    unknownR: { type: 'dispatch_unknown', resume: true },
    frame: { type: 'frame' },
    limit: { type: 'limit', resume: false },
    limitR: { type: 'limit', resume: true },
    text: { type: 'terminal', text: true },
    empty: { type: 'terminal', text: false },
    failed: { type: 'task_failed' },
    pause: { type: 'pause' },
    truncated: { type: 'truncated' },
    eof: { type: 'eof', resume: false },
    eofR: { type: 'eof', resume: true },
    ceiling: { type: 'ceiling' },
    match: { type: 'history', result: 'match', hadText: true },
    noMatchText: { type: 'history', result: 'no_match', hadText: true },
    noMatchEmpty: { type: 'history', result: 'no_match', hadText: false },
    histErr: { type: 'history', result: 'error', hadText: true },
    abort: { type: 'abort' },
    timeoutText: { type: 'history', result: 'timeout', hadText: true },
    timeoutEmpty: { type: 'history', result: 'timeout', hadText: false },
  } satisfies Record<string, RoutedEvent>
  type Row = [Phase, keyof typeof E, Phase | 'settled']
  const rows: Row[] = [
    ['creating', 'createFailed', 'not_started'],
    ['saving_user', 'createFailed', 'not_started'],
    ['creating', 'abort', 'aborted'],
    ['creating', 'frame', 'creating'],
    ['waiting', 'rejected', 'not_started'],
    ['waiting', 'rejected403r', 'resume_forbidden'],
    ['waiting', 'rejected429r', 'resume_uncertain'],
    ['waiting', 'unknown', 'lost'],
    ['waiting', 'unknownR', 'resume_uncertain'],
    ['waiting', 'unknownNetR', 'resume_uncertain'],
    ['waiting', 'frame', 'streaming'],
    ['waiting', 'eof', 'lost'],
    ['waiting', 'eofR', 'resume_uncertain'],
    ['waiting', 'abort', 'aborted'],
    ['streaming', 'frame', 'streaming'],
    ['streaming', 'limit', 'draining'],
    ['streaming', 'limitR', 'loading_saved'],
    ['streaming', 'text', 'done'],
    ['streaming', 'empty', 'known_empty'],
    ['streaming', 'failed', 'error'],
    ['streaming', 'pause', 'paused'],
    ['streaming', 'truncated', 'loading_saved'],
    ['streaming', 'eof', 'lost'],
    ['streaming', 'eofR', 'resume_uncertain'],
    ['streaming', 'abort', 'aborted'],
    ['draining', 'frame', 'draining'],
    ['draining', 'text', 'loading_saved'],
    ['draining', 'empty', 'loading_saved'],
    ['draining', 'failed', 'loading_saved'],
    ['draining', 'pause', 'loading_saved'],
    ['draining', 'eof', 'loading_saved'],
    ['draining', 'ceiling', 'loading_saved'],
    ['draining', 'abort', 'aborted'],
    ['done', 'match', 'settled'],
    ['done', 'noMatchText', 'done'],
    ['done', 'timeoutText', 'lost'],
    ['done', 'histErr', 'history_failed'],
    ['done', 'abort', 'done'],
    ['loading_saved', 'match', 'settled'],
    ['loading_saved', 'noMatchText', 'loading_saved'],
    ['loading_saved', 'timeoutText', 'lost'],
    ['loading_saved', 'timeoutEmpty', 'known_empty'],
    ['loading_saved', 'histErr', 'history_failed'],
    ['history_failed', 'match', 'settled'],
    ['history_failed', 'noMatchText', 'loading_saved'],
    ['history_failed', 'timeoutText', 'lost'],
    ['history_failed', 'histErr', 'history_failed'],
    ['paused', 'abort', 'aborted'],
    ['paused', 'match', 'paused'],
    ['known_empty', 'match', 'settled'],
    ['known_empty', 'noMatchEmpty', 'known_empty'],
    ['lost', 'match', 'settled'],
    ['error', 'match', 'settled'],
    ['resume_uncertain', 'match', 'settled'],
    ['resume_forbidden', 'noMatchText', 'resume_forbidden'],
    ['not_started', 'match', 'not_started'],
  ]
  it.each(rows)('%s + %s → %s', (from, ev, to) => {
    expect(stepRouted(from, E[ev])).toBe(to)
  })
})

describe('routed send (§5.2)', () => {
  it('creates with no agent, saves only the user row, dispatches with contextId == session id', async () => {
    const { reg, deps, saved, ends } = setup()
    const t = await reg.send({ chatMode: 'routed', text: 'what happened last week?' })
    await ended(reg, t.sessionId)
    expect(deps.createSession).toHaveBeenCalledWith(
      { session_id: t.sessionId, first_prompt: 'what happened last week?' },
      expect.any(AbortSignal),
    )
    expect(saved.map((s) => s.body.role)).toEqual(['user'])
    const body = bodyOf(deps)
    expect(body.params.metadata).toEqual({ session_id: t.sessionId })
    expect(body.params.message.contextId).toBe(t.sessionId)
    expect(reg.get(t.sessionId)).toMatchObject({
      phase: 'done',
      chatMode: 'routed',
      operation: 'send',
      agentId: null,
    })
    expect(ends).toEqual(['reply'])
  })

  it('never saves the assistant row, even when the row claims the client owns it (G-4)', async () => {
    const { reg, saved } = setup()
    const t = await reg.send({
      chatMode: 'routed',
      text: 'x',
      sessionId: 's-1',
      ...({ transcript: { user: 'client', assistant: 'client' } } as object),
    })
    await ended(reg, t.sessionId)
    expect(saved.filter((s) => s.body.role === 'assistant')).toEqual([])
  })

  it('an empty completion is known-empty and records it; sub_content alone is not a reply (Q2)', async () => {
    for (const sc of ['routed-empty', 'routed-sub-content-only'] as const) {
      const { reg, saved, ends } = setup({}, withTrace(CHAT_SCENARIOS[sc]))
      const t = await reg.send({ chatMode: 'routed', text: 'x' })
      await ended(reg, t.sessionId)
      expect(reg.get(t.sessionId)?.phase).toBe('known_empty')
      // It finished: a new send doesn't need the "may still be running" confirm.
      expect(reg.isExecutionUnknown(t.sessionId)).toBe(false)
      expect(ends).toEqual(['empty'])
      expect(saved.map((s) => s.body.role)).toEqual(['user'])
    }
  })

  it('TASK_STATE_FAILED is failed with the server text; EOF without a terminal is lost', async () => {
    const failed = setup({}, withTrace(CHAT_SCENARIOS['routed-failed']))
    const t1 = await failed.reg.send({ chatMode: 'routed', text: 'x' })
    await ended(failed.reg, t1.sessionId)
    expect(failed.reg.get(t1.sessionId)).toMatchObject({
      phase: 'error',
      error: { key: 'routedFailed', serverDetail: 'orchestrator error: model unavailable' },
    })
    const cut = setup(
      {},
      withTrace([{ data: traceMeta(ROUTED_TRACE) }, { data: artifact('half') }]),
    )
    const t2 = await cut.reg.send({ chatMode: 'routed', text: 'x' })
    await ended(cut.reg, t2.sessionId)
    expect(cut.reg.get(t2.sessionId)).toMatchObject({ phase: 'lost', error: { key: 'cutOff' } })
    expect(cut.ends).toEqual(['error'])
  })

  it('pre-stream refusals are not-started with the routed copy; a 500 is lost and uncertain', async () => {
    for (const [code, key, phase] of [
      [400, 'routedBadRequest', 'not_started'],
      [429, 'routedRateLimited', 'not_started'],
      [503, 'routedNoAgents', 'not_started'],
      [500, 'routedInternal', 'lost'],
    ] as const) {
      const { reg, ends } = setup({
        dispatch: vi.fn(async () =>
          HttpResponse.json(
            { jsonrpc: '2.0', id: null, error: { code: -32603, message: 'm' } },
            { status: code },
          ),
        ),
      })
      const t = await reg.send({ chatMode: 'routed', text: 'x' })
      await ended(reg, t.sessionId)
      expect(reg.get(t.sessionId)).toMatchObject({ phase, error: { key } })
      expect(ends).toEqual([phase === 'lost' ? 'error' : 'not_started'])
    }
  })

  it('a create failure is not-started and gives the text back', async () => {
    const onCreateFailed = vi.fn()
    const { reg } = setup({
      createSession: vi.fn(async () => {
        throw new Error('boom')
      }),
    })
    const t = await reg.send({ chatMode: 'routed', text: 'keep me', onCreateFailed })
    await ended(reg, t.sessionId)
    expect(reg.get(t.sessionId)).toMatchObject({
      phase: 'not_started',
      error: { key: 'createFailed' },
    })
    expect(onCreateFailed).toHaveBeenCalledWith(t.sessionId, 'keep me')
  })

  it('a create slower than CREATE_TIMEOUT_MS gives up and keeps the text (NE-7)', async () => {
    vi.useFakeTimers()
    const onCreateFailed = vi.fn()
    const { reg } = setup({
      createSession: vi.fn(
        (_b, signal: AbortSignal) =>
          new Promise<ChatSessionRow>((_r, reject) =>
            signal.addEventListener('abort', () =>
              reject(new DOMException('Aborted', 'AbortError')),
            ),
          ),
      ),
    })
    const t = await reg.send({ chatMode: 'routed', text: 'slow', onCreateFailed })
    await vi.advanceTimersByTimeAsync(tuning.CREATE_TIMEOUT_MS + 1)
    expect(reg.get(t.sessionId)?.phase).toBe('not_started')
    expect(onCreateFailed).toHaveBeenCalledWith(t.sessionId, 'slow')
  })

  it('never stops a routed turn (no Stop, DS3)', async () => {
    const gate = gatedSseResponse(withTrace(CHAT_SCENARIOS['routed-plain']))
    const { reg } = setup({ dispatch: vi.fn(async () => gate.response) })
    const t = await reg.send({ chatMode: 'routed', text: 'x', sessionId: 's-1' })
    await vi.waitFor(() => expect(reg.get('s-1')?.phase).toBe('waiting'))
    await gate.release(2)
    reg.stop(t.sessionId)
    await gate.releaseAll()
    await ended(reg, 's-1')
    expect(reg.get('s-1')).toMatchObject({ phase: 'done', stopped: false })
    expect(gate.cancelled).toBe(false)
  })

  it('a first send whose dispatch never gets a response is lost and may have run', async () => {
    const { reg, ends } = setup({
      dispatch: vi.fn(async () => {
        throw new TypeError('Failed to fetch')
      }),
    })
    const t = await reg.send({ chatMode: 'routed', text: 'hi' })
    await ended(reg, t.sessionId)
    expect(reg.get(t.sessionId)).toMatchObject({
      phase: 'lost',
      error: { key: 'cutOff', certainty: 'unknown' },
    })
    expect(reg.isExecutionUnknown(t.sessionId)).toBe(true)
    expect(ends).toEqual(['error'])
  })

  it('a failed user-row save is not-started, and nothing is dispatched (§2.3)', async () => {
    const { reg, deps } = setup({
      saveMessage: vi.fn(async () => {
        throw new ApiError(500, 'save failed', '/api/chat/sessions/s-1/messages', 'save failed')
      }),
    })
    const t = await reg.send({ chatMode: 'routed', text: 'hi', sessionId: 's-1' })
    await ended(reg, 's-1')
    expect(t.sessionId).toBe('s-1')
    expect(reg.get('s-1')).toMatchObject({ phase: 'not_started', error: { phase: 'save-user' } })
    expect(deps.dispatch).not.toHaveBeenCalled()
  })
})

describe('drain (§5.3, EN-1, NE-10)', () => {
  it('an oversized first-send stream drains to EOF without cancelling, then loads the saved reply', async () => {
    const cap = tuning.MAX_TURN_BYTES
    tuning.MAX_TURN_BYTES = 100_000
    try {
      const gate = gatedSseResponse(withTrace(CHAT_SCENARIOS['routed-oversized']))
      const { reg } = setup({ dispatch: vi.fn(async () => gate.response) })
      const t = await reg.send({ chatMode: 'routed', text: 'x' })
      await vi.waitFor(() => expect(reg.get(t.sessionId)?.phase).toBe('waiting'))
      await gate.release(3)
      await vi.waitFor(() => expect(reg.get(t.sessionId)?.phase).toBe('draining'))
      expect(reg.busy()).toBe(true)
      await gate.releaseAll()
      await ended(reg, t.sessionId)
      expect(gate.cancelled).toBe(false)
      expect(reg.get(t.sessionId)?.phase).toBe('loading_saved')
      // The text parsed before the limit stays until the saved row replaces it.
      expect(reg.get(t.sessionId)?.state.artifacts[0]?.text).toMatch(/^x+$/)
      reg.reconcile(t.sessionId, 'match')
      expect(reg.get(t.sessionId)).toBeUndefined()
    } finally {
      tuning.MAX_TURN_BYTES = cap
    }
  })

  it('a first chunk already over the cap still drains and loads the saved reply', async () => {
    const cap = tuning.MAX_TURN_BYTES
    tuning.MAX_TURN_BYTES = 1_000
    try {
      const { reg } = setup(
        {},
        withTrace([
          { data: artifact('q'.repeat(5_000)) },
          ...CHAT_SCENARIOS['routed-plain'].slice(-2),
        ]),
      )
      const t = await reg.send({ chatMode: 'routed', text: 'x' })
      await ended(reg, t.sessionId)
      expect(reg.get(t.sessionId)?.phase).toBe('loading_saved')
    } finally {
      tuning.MAX_TURN_BYTES = cap
    }
  })

  it('too many unreadable frames drain a first send instead of failing it', async () => {
    const gate = gatedSseResponse(withTrace(CHAT_SCENARIOS['routed-malformed']))
    const { reg } = setup({ dispatch: vi.fn(async () => gate.response) })
    const t = await reg.send({ chatMode: 'routed', text: 'x' })
    await vi.waitFor(() => expect(reg.get(t.sessionId)?.phase).toBe('waiting'))
    await gate.releaseAll()
    await ended(reg, t.sessionId)
    expect(gate.cancelled).toBe(false)
    expect(reg.get(t.sessionId)?.phase).toBe('loading_saved')
  })

  it('the drain ceiling cancels and loads the saved reply', async () => {
    const ceiling = tuning.DRAIN_CEILING_MS
    const cap = tuning.MAX_TURN_BYTES
    tuning.DRAIN_CEILING_MS = 30
    tuning.MAX_TURN_BYTES = 100_000
    try {
      const gate = gatedSseResponse(withTrace(CHAT_SCENARIOS['routed-oversized']))
      const { reg } = setup({ dispatch: vi.fn(async () => gate.response) })
      const t = await reg.send({ chatMode: 'routed', text: 'x' })
      await vi.waitFor(() => expect(reg.get(t.sessionId)?.phase).toBe('waiting'))
      await gate.release(3)
      await ended(reg, t.sessionId)
      expect(reg.get(t.sessionId)?.phase).toBe('loading_saved')
      expect(gate.cancelled).toBe(true)
    } finally {
      tuning.DRAIN_CEILING_MS = ceiling
      tuning.MAX_TURN_BYTES = cap
    }
  })

  it('a drained first send that keeps trickling bytes past STREAM_IDLE_MS never shows idle', async () => {
    const saved = {
      idle: tuning.STREAM_IDLE_MS,
      cap: tuning.MAX_TURN_BYTES,
      ceiling: tuning.DRAIN_CEILING_MS,
    }
    tuning.STREAM_IDLE_MS = 40
    tuning.MAX_TURN_BYTES = 2_000
    tuning.DRAIN_CEILING_MS = 60_000
    const enc = new TextEncoder()
    let controller!: ReadableStreamDefaultController<Uint8Array>
    const body = new ReadableStream<Uint8Array>({
      start(c) {
        controller = c
      },
    })
    const send = (f: MockFrame) => controller.enqueue(enc.encode(encodeFrame(f)))
    // Fake time: the idle windows pass exactly as advanced, however busy the machine is.
    vi.useFakeTimers()
    try {
      const { reg } = setup({
        dispatch: vi.fn(
          async () =>
            new Response(body, { status: 200, headers: { 'Content-Type': 'text/event-stream' } }),
        ),
      })
      const t = await reg.send({ chatMode: 'routed', text: 'big' })
      await vi.waitFor(() => expect(reg.get(t.sessionId)?.phase).toBe('waiting'))
      send({ data: traceMeta(TRACE) })
      await vi.waitFor(() => expect(reg.get(t.sessionId)?.phase).toBe('streaming'), { interval: 1 })
      // Over the cap: the drain starts.
      send({ data: artifact('x'.repeat(5_000)) })
      await vi.waitFor(() => expect(reg.get(t.sessionId)?.phase).toBe('draining'), { interval: 1 })
      expect(reg.get(t.sessionId)?.idle).toBe(false)
      let sawIdle = false
      const unsubscribe = reg.subscribe(() => {
        if (reg.get(t.sessionId)?.idle) sawIdle = true
      })
      // A busy stream: bytes keep coming for several idle windows, none of them decoded.
      for (let i = 0; i < 20; i++) {
        await vi.advanceTimersByTimeAsync(10)
        send({ data: artifact('y', { append: true }) })
      }
      expect(reg.get(t.sessionId)?.phase).toBe('draining')
      expect(reg.get(t.sessionId)?.idle).toBe(false)
      send({ data: usageMeta({ duration_ms: 1, trace_id: TRACE }) })
      send({ data: status('TASK_STATE_COMPLETED') })
      controller.close()
      await ended(reg, t.sessionId)
      expect(reg.get(t.sessionId)?.phase).toBe('loading_saved')
      // And once more after the idle window, now that nothing is live.
      await vi.advanceTimersByTimeAsync(tuning.STREAM_IDLE_MS * 2)
      unsubscribe()
      expect(sawIdle).toBe(false)
      expect(reg.get(t.sessionId)?.idle).toBe(false)
    } finally {
      tuning.STREAM_IDLE_MS = saved.idle
      tuning.MAX_TURN_BYTES = saved.cap
      tuning.DRAIN_CEILING_MS = saved.ceiling
    }
  })
})

describe('settle and reconcile (§5.4, EN-2, EN-5)', () => {
  const reply = (trace: string | null): Pick<ChatMessage, 'role' | 'trace_id'> => ({
    role: 'assistant',
    trace_id: trace,
  })

  it('settles on the reply with its own trace id; an older reply never settles a rerun', () => {
    const t = { state: { traceId: TRACE } } as Parameters<typeof routedSettles>[0]
    expect(routedSettles(t, [reply('older')], 1)).toBe(false)
    expect(routedSettles(t, [reply('older'), reply(TRACE)], 1)).toBe(true)
    // No trace id: any reply newer than the attempt's baseline.
    const noTrace = { state: { traceId: null } } as Parameters<typeof routedSettles>[0]
    expect(routedSettles(noTrace, [reply(null)], 0)).toBe(false)
    expect(routedSettles(noTrace, [reply(null)], 1)).toBe(true)
  })

  it('a delayed save keeps the turn until history has it; its steps survive the forget by trace id (R7)', async () => {
    const { reg } = setup()
    const t = await reg.send({ chatMode: 'routed', text: 'x', sessionId: 's-1' })
    await ended(reg, 's-1')
    reg.reconcile('s-1', 'no_match')
    expect(reg.get('s-1')?.phase).toBe('done')
    reg.reconcile('s-1', 'match')
    expect(reg.get(t.sessionId)).toBeUndefined()
    expect(reg.stepsFor('saved-by-server', TRACE)?.map((s) => s.name)).toEqual(['@agent-1@'])
  })

  it('a late saved reply earns one signals-only reply end; a settled reply adds none (v1c E3)', async () => {
    const late = setup()
    const got: TurnEnd[] = []
    late.reg.subscribeEnds((e) => got.push(e))
    const t = await late.reg.send({ chatMode: 'routed', text: 'x', sessionId: 's-1' })
    await ended(late.reg, 's-1')
    late.reg.reconcile('s-1', 'no_match')
    late.reg.reconcile('s-1', 'timeout')
    expect(late.reg.get('s-1')?.phase).toBe('lost')
    const before = got.length
    const endFor = late.reg.endFor('s-1', t.userMessageId!)
    const recorded = [...late.ends]
    late.reg.reconcile('s-1', 'match')
    expect(got.slice(before).map((e) => [e.kind, e.chatMode])).toEqual([['reply', 'routed']])
    // The page's outcome and onTurnEnd are unchanged (EN-5, NE-4).
    expect(late.reg.endFor('s-1', t.userMessageId!)).toEqual(endFor)
    expect(late.ends).toEqual(recorded)

    const plain = setup()
    const got2: TurnEnd[] = []
    plain.reg.subscribeEnds((e) => got2.push(e))
    await plain.reg.send({ chatMode: 'routed', text: 'x', sessionId: 's-1' })
    await ended(plain.reg, 's-1')
    const n = got2.length
    plain.reg.reconcile('s-1', 'match')
    expect(got2.length).toBe(n)
  })

  it('a refetch error is history-failed; a later refetch keeps checking, and giving up is unconfirmed', async () => {
    const { reg } = setup()
    await reg.send({ chatMode: 'routed', text: 'x', sessionId: 's-1' })
    await ended(reg, 's-1')
    reg.reconcile('s-1', 'error')
    expect(reg.get('s-1')?.phase).toBe('history_failed')
    reg.reconcile('s-1', 'no_match')
    expect(reg.get('s-1')?.phase).toBe('loading_saved')
    reg.reconcile('s-1', 'timeout')
    expect(reg.get('s-1')?.phase).toBe('lost')
  })

  it('a complete reply the server never saves ends unconfirmed, not "saved" forever', async () => {
    const { reg } = setup()
    await reg.send({ chatMode: 'routed', text: 'x', sessionId: 's-1' })
    await ended(reg, 's-1')
    reg.reconcile('s-1', 'no_match')
    expect(reg.get('s-1')?.phase).toBe('done')
    reg.reconcile('s-1', 'timeout')
    expect(reg.get('s-1')?.phase).toBe('lost')
  })

  it('a drained turn with no parsed text is never recorded as "no reply" (hadText from incomplete)', async () => {
    const cap = tuning.MAX_TURN_BYTES
    tuning.MAX_TURN_BYTES = 1_000
    try {
      const { reg, ends } = setup(
        {},
        withTrace([
          { data: traceMeta(ROUTED_TRACE) },
          { data: artifact('q'.repeat(5_000)) },
          ...CHAT_SCENARIOS['routed-plain'].slice(-2),
        ]),
      )
      const t = await reg.send({ chatMode: 'routed', text: 'x' })
      await ended(reg, t.sessionId)
      expect(reg.get(t.sessionId)).toMatchObject({ phase: 'loading_saved', incomplete: true })
      reg.reconcile(t.sessionId, 'timeout')
      expect(reg.get(t.sessionId)?.phase).toBe('lost')
      expect(ends).not.toContain('empty')
    } finally {
      tuning.MAX_TURN_BYTES = cap
    }
  })

  it('settling clears the "may run twice" flag', async () => {
    vi.useFakeTimers({ toFake: ['setTimeout', 'clearTimeout'] })
    const gate = gatedSseResponse(withTrace(CHAT_SCENARIOS['routed-plain']))
    const { reg } = setup({ dispatch: vi.fn(async () => gate.response) })
    await reg.send({ chatMode: 'routed', text: 'x', sessionId: 's-1' })
    await vi.advanceTimersByTimeAsync(tuning.STREAM_IDLE_MS + 1)
    expect(reg.isExecutionUnknown('s-1')).toBe(true)
    vi.useRealTimers()
    await gate.releaseAll()
    await ended(reg, 's-1')
    reg.reconcile('s-1', 'match')
    expect(reg.isExecutionUnknown('s-1')).toBe(false)
  })

  it('an empty ending survives the forget and a following send; a matching reply clears it', async () => {
    const { reg } = setup({}, withTrace(CHAT_SCENARIOS['routed-empty']))
    const t = await reg.send({ chatMode: 'routed', text: 'x', sessionId: 's-1' })
    await ended(reg, 's-1')
    const userMessageId = reg.get('s-1')!.userMessageId!
    reg.forget('s-1')
    expect(reg.endFor('s-1', userMessageId)?.kind).toBe('empty')
    await reg.send({ chatMode: 'routed', text: 'next', sessionId: t.sessionId })
    await ended(reg, 's-1')
    expect(reg.endFor('s-1', userMessageId)?.kind).toBe('empty')
    // A known-empty turn whose reply later shows up (another tab, a late save) is settled and cleared.
    const again = setup({}, withTrace(CHAT_SCENARIOS['routed-empty']))
    await again.reg.send({ chatMode: 'routed', text: 'x', sessionId: 's-2' })
    await ended(again.reg, 's-2')
    const uid = again.reg.get('s-2')!.userMessageId!
    again.reg.reconcile('s-2', 'match')
    expect(again.reg.endFor('s-2', uid)).toBeUndefined()
  })

  it('the turn-end store keeps the newest 200 and clears with the registry', async () => {
    const { reg } = setup({
      dispatch: vi.fn(async () =>
        HttpResponse.json(
          { jsonrpc: '2.0', id: null, error: { code: -32602, message: 'm' } },
          { status: 400 },
        ),
      ),
    })
    for (let i = 0; i < 205; i++) {
      await reg.send({ chatMode: 'routed', text: 'x', sessionId: `s-${i}` })
      await ended(reg, `s-${i}`)
    }
    expect(reg.ends()).toHaveLength(200)
    expect(reg.ends()[0]!.sessionId).toBe('s-5')
    reg.clearAll()
    expect(reg.ends()).toEqual([])
  })

  it('reconcile ignores a direct turn and a routed turn still live', async () => {
    const { reg } = setup({
      dispatch: vi.fn(async () => sseResponse(withTrace(CHAT_SCENARIOS['direct-plain']))),
    })
    await reg.send({ agentId: 'a-1', text: 'direct', sessionId: 'd-1' })
    await ended(reg, 'd-1')
    const before = reg.get('d-1')
    reg.reconcile('d-1', 'match')
    expect(reg.get('d-1')).toBe(before)

    const hang = (_b: unknown, signal: AbortSignal) =>
      new Promise<Response>((_r, reject) =>
        signal.addEventListener('abort', () => reject(new DOMException('Aborted', 'AbortError'))),
      )
    const other = setup({ dispatch: vi.fn(hang) })
    await other.reg.send({ chatMode: 'routed', text: 'x', sessionId: 'r-1' })
    await vi.waitFor(() => expect(other.reg.get('r-1')?.phase).toBe('waiting'))
    other.reg.reconcile('r-1', 'match')
    expect(other.reg.get('r-1')?.phase).toBe('waiting')
    other.reg.clearAll()
  })
})

describe('routed resume (§5.4, R3, NE-2, NE-3, EN-4)', () => {
  const resumed = (trace: string) =>
    withTrace(
      [
        { data: traceMeta(ROUTED_TRACE) },
        { data: toolCall('@agent-1@', 1) },
        { data: artifact('Resumed.', { lastChunk: true }) },
        { data: usageMeta({ duration_ms: 1 }) },
        { data: status('TASK_STATE_COMPLETED') },
      ],
      trace,
    )

  it('reconnects with the hitl id and no agent, never saves, and settles on the resumed trace', async () => {
    const { reg, deps, saved } = setup({
      dispatch: vi.fn(async () => sseResponse(resumed('5eedf000000000000000000000000002'))),
    })
    await reg.resume('s-1', 'h-1', null, 'm-1', 'routed')
    await ended(reg, 's-1')
    expect(bodyOf(deps).params.metadata).toEqual({ reconnect_after_hitl_id: 'h-1' })
    expect(saved).toEqual([])
    const t = reg.get('s-1')!
    expect(t).toMatchObject({
      phase: 'done',
      chatMode: 'routed',
      operation: 'resume',
      attempt: 'resume:h-1',
    })
    expect(routedSettles(t, [{ role: 'assistant', trace_id: TRACE }], 1)).toBe(false)
    expect(
      routedSettles(t, [{ role: 'assistant', trace_id: '5eedf000000000000000000000000002' }], 1),
    ).toBe(true)
  })

  it('one reconnect per hitl id; a resolve while the paused stream is still open waits, and the newest wins', async () => {
    const pause = gatedSseResponse(withTrace(CHAT_SCENARIOS['routed-hitl']))
    const dispatch = vi
      .fn()
      .mockResolvedValueOnce(pause.response)
      .mockImplementation(async () => sseResponse(resumed('5eedf000000000000000000000000003')))
    const { reg, deps } = setup({ dispatch })
    await reg.send({ chatMode: 'routed', text: 'deploy', sessionId: 's-1' })
    await vi.waitFor(() => expect(reg.get('s-1')?.phase).toBe('waiting'))
    await pause.release(4)
    await reg.resume('s-1', 'h-old', null, undefined, 'routed')
    await reg.resume('s-1', 'h-1', null, undefined, 'routed')
    expect(deps.dispatch).toHaveBeenCalledTimes(1)
    await pause.releaseAll()
    await vi.waitFor(() => expect(deps.dispatch).toHaveBeenCalledTimes(2))
    await ended(reg, 's-1')
    expect(bodyOf(deps, 1).params.metadata).toEqual({ reconnect_after_hitl_id: 'h-1' })
    await reg.resume('s-1', 'h-1', null, undefined, 'routed')
    expect(deps.dispatch).toHaveBeenCalledTimes(2)
  })

  it('reconnect refusals: 403 is resume-forbidden; 400/404/409 may still arrive; an empty 200 (expired) too', async () => {
    const cases: [Response, string][] = [
      [
        HttpResponse.json(
          { jsonrpc: '2.0', id: null, error: { code: -32605, message: 'not authorized' } },
          { status: 403 },
        ),
        'resume_forbidden',
      ],
      [
        HttpResponse.json(
          { jsonrpc: '2.0', id: null, error: { code: -32602, message: 'maf' } },
          { status: 400 },
        ),
        'resume_uncertain',
      ],
      [new HttpResponse('gone', { status: 409 }), 'resume_uncertain'],
      [sseResponse([]), 'resume_uncertain'],
    ]
    for (const [res, phase] of cases) {
      const { reg } = setup({ dispatch: vi.fn(async () => res) })
      await reg.resume('s-1', 'h-1', null, undefined, 'routed')
      await ended(reg, 's-1')
      expect(reg.get('s-1')?.phase).toBe(phase)
    }
  })

  it('a truncation marker cancels the replay and loads the saved reply; limits cancel too', async () => {
    const gate = gatedSseResponse(
      withTrace([
        { data: traceMeta(ROUTED_TRACE) },
        {
          data: status('TASK_STATE_WORKING', [
            { text: '[replay truncated: some output was dropped]' },
          ]),
        },
        { data: artifact('never read') },
      ]),
    )
    const { reg } = setup({ dispatch: vi.fn(async () => gate.response) })
    await reg.resume('s-1', 'h-1', null, undefined, 'routed')
    await vi.waitFor(() => expect(reg.get('s-1')?.phase).toBe('waiting'))
    await gate.release(2)
    await ended(reg, 's-1')
    expect(reg.get('s-1')?.phase).toBe('loading_saved')
    expect(gate.cancelled).toBe(true)
  })

  it('a second pause after the resume pauses again', async () => {
    const { reg } = setup({
      dispatch: vi.fn(async () => sseResponse(withTrace(CHAT_SCENARIOS['routed-hitl']))),
    })
    await reg.resume('s-1', 'h-1', null, undefined, 'routed')
    await ended(reg, 's-1')
    expect(reg.get('s-1')?.phase).toBe('paused')
  })

  it('a reconnect whose dispatch never gets a response is uncertain, never lost', async () => {
    const { reg } = setup({
      dispatch: vi.fn(async () => {
        throw new TypeError('Failed to fetch')
      }),
    })
    await reg.resume('s-1', 'h-1', null, 'u-1', 'routed')
    await ended(reg, 's-1')
    expect(reg.get('s-1')?.phase).toBe('resume_uncertain')
  })

  it('a reconnect flooding unreadable frames cancels and loads the saved reply, marked incomplete', async () => {
    const body = 'data: {not json\n\n'.repeat(tuning.MAX_BAD_FRAMES + 2)
    const { reg } = setup({
      dispatch: vi.fn(
        async () =>
          new Response(body, { status: 200, headers: { 'Content-Type': 'text/event-stream' } }),
      ),
    })
    await reg.resume('s-1', 'h-1', null, 'u-1', 'routed')
    await ended(reg, 's-1')
    expect(reg.get('s-1')).toMatchObject({ phase: 'loading_saved', incomplete: true })
  })
})

describe('review fixes: resume edges (2026-09-27 review)', () => {
  it('a resume that overflows before its first event ends uncertain, not stuck in waiting', async () => {
    const cap = tuning.MAX_EVENT_CHARS
    tuning.MAX_EVENT_CHARS = 100
    try {
      const huge = new Response(
        new ReadableStream<Uint8Array>({
          start(c) {
            c.enqueue(new TextEncoder().encode(`data: ${'x'.repeat(500)}`))
          },
        }),
        { status: 200, headers: { 'Content-Type': 'text/event-stream' } },
      )
      const { reg } = setup({ dispatch: vi.fn(async () => huge) })
      await reg.resume('s-1', 'h-1', null, undefined, 'routed')
      await ended(reg, 's-1')
      expect(reg.get('s-1')?.phase).toBe('loading_saved')
      expect(reg.busy()).toBe(false)
    } finally {
      tuning.MAX_EVENT_CHARS = cap
    }
  })

  it('a silent reconnect (expired buffer, kept open by the server) gives up after RESUME_FIRST_FRAME_MS', async () => {
    const wait = tuning.RESUME_FIRST_FRAME_MS
    tuning.RESUME_FIRST_FRAME_MS = 30
    try {
      const silent = () =>
        new Response(new ReadableStream<Uint8Array>({ start() {} }), {
          status: 200,
          headers: { 'Content-Type': 'text/event-stream' },
        })
      const { reg } = setup({ dispatch: vi.fn(async () => silent()) })
      await reg.resume('s-1', 'h-1', null, undefined, 'routed')
      await ended(reg, 's-1')
      expect(reg.get('s-1')).toMatchObject({
        phase: 'resume_uncertain',
        error: { key: 'routedMayStillArrive' },
      })
    } finally {
      tuning.RESUME_FIRST_FRAME_MS = wait
    }
  })

  it('a resume whose lock is held retries until it frees, then reconnects once', async () => {
    const retry = tuning.RESUME_LOCK_RETRY_MS
    tuning.RESUME_LOCK_RETRY_MS = 5
    try {
      let busy = true
      const locks = {
        request(_n: string, _o: unknown, cb: (l: { name: string } | null) => unknown) {
          return Promise.resolve(cb(busy ? null : { name: 'x' }))
        },
      } as unknown as Pick<LockManager, 'request'>
      const { reg, deps } = setup({
        locks,
        dispatch: vi.fn(async () => sseResponse(withTrace(CHAT_SCENARIOS['routed-plain']))),
      })
      await reg.resume('s-1', 'h-1', null, undefined, 'routed')
      expect(deps.dispatch).not.toHaveBeenCalled()
      busy = false
      await vi.waitFor(() => expect(deps.dispatch).toHaveBeenCalledOnce())
      expect(bodyOf(deps).params.metadata).toEqual({ reconnect_after_hitl_id: 'h-1' })
    } finally {
      tuning.RESUME_LOCK_RETRY_MS = retry
    }
  })

  it('a lock retry never reconnects on a chat that was aborted (deleted) meanwhile', async () => {
    const retry = tuning.RESUME_LOCK_RETRY_MS
    tuning.RESUME_LOCK_RETRY_MS = 5
    try {
      let busy = true
      const locks = {
        request(_n: string, _o: unknown, cb: (l: { name: string } | null) => unknown) {
          return Promise.resolve(cb(busy ? null : { name: 'x' }))
        },
      } as unknown as Pick<LockManager, 'request'>
      vi.useFakeTimers()
      const { reg, deps } = setup({ locks })
      await reg.resume('s-1', 'h-1', null, undefined, 'routed')
      reg.abort('s-1')
      busy = false
      // Fake time: every retry the budget allows runs, however busy the machine is.
      await vi.advanceTimersByTimeAsync(
        tuning.RESUME_LOCK_RETRY_MS * (tuning.RESUME_LOCK_RETRIES + 1),
      )
      expect(deps.dispatch).not.toHaveBeenCalled()
    } finally {
      tuning.RESUME_LOCK_RETRY_MS = retry
    }
  })

  it('a queued resume for a turn that has since been replaced is dropped', async () => {
    const hang = gatedSseResponse(withTrace(CHAT_SCENARIOS['routed-plain']))
    const dispatch = vi
      .fn()
      .mockResolvedValueOnce(hang.response)
      .mockImplementation(async () => sseResponse(withTrace(CHAT_SCENARIOS['routed-plain'])))
    // Fake time: a dropped resume can't hide behind a lock retry that fires after the check.
    vi.useFakeTimers()
    const { reg, deps } = setup({ dispatch })
    await reg.send({ chatMode: 'routed', text: 'x', sessionId: 's-1' })
    await vi.waitFor(() => expect(reg.get('s-1')?.phase).toBe('waiting'))
    await reg.resume('s-1', 'h-1', null, undefined, 'routed')
    reg.abort('s-1')
    await vi.advanceTimersByTimeAsync(0)
    await reg.send({ chatMode: 'routed', text: 'new', sessionId: 's-1' })
    await ended(reg, 's-1')
    await vi.advanceTimersByTimeAsync(
      tuning.RESUME_LOCK_RETRY_MS * (tuning.RESUME_LOCK_RETRIES + 1),
    )
    expect(
      (deps.dispatch as ReturnType<typeof vi.fn>).mock.calls.some(
        (c) =>
          (c[0] as { params: { metadata: Record<string, unknown> } }).params.metadata
            .reconnect_after_hitl_id,
      ),
    ).toBe(false)
  })

  it('a resume carries the baseline measured when its request was answered', async () => {
    const { reg } = setup({ dispatch: vi.fn(async () => sseResponse([])) })
    await reg.resume('s-1', 'h-1', null, 'm-1', 'routed', 2)
    await ended(reg, 's-1')
    expect(reg.get('s-1')?.baselineReplies).toBe(2)
  })

  it('a routed 401 hands off to sign-in, records no end and forgets the turn (nothing ran)', async () => {
    const { reg, deps, ends, saved } = setup({
      dispatch: vi.fn(async () => new HttpResponse('unauthorized', { status: 401 })),
    })
    const t = await reg.send({ chatMode: 'routed', text: 'x' })
    await vi.waitFor(() => expect(deps.onUnauthorized).toHaveBeenCalledOnce())
    // Forgotten, so history's no-reply notice (with Run again) shows after signing back in (D4).
    expect(reg.get(t.sessionId)).toBeUndefined()
    expect(ends).toEqual([])
    expect(saved.map((x) => x.body.role)).toEqual(['user'])
  })

  it("the resumed reply is the orchestrator's text only, not the sub-agent's replayed stream", async () => {
    const frames = withTrace([
      { data: status('TASK_STATE_WORKING') },
      { data: artifact('sub-agent raw reply', { id: 'sub-a1', lastChunk: true }) },
      { data: status('TASK_STATE_COMPLETED') },
      { data: traceMeta(ROUTED_TRACE) },
      { data: status('TASK_STATE_COMPLETED') },
    ])
    const { reg } = setup({ dispatch: vi.fn(async () => sseResponse(frames)) })
    await reg.resume('s-1', 'h-1', null, undefined, 'routed')
    await ended(reg, 's-1')
    // The orchestrator's continuation turn wrote nothing: the server saves nothing either.
    expect(reg.get('s-1')?.phase).toBe('known_empty')
  })
})

describe('review fixes: reducer limits and names', () => {
  it('stops tracking calls, results and unpaired steps past MAX_STEPS instead of freezing', async () => {
    const { reduceSseEvent, emptyTurn } = await import('./a2aReducer')
    let s = emptyTurn()
    for (let i = 0; i < tuning.MAX_STEPS + 50; i++)
      s = reduceSseEvent(s, { event: 'message', data: JSON.stringify(toolCall('a', i)) }).state
    for (let i = tuning.MAX_STEPS; i < tuning.MAX_STEPS + 50; i++)
      s = reduceSseEvent(s, {
        event: 'message',
        data: JSON.stringify(toolResult('a', i, true, 'x')),
      }).state
    s = reduceSseEvent(s, {
      event: 'message',
      data: JSON.stringify(
        status('TASK_STATE_WORKING', [
          { data: { type: 'policy_rejected', agent: 'b', reason: 'flow timeout: 1s/1s', turn: 9 } },
        ]),
      ),
    }).state
    expect(s.steps).toHaveLength(tuning.MAX_STEPS)
  })

  it('the reply boundary is the first trace_meta even when usage_meta supplied a trace id first', async () => {
    const { reduceSseEvent, emptyTurn, routedReplyText } = await import('./a2aReducer')
    const frames = [
      usageMeta({ trace_id: 'early' }),
      artifact('sub text', { id: 'sub-a1' }),
      traceMeta('orch'),
      artifact('orchestrator reply', { id: 'o1' }),
    ]
    const s = frames.reduce(
      (st, f) => reduceSseEvent(st, { event: 'message', data: JSON.stringify(f) }).state,
      emptyTurn(),
    )
    expect(routedReplyText(s, true)).toBe('orchestrator reply')
  })

  it('keys sub-agent notes by folded name, so raw and display spellings meet', async () => {
    const { reduceSseEvent, emptyTurn, notesFor } = await import('./a2aReducer')
    const { subStatus } = await import('@/mocks/chat')
    const s = reduceSseEvent(emptyTurn(), {
      event: 'message',
      data: JSON.stringify(subStatus('seed_sql.analyst', 'querying')),
    }).state
    expect(notesFor(s, 'seed-sql-analyst')?.status).toBe('querying')
  })
})

describe('rerun, cap and lifetime (§5.2, §5.4, G-10, G-15, EN-3b)', () => {
  it('Run again after a reload re-dispatches a routed chat without a user row, as a rerun', async () => {
    const { reg, deps, saved } = setup()
    await reg.runAgain('s-1', {
      agentId: null,
      chatMode: 'routed',
      userText: 'again',
      userMessageId: 'm-9',
    })
    await ended(reg, 's-1')
    expect(bodyOf(deps).params.metadata).toEqual({ session_id: 's-1' })
    expect(saved).toEqual([])
    expect(reg.get('s-1')).toMatchObject({ operation: 'rerun', attempt: 'rerun:1', phase: 'done' })
  })

  it('a 4th live routed send is refused; direct sends are not; the cap counts both', async () => {
    const hang = (_b: unknown, signal: AbortSignal) =>
      new Promise<Response>((_r, reject) =>
        signal.addEventListener('abort', () => reject(new DOMException('Aborted', 'AbortError'))),
      )
    const { reg } = setup({ dispatch: vi.fn(hang) })
    await reg.send({ agentId: 'a', text: 'direct', sessionId: 'd-1' })
    await reg.send({ chatMode: 'routed', text: '1', sessionId: 'r-1' })
    await reg.send({ chatMode: 'routed', text: '2', sessionId: 'r-2' })
    await expect(
      reg.send({ chatMode: 'routed', text: '3', sessionId: 'r-3' }),
    ).rejects.toMatchObject({ key: 'tooManyLive', certainty: 'not-dispatched' })
    await expect(
      reg.send({ agentId: 'a', text: 'direct 2', sessionId: 'd-2' }),
    ).resolves.toBeDefined()
    expect(reg.liveCount()).toBe(4)
    reg.clearAll()
  })

  it('busy() stays true while any routed turn is live, and with two live while one finishes', async () => {
    const a = gatedSseResponse(withTrace(CHAT_SCENARIOS['routed-plain']))
    const b = gatedSseResponse(withTrace(CHAT_SCENARIOS['routed-plain']))
    const dispatch = vi.fn().mockResolvedValueOnce(a.response).mockResolvedValueOnce(b.response)
    const { reg } = setup({ dispatch })
    await reg.send({ chatMode: 'routed', text: 'a', sessionId: 's-a' })
    await reg.send({ chatMode: 'routed', text: 'b', sessionId: 's-b' })
    await vi.waitFor(() => expect(dispatch).toHaveBeenCalledTimes(2))
    expect(reg.busy()).toBe(true)
    await a.releaseAll()
    await ended(reg, 's-a')
    expect(reg.busy()).toBe(true)
    await b.releaseAll()
    await ended(reg, 's-b')
    expect(reg.busy()).toBe(false)
  })

  it('snapshot() keeps its identity until something changes', async () => {
    const { reg } = setup()
    const first = reg.snapshot()
    expect(reg.snapshot()).toBe(first)
    await reg.send({ chatMode: 'routed', text: 'x', sessionId: 's-1' })
    expect(reg.snapshot()).not.toBe(first)
    await ended(reg, 's-1')
    const settled = reg.snapshot()
    expect(reg.snapshot()).toBe(settled)
  })

  it('each Run again on a routed chat is its own attempt, recorded under its own key', async () => {
    const { reg, deps } = setup({
      dispatch: vi.fn(async () => sseResponse(withTrace(CHAT_SCENARIOS['routed-empty']))),
    })
    await reg.runAgain('s-1', {
      agentId: null,
      chatMode: 'routed',
      userText: 'again',
      userMessageId: 'm-9',
    })
    await ended(reg, 's-1')
    expect(reg.get('s-1')).toMatchObject({ phase: 'known_empty', attempt: 'rerun:1' })
    await reg.runAgain('s-1')
    await ended(reg, 's-1')
    expect(reg.get('s-1')).toMatchObject({ phase: 'known_empty', attempt: 'rerun:2' })
    expect(deps.dispatch).toHaveBeenCalledTimes(2)
    expect(reg.ends().map((e) => e.attemptKey)).toEqual(['m-9:rerun:1', 'm-9:rerun:2'])
    expect(reg.endFor('s-1', 'm-9')?.attemptKey).toBe('m-9:rerun:2')
  })

  it('a 401 before the dispatch ran forgets the turn, so Run again from history re-dispatches the saved user row (D4)', async () => {
    let calls = 0
    const dispatch = vi.fn(async () =>
      ++calls === 1
        ? new HttpResponse('unauthorized', { status: 401 })
        : sseResponse(withTrace(CHAT_SCENARIOS['routed-plain'])),
    )
    const { reg, deps, saved } = setup({ dispatch })
    const t = await reg.send({ chatMode: 'routed', text: 'hi' })
    await vi.waitFor(() => expect(deps.onUnauthorized).toHaveBeenCalledOnce())
    expect(reg.get(t.sessionId)).toBeUndefined()
    // After signing back in, the page's no-reply notice calls Run again with the saved user row.
    // The chat's lock is released a tick after the dispatch settles (as a Web Lock is).
    await new Promise((r) => setTimeout(r, 0))
    await reg.runAgain(t.sessionId, {
      agentId: null,
      chatMode: 'routed',
      userText: 'hi',
      userMessageId: 'm-1',
    })
    await ended(reg, t.sessionId)
    expect(dispatch).toHaveBeenCalledTimes(2)
    expect(reg.get(t.sessionId)).toMatchObject({
      phase: 'done',
      attempt: 'rerun:1',
      userMessageId: 'm-1',
    })
    // The user row isn't saved again, and no assistant row is ever saved on a routed chat.
    expect(saved.map((x) => x.body.role)).toEqual(['user'])
  })
})

describe('the app registry (registry.ts)', () => {
  it('a routed turn refreshes the chat queries when it ends, and a hot reload mid-turn warns (DX-4)', async () => {
    const rec = recordRequestBodies()
    try {
      configureMocks({ loggedIn: true })
      configureChatMock({ scenario: 'routed-plain' })
      const gate = gatedSseResponse(withTrace(CHAT_SCENARIOS['routed-plain']))
      server.use(http.post('/api/orchestrator/a2a', () => gate.response))
      const client = new QueryClient()
      const invalidate = vi.spyOn(client, 'invalidateQueries')
      const reg = chatRegistry(client, 'u-hmr')
      await reg.send({
        chatMode: 'routed',
        text: 'x',
        sessionId: '5eedc000-0000-4000-8000-00000000d001',
        create: true,
      })
      await vi.waitFor(
        () => expect(reg.get('5eedc000-0000-4000-8000-00000000d001')?.phase).toBe('waiting'),
        { timeout: 2000 },
      )
      const warn = vi.fn()
      disposeForHotReload(warn)
      expect(warn).toHaveBeenCalledWith(
        expect.stringMatching(
          /^\[chat\] hot reload aborted 1 live turn\(s\)\. A routed reply is saved server-side only if the stream reached Done/,
        ),
      )
      // A fresh registry: the turn ends and the one invalidation comes from the registry.
      const reg2 = chatRegistry(client, 'u-hmr')
      const gate2 = gatedSseResponse(withTrace(CHAT_SCENARIOS['routed-plain']))
      server.use(http.post('/api/orchestrator/a2a', () => gate2.response))
      await reg2.send({
        chatMode: 'routed',
        text: 'y',
        sessionId: '5eedc000-0000-4000-8000-00000000d001',
      })
      await gate2.releaseAll()
      await ended(reg2, '5eedc000-0000-4000-8000-00000000d001')
      expect(invalidate).toHaveBeenCalledWith({
        queryKey: ['chat', 'history', '5eedc000-0000-4000-8000-00000000d001'],
      })
      await rec.flush()
      expect(rec.assistantPosts()).toEqual([])
    } finally {
      rec.stop()
    }
  })
})

describe('recordRequestBodies (EN-11)', () => {
  it('catches a deliberately delayed assistant POST once flushed', async () => {
    const rec = recordRequestBodies()
    try {
      server.use(
        http.post('/api/chat/sessions/:id/messages', () => HttpResponse.json({}, { status: 201 })),
      )
      const slow = new ReadableStream<Uint8Array>({
        async start(c) {
          // The delay is the scenario (a body that arrives late), not a wait for the code under test.
          await new Promise((r) => setTimeout(r, 20))
          c.enqueue(
            new TextEncoder().encode(JSON.stringify({ role: 'assistant', content: 'late' })),
          )
          c.close()
        },
      })
      await fetch(new URL('/api/chat/sessions/s-1/messages', globalThis.location.origin), {
        method: 'POST',
        body: slow,
        headers: { 'Content-Type': 'application/json' },
        duplex: 'half',
      } as RequestInit)
      await rec.flush()
      expect(rec.assistantPosts()).toHaveLength(1)
      expect(rec.parseErrors).toBe(0)
    } finally {
      rec.stop()
    }
  })
})

describe('mock flows for a paused routed turn', () => {
  it("awaiting_human marks the agent's running call paused and adds no row", async () => {
    configureMocks({ loggedIn: true })
    configureChatMock({ scenario: 'routed-hitl' })
    const url = (p: string) => new URL(p, globalThis.location.origin)
    const post = (p: string, body: unknown) =>
      fetch(url(p), {
        method: 'POST',
        body: JSON.stringify(body),
        headers: { 'Content-Type': 'application/json' },
      })
    const sid = '5eedc000-0000-4000-8000-00000000d001'
    expect(
      (await post('/api/chat/sessions', { session_id: sid, first_prompt: 'deploy it' })).status,
    ).toBe(201)
    await post(`/api/chat/sessions/${sid}/messages`, { role: 'user', content: 'deploy it' })
    const res = await post('/api/orchestrator/a2a', {
      jsonrpc: '2.0',
      id: 1,
      method: 'message/stream',
      params: {
        message: {
          messageId: 'm',
          role: 'ROLE_USER',
          parts: [{ text: 'deploy it' }],
          contextId: sid,
        },
        metadata: { session_id: sid },
      },
    })
    const frames: unknown[] = []
    await readSse(res, (evs) => evs.forEach((e) => frames.push(JSON.parse(e.data))))
    const trace = JSON.stringify(frames).match(/"trace_id":"(5eedf\w+)"/)![1]!
    const flow = (await (await fetch(url(`/api/flows/${trace}`))).json()) as {
      steps: { depth: number; agent_name: string; status: string }[]
    }
    // One call, one row: the pause is that row's status, not a second row.
    expect(flow.steps).toHaveLength(1)
    expect(flow.steps[0]).toMatchObject({ depth: 1, status: 'awaiting_human' })
  })
})
