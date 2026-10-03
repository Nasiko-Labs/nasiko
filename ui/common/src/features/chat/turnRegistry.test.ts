import { afterEach, describe, expect, it, vi } from 'vitest'
import { ApiError } from '@/lib/api/client'
import {
  CHAT_SCENARIOS,
  mockStream,
  sseResponse,
  artifact,
  status,
  type MockFrame,
} from '@/mocks/chat'
import { RECEIVING_STOPPED_MARKER } from './normalize'
import { tuning } from './tuning'
import { createTurnRegistry, isLivePhase, type ChatDeps, type TurnRegistry } from './turnRegistry'
import type { ChatMessage, ChatSessionRow, SaveMessageBody } from './types'

/** In-memory `navigator.locks` with `ifAvailable` semantics, shared between "tabs". */
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
    held,
  } as unknown as Pick<LockManager, 'request'> & { held: Set<string> }
}

function setup(over: Partial<ChatDeps> = {}, frames: MockFrame[] = CHAT_SCENARIOS['direct-plain']) {
  let n = 0
  const saved: { sessionId: string; body: SaveMessageBody }[] = []
  const deps: ChatDeps = {
    userId: 'u-1',
    createSession: vi.fn(
      async (b) =>
        ({
          session_id: b.session_id,
          agent_id: b.agent_id,
          title: 'T',
          created_at: '',
          updated_at: '',
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
    now: () => 0,
    locks: lockManager(),
    warn: vi.fn(),
    ...over,
  }
  return { deps, saved, reg: createTurnRegistry(deps) }
}

const settle = (reg: TurnRegistry, id: string) =>
  vi.waitFor(
    () => {
      const t = reg.get(id)
      if (!t || isLivePhase(t.phase)) throw new Error(`still ${t?.phase}`)
    },
    { timeout: 2000 },
  )

afterEach(() => vi.useRealTimers())

describe('direct turn', () => {
  it('creates the chat with a client id, saves the user row, then the reply once with usage', async () => {
    const onCreated = vi.fn()
    const { reg, deps, saved } = setup()
    const t = await reg.send({ agentId: 'agent-1', text: '  hello  ', onCreated })
    await settle(reg, t.sessionId)
    expect(deps.createSession).toHaveBeenCalledWith(
      { session_id: t.sessionId, agent_id: 'agent-1', first_prompt: 'hello' },
      expect.any(AbortSignal),
    )
    expect(onCreated).toHaveBeenCalledOnce()
    expect(saved.map((s) => s.body.role)).toEqual(['user', 'assistant'])
    expect(saved[1].body).toEqual({
      role: 'assistant',
      content: 'Hello from the agent.',
      usage: {
        input_tokens: 812,
        output_tokens: 96,
        model: 'gpt-4o-mini',
        duration_ms: 1240,
        cost_usd: 0.0021,
        estimated: false,
        trace_id: '5eedc0000000000000000000000000a1',
      },
    })
    expect(reg.get(t.sessionId)).toMatchObject({ phase: 'done', userMessageId: 'm-1' })
  })

  it('sends contextId = session id and the agent in metadata', async () => {
    const { reg, deps } = setup()
    const t = await reg.send({ sessionId: 'sess-9', agentId: 'agent-1', text: 'hi' })
    await settle(reg, 'sess-9')
    const body = (deps.dispatch as ReturnType<typeof vi.fn>).mock.calls[0][0] as {
      params: { message: { contextId: string }; metadata: object }
    }
    expect(body.params.message.contextId).toBe('sess-9')
    expect(body.params.metadata).toEqual({ agent_id: 'agent-1', session_id: 'sess-9' })
    expect(deps.createSession).not.toHaveBeenCalled()
    expect(t.sessionId).toBe('sess-9')
  })

  it('saves once across terminal → usage → terminal', async () => {
    const { reg, saved } = setup({}, CHAT_SCENARIOS['terminal-usage-terminal'])
    await reg.send({ sessionId: 's', agentId: 'a', text: 'x' })
    await settle(reg, 's')
    expect(saved.filter((s) => s.body.role === 'assistant')).toHaveLength(1)
    expect(saved[1].body.usage?.duration_ms).toBe(1240)
  })

  it('never saves an empty reply and marks the chat execution unknown', async () => {
    const { reg, saved } = setup({}, CHAT_SCENARIOS['empty-reply'])
    await reg.send({ sessionId: 's', agentId: 'a', text: 'x' })
    await settle(reg, 's')
    expect(saved.map((s) => s.body.role)).toEqual(['user'])
    expect(reg.get('s')?.phase).toBe('no_reply')
    expect(reg.isExecutionUnknown('s')).toBe(true)
  })

  it('a pause saves nothing (the server saves the resumed reply)', async () => {
    const { reg, saved } = setup({}, CHAT_SCENARIOS['hitl-options'])
    await reg.send({ sessionId: 's', agentId: 'a', text: 'x' })
    await settle(reg, 's')
    expect(reg.get('s')?.phase).toBe('paused')
    expect(saved.map((s) => s.body.role)).toEqual(['user'])
  })

  it('an agent failure is an error with unknown certainty', async () => {
    const { reg } = setup({}, CHAT_SCENARIOS.failed)
    await reg.send({ sessionId: 's', agentId: 'a', text: 'x' })
    await settle(reg, 's')
    expect(reg.get('s')?.error).toMatchObject({ key: 'agentFailed', certainty: 'unknown' })
  })

  it('writes nothing the server owns when the session row says so', async () => {
    const { reg, saved } = setup()
    await reg.send({
      sessionId: 's',
      agentId: 'a',
      text: 'x',
      transcript: { user: 'server', assistant: 'server' },
    })
    await settle(reg, 's')
    expect(saved).toEqual([])
    expect(reg.get('s')?.phase).toBe('done')
  })

  it('keeps the steps of a saved reply after the live turn is forgotten (ISSUE-001)', async () => {
    let n = 0
    const reg = createTurnRegistry({
      userId: 'u',
      newId: () => `id-${++n}`,
      now: () => 0,
      locks: null,
      onUnauthorized: vi.fn(),
      createSession: vi.fn(),
      replyExists: vi.fn(async () => false),
      saveMessage: vi.fn(
        async (sessionId: string, body: SaveMessageBody) =>
          ({
            id: `m-${body.role}`,
            session_id: sessionId,
            role: body.role,
            content: body.content,
            timestamp: '',
          }) as ChatMessage,
      ),
      dispatch: vi.fn(async () => sseResponse(CHAT_SCENARIOS['direct-steps'])),
    })
    await reg.send({ sessionId: 's', agentId: 'a', text: 'x' })
    await vi.waitFor(
      () => {
        const t = reg.get('s')
        if (!t || isLivePhase(t.phase)) throw new Error('live')
      },
      { timeout: 2000 },
    )
    const live = reg.get('s')!.state.steps
    expect(live.length).toBeGreaterThan(0)
    reg.forget('s')
    expect(reg.get('s')).toBeUndefined()
    // Kept for the chip, tool results included (these are short, so untruncated).
    expect(reg.stepsFor('m-assistant')).toEqual(live)
  })
})

describe('failures', () => {
  it('a create failure dispatches nothing', async () => {
    const { reg, deps } = setup({
      createSession: vi.fn(async () => {
        throw new ApiError(500, null, '/x', 'test')
      }),
    })
    const t = await reg.send({ agentId: 'a', text: 'x' })
    await settle(reg, t.sessionId)
    expect(reg.get(t.sessionId)?.error).toMatchObject({
      key: 'createFailed',
      certainty: 'not-dispatched',
    })
    expect(deps.dispatch).not.toHaveBeenCalled()
  })

  it('a create timeout aborts and a retry reuses the same session id', async () => {
    vi.useFakeTimers()
    const createSession = vi.fn(
      (_b: unknown, signal: AbortSignal) =>
        new Promise<ChatSessionRow>((_r, reject) =>
          signal.addEventListener('abort', () => reject(new DOMException('Aborted', 'AbortError'))),
        ),
    )
    const { reg } = setup({ createSession })
    const t = await reg.send({ agentId: 'a', text: 'x' })
    await vi.advanceTimersByTimeAsync(tuning.CREATE_TIMEOUT_MS + 1)
    expect(reg.get(t.sessionId)?.phase).toBe('error')
    vi.useRealTimers()
    createSession.mockImplementation(async (b: unknown) => ({
      session_id: (b as { session_id: string }).session_id,
      agent_id: 'a',
      title: '',
      created_at: '',
      updated_at: '',
    }))
    // The UI retries with the minted id, so the server returns the same chat (routes.rs:332-345).
    await reg.send({ sessionId: t.sessionId, create: true, agentId: 'a', text: 'x' })
    await settle(reg, t.sessionId)
    expect(reg.get(t.sessionId)?.phase).toBe('done')
    expect(createSession).toHaveBeenCalledTimes(2)
    expect(
      createSession.mock.calls.map((c) => (c[0] as { session_id: string }).session_id),
    ).toEqual([t.sessionId, t.sessionId])
  })

  it('maps HTTP errors before any frame', async () => {
    const { reg } = setup({
      dispatch: vi.fn(
        async () => new Response('rate limit exceeded, try again shortly', { status: 429 }),
      ),
    })
    await reg.send({ sessionId: 's', agentId: 'a', text: 'x' })
    await settle(reg, 's')
    expect(reg.get('s')?.error).toMatchObject({
      key: 'rateLimited',
      certainty: 'rejected-before-run',
    })
    expect(reg.isExecutionUnknown('s')).toBe(false)
  })

  it('a 401 hands off to the login redirect without cut-off copy', async () => {
    const { reg, deps } = setup({ dispatch: vi.fn(async () => new Response('', { status: 401 })) })
    await reg.send({ sessionId: 's', agentId: 'a', text: 'x' })
    await settle(reg, 's')
    expect(deps.onUnauthorized).toHaveBeenCalledOnce()
    expect(reg.get('s')).toMatchObject({ phase: 'aborted', error: null })
  })

  it('a dropped stream is cut off with unknown certainty', async () => {
    const { reg } = setup({
      dispatch: vi.fn(
        async () => new Response(mockStream([{ data: artifact('part') }], { failAtEnd: true })),
      ),
    })
    await reg.send({ sessionId: 's', agentId: 'a', text: 'x' })
    await settle(reg, 's')
    expect(reg.get('s')?.error).toMatchObject({ key: 'cutOff', certainty: 'unknown' })
    expect(reg.isExecutionUnknown('s')).toBe(true)
  })

  it('too many unreadable frames fail the turn and warn with turn and frame index', async () => {
    const frames: MockFrame[] = Array.from({ length: tuning.MAX_BAD_FRAMES + 1 }, () => ({
      raw: 'x',
    }))
    const { reg, deps } = setup({}, frames)
    await reg.send({ sessionId: 's', agentId: 'a', text: 'x' })
    await settle(reg, 's')
    expect(reg.get('s')?.error?.key).toBe('unreadable')
    expect(deps.warn).toHaveBeenCalledWith(
      expect.any(String),
      expect.objectContaining({ turnId: expect.any(String), frameIndex: 0 }),
    )
  })

  it('an unknown save failure checks history before saving again', async () => {
    let fail = true
    const saveMessage = vi.fn(async (sessionId: string, body: SaveMessageBody) => {
      if (body.role === 'assistant' && fail) throw new TypeError('network')
      return {
        id: `m-${body.role}`,
        session_id: sessionId,
        role: body.role,
        content: body.content,
        timestamp: '',
      } as ChatMessage
    })
    const replyExists = vi.fn(async () => true)
    const { reg } = setup({ saveMessage, replyExists })
    await reg.send({ sessionId: 's', agentId: 'a', text: 'x' })
    await settle(reg, 's')
    expect(reg.get('s')).toMatchObject({ phase: 'unsaved', error: { key: 'saveUnknown' } })
    fail = false
    await reg.saveAgain('s')
    expect(replyExists).toHaveBeenCalledWith('s', 'm-user', 'Hello from the agent.')
    expect(saveMessage.mock.calls.filter(([, b]) => b.role === 'assistant')).toHaveLength(1)
    expect(reg.get('s')?.phase).toBe('done')
  })
})

describe('resume', () => {
  it('streams the resumed reply without saving it, once per request id', async () => {
    const { reg, saved, deps } = setup({}, CHAT_SCENARIOS['direct-plain'])
    await reg.resume('s', 'req-1', 'a')
    await settle(reg, 's')
    expect(saved).toEqual([])
    expect(reg.get('s')).toMatchObject({ phase: 'done', operation: 'resume' })
    const body = (deps.dispatch as ReturnType<typeof vi.fn>).mock.calls[0][0] as {
      params: { message: { parts: unknown[] }; metadata: object }
    }
    expect(body.params.metadata).toEqual({ reconnect_after_hitl_id: 'req-1' })
    expect(body.params.message.parts).toEqual([])
    await reg.resume('s', 'req-1', 'a')
    expect(deps.dispatch).toHaveBeenCalledTimes(1)
  })

  it('a 409 or 429 on resume reconciles through history instead of failing', async () => {
    const { reg } = setup({
      dispatch: vi.fn(
        async () => new Response('rate limit exceeded, try again shortly', { status: 429 }),
      ),
    })
    await reg.resume('s', 'req-1', 'a')
    await settle(reg, 's')
    expect(reg.get('s')).toMatchObject({ phase: 'done', error: null })
  })

  it('a resume that ends without a terminal event is done (history decides)', async () => {
    const { reg } = setup({
      dispatch: vi.fn(
        async () => new Response(mockStream([{ data: artifact('half') }], { failAtEnd: true })),
      ),
    })
    await reg.resume('s', 'req-1', 'a')
    await settle(reg, 's')
    expect(reg.get('s')?.phase).toBe('done')
  })
})

describe('lifetime', () => {
  it('holds a Web Lock so a second tab cannot send into the same chat', async () => {
    const locks = lockManager()
    let release!: () => void
    const gate = new Promise<void>((r) => {
      release = r
    })
    const slow = vi.fn(async () => {
      await gate
      return sseResponse(CHAT_SCENARIOS['direct-plain'])
    })
    const tabA = setup({ locks, dispatch: slow }).reg
    const tabB = setup({ locks }).reg
    await tabA.send({ sessionId: 's', agentId: 'a', text: 'x' })
    await vi.waitFor(() => expect(tabA.get('s')?.phase).toBe('waiting'))
    await expect(tabB.send({ sessionId: 's', agentId: 'a', text: 'y' })).rejects.toMatchObject({
      key: 'otherTab',
    })
    release()
    await settle(tabA, 's')
    await vi.waitFor(() => expect(locks.held.size).toBe(0))
  })

  it('refuses a second send while a turn is live in this tab', async () => {
    const { reg } = setup({ dispatch: vi.fn(() => new Promise<Response>(() => undefined)) })
    await reg.send({ sessionId: 's', agentId: 'a', text: 'x' })
    await expect(reg.send({ sessionId: 's', agentId: 'a', text: 'y' })).rejects.toMatchObject({
      key: 'otherTab',
    })
  })

  it('Stop receiving saves the partial once with the marker and marks execution unknown', async () => {
    const frames: MockFrame[] = [
      { data: artifact('partial ') },
      { data: artifact('more', { append: true }), delayMs: 5000 },
    ]
    vi.useFakeTimers()
    const { reg, saved } = setup({}, frames)
    await reg.send({ sessionId: 's', agentId: 'a', text: 'x' })
    await vi.waitFor(() => expect(reg.get('s')?.phase).toBe('streaming'))
    reg.stop('s')
    await vi.runAllTimersAsync()
    vi.useRealTimers()
    await settle(reg, 's')
    expect(saved[1].body).toEqual({
      role: 'assistant',
      content: `partial ${RECEIVING_STOPPED_MARKER}`,
    })
    expect(reg.isExecutionUnknown('s')).toBe(true)
  })

  it('an idle stream shows a notice and keeps reading', async () => {
    vi.useFakeTimers()
    const frames: MockFrame[] = [
      { data: artifact('a') },
      { data: artifact('b', { append: true }), delayMs: tuning.STREAM_IDLE_MS + 1000 },
      { data: status('TASK_STATE_COMPLETED') },
    ]
    const { reg, saved } = setup({}, frames)
    await reg.send({ sessionId: 's', agentId: 'a', text: 'x' })
    await vi.advanceTimersByTimeAsync(tuning.STREAM_IDLE_MS + 10)
    expect(reg.get('s')).toMatchObject({ idle: true, phase: 'streaming' })
    await vi.runAllTimersAsync()
    vi.useRealTimers()
    await settle(reg, 's')
    expect(saved[1].body.content).toBe('ab')
  })

  it('abort (delete, logout) saves nothing and releases the lock', async () => {
    const locks = lockManager()
    const hang = (_b: unknown, signal: AbortSignal) =>
      new Promise<Response>((_r, reject) =>
        signal.addEventListener('abort', () => reject(new DOMException('Aborted', 'AbortError'))),
      )
    const { reg, saved } = setup({ locks, dispatch: vi.fn(hang) })
    await reg.send({ sessionId: 's', agentId: 'a', text: 'x' })
    await vi.waitFor(() => expect(reg.get('s')?.phase).toBe('waiting'))
    reg.abort('s')
    expect(reg.get('s')?.phase).toBe('aborted')
    expect(saved.map((s) => s.body.role)).toEqual(['user'])
    await vi.waitFor(() => expect(locks.held.size).toBe(0))
  })

  it('Run again re-dispatches without a new user row', async () => {
    const { reg, saved, deps } = setup({}, CHAT_SCENARIOS['empty-reply'])
    await reg.send({ sessionId: 's', agentId: 'a', text: 'x' })
    await settle(reg, 's')
    vi.mocked(deps.dispatch).mockImplementation(async () =>
      sseResponse(CHAT_SCENARIOS['direct-plain']),
    )
    await reg.runAgain('s')
    await settle(reg, 's')
    expect(saved.map((s) => s.body.role)).toEqual(['user', 'assistant'])
    expect(reg.isExecutionUnknown('s')).toBe(false)
  })

  it('Run again after a reload dispatches the history turn without a new user row', async () => {
    const { reg, saved, deps } = setup()
    await reg.runAgain('s', { agentId: 'a', userText: 'from history', userMessageId: 'm-9' })
    await settle(reg, 's')
    const body = (deps.dispatch as ReturnType<typeof vi.fn>).mock.calls[0][0] as {
      params: { message: { parts: { text: string }[] } }
    }
    expect(body.params.message.parts[0].text).toBe('from history')
    expect(saved.map((s) => s.body.role)).toEqual(['assistant'])
    expect(reg.get('s')).toMatchObject({ phase: 'done', userMessageId: 'm-9' })
  })

  it('Discard reply drops an unsaved reply and keeps the chat execution unknown', async () => {
    let calls = 0
    const { reg } = setup({
      saveMessage: vi.fn(async (sessionId: string, body: SaveMessageBody) => {
        if (++calls > 1) throw new ApiError(400, null, '/x', 'bad')
        return {
          id: 'm-1',
          session_id: sessionId,
          role: body.role,
          content: body.content,
          timestamp: '',
        } as ChatMessage
      }),
    })
    await reg.send({ sessionId: 's', agentId: 'a', text: 'x' })
    await settle(reg, 's')
    expect(reg.get('s')?.phase).toBe('unsaved')
    reg.discard('s')
    expect(reg.get('s')).toBeUndefined()
    expect(reg.isExecutionUnknown('s')).toBe(true)
  })

  it('clearAll aborts every turn (logout)', async () => {
    const { reg } = setup({ dispatch: vi.fn(() => new Promise<Response>(() => undefined)) })
    await reg.send({ sessionId: 's', agentId: 'a', text: 'x' })
    reg.clearAll()
    expect(reg.get('s')).toBeUndefined()
  })
})

describe('review fixes (races and lifecycle)', () => {
  const never = () => new Promise<never>(() => undefined)

  it('a chat deleted while its user row is saving never dispatches', async () => {
    let finishSave!: (m: ChatMessage) => void
    const { reg, deps } = setup({
      saveMessage: vi.fn(
        () =>
          new Promise<ChatMessage>((r) => {
            finishSave = r
          }),
      ),
    })
    await reg.send({ sessionId: 's', agentId: 'a', text: 'x' })
    reg.abort('s')
    finishSave({
      id: 'm-1',
      session_id: 's',
      role: 'user',
      content: 'x',
      timestamp: '',
    } as ChatMessage)
    // Proves an absence (no dispatch): give the save's promise chain time to run on.
    await new Promise((r) => setTimeout(r, 20))
    expect(deps.dispatch).not.toHaveBeenCalled()
    expect(reg.get('s')?.phase).toBe('aborted')
  })

  it("a stale run never releases a newer turn's lock", async () => {
    const locks = lockManager()
    let failFirst!: (e: Error) => void
    let calls = 0
    const { reg } = setup({
      locks,
      saveMessage: vi.fn((sessionId: string, body: SaveMessageBody) => {
        if (++calls === 1)
          return new Promise<ChatMessage>((_r, reject) => {
            failFirst = reject
          })
        return Promise.resolve({
          id: `m-${calls}`,
          session_id: sessionId,
          role: body.role,
          content: body.content,
          timestamp: '',
        } as ChatMessage)
      }),
      dispatch: vi.fn(never),
    })
    await reg.send({ sessionId: 's', agentId: 'a', text: 'first' })
    reg.abort('s')
    // Web Locks free a released lock asynchronously.
    await new Promise((r) => setTimeout(r, 0))
    await reg.send({ sessionId: 's', agentId: 'a', text: 'second' })
    await vi.waitFor(() => expect(reg.get('s')?.phase).toBe('waiting'))
    // The first run ends now; its cleanup must leave the second turn's lock alone.
    failFirst(new Error('late'))
    // Proves an absence (the lock is not released): give the stale run's cleanup time to run.
    await new Promise((r) => setTimeout(r, 20))
    expect(locks.held.has('openruntime-chat:u-1:s')).toBe(true)
    expect(reg.get('s')).toMatchObject({ phase: 'waiting', userText: 'second' })
  })

  it('a double send in one tab (no Web Locks) saves one user row and dispatches once', async () => {
    const { reg, saved, deps } = setup({ locks: null })
    const [a, b] = await Promise.all([
      reg.send({ sessionId: 's', agentId: 'a', text: 'x' }),
      reg.send({ sessionId: 's', agentId: 'a', text: 'x' }),
    ])
    expect(a).toBe(b)
    await settle(reg, 's')
    expect(saved.filter((s) => s.body.role === 'user')).toHaveLength(1)
    expect(deps.dispatch).toHaveBeenCalledOnce()
  })

  it('Save again twice in a row posts the reply once', async () => {
    let n = 0
    const posted: string[] = []
    const { reg } = setup({
      saveMessage: vi.fn(async (sessionId: string, body: SaveMessageBody) => {
        n++
        if (n === 2) throw new ApiError(422, null, '/x', 'bad')
        posted.push(body.role)
        return {
          id: `m-${n}`,
          session_id: sessionId,
          role: body.role,
          content: body.content,
          timestamp: '',
        } as ChatMessage
      }),
    })
    await reg.send({ sessionId: 's', agentId: 'a', text: 'x' })
    await settle(reg, 's')
    expect(reg.get('s')?.phase).toBe('unsaved')
    await Promise.all([reg.saveAgain('s'), reg.saveAgain('s')])
    expect(posted).toEqual(['user', 'assistant'])
  })

  it('a lock manager that throws is treated as no lock, and the send goes ahead', async () => {
    const locks = {
      request: () => Promise.reject(new DOMException('denied', 'SecurityError')),
    } as unknown as Pick<LockManager, 'request'>
    const { reg } = setup({ locks })
    await reg.send({ sessionId: 's', agentId: 'a', text: 'x' })
    await settle(reg, 's')
    expect(reg.get('s')?.phase).toBe('done')
  })

  it('a 401 while saving the user row hands off to the login redirect', async () => {
    const { reg, deps } = setup({
      saveMessage: vi.fn(async () => {
        throw new ApiError(401, null, '/x', 'expired')
      }),
    })
    await reg.send({ sessionId: 's', agentId: 'a', text: 'x' })
    await settle(reg, 's')
    expect(deps.onUnauthorized).toHaveBeenCalledOnce()
    expect(deps.dispatch).not.toHaveBeenCalled()
  })

  it('a pause never saves a partial reply (the server saves the resumed reply)', async () => {
    const frames = CHAT_SCENARIOS['hitl-options']
    const { reg, saved } = setup({}, frames)
    await reg.send({ sessionId: 's', agentId: 'a', text: 'x' })
    await settle(reg, 's')
    expect(reg.get('s')?.phase).toBe('paused')
    expect(saved.map((s) => s.body.role)).toEqual(['user'])
  })

  it('resume leaves an unsaved reply alone', async () => {
    let n = 0
    const { reg, deps } = setup({
      saveMessage: vi.fn(async (sessionId: string, body: SaveMessageBody) => {
        if (++n > 1) throw new ApiError(500, null, '/x', 'down')
        return {
          id: 'm-1',
          session_id: sessionId,
          role: body.role,
          content: body.content,
          timestamp: '',
        } as ChatMessage
      }),
    })
    await reg.send({ sessionId: 's', agentId: 'a', text: 'x' })
    await settle(reg, 's')
    await reg.resume('s', 'h-1', 'a')
    expect(reg.get('s')?.phase).toBe('unsaved')
    expect(deps.dispatch).toHaveBeenCalledOnce()
  })

  it('a send without transcript keeps the chat’s known ownership', async () => {
    const { reg, saved } = setup()
    await reg.send({
      sessionId: 's',
      agentId: 'a',
      text: 'x',
      transcript: { user: 'server', assistant: 'server' },
    })
    await settle(reg, 's')
    await reg.send({ sessionId: 's', agentId: 'a', text: 'y' })
    await settle(reg, 's')
    expect(saved).toHaveLength(0)
  })

  it('busy() is true while a turn runs or a reply is unsaved; onCreateFailed fires on create failure', async () => {
    const onCreateFailed = vi.fn()
    const { reg } = setup({
      createSession: vi.fn(async () => {
        throw new ApiError(500, null, '/x', 'down')
      }),
      dispatch: vi.fn(never),
    })
    expect(reg.busy()).toBe(false)
    const t = await reg.send({ agentId: 'a', text: 'hello', onCreateFailed })
    await settle(reg, t.sessionId)
    expect(onCreateFailed).toHaveBeenCalledWith(t.sessionId, 'hello')
    const live = setup({ dispatch: vi.fn(never) })
    await live.reg.send({ sessionId: 's', agentId: 'a', text: 'x' })
    await vi.waitFor(() => expect(live.reg.busy()).toBe(true))
  })
})

describe('failure branches', () => {
  it('a turn over MAX_TURN_BYTES is too large and execution unknown', async () => {
    const big = 'x'.repeat(1024)
    const frames: MockFrame[] = Array.from(
      { length: Math.ceil(tuning.MAX_TURN_BYTES / 1024) + 5 },
      () => ({ data: artifact(big) }),
    )
    const { reg } = setup({}, frames)
    await reg.send({ sessionId: 's', agentId: 'a', text: 'x' })
    await settle(reg, 's')
    expect(reg.get('s')?.error).toMatchObject({ key: 'tooLarge', certainty: 'unknown' })
    expect(reg.isExecutionUnknown('s')).toBe(true)
  })

  it('a failed user-row save dispatches nothing', async () => {
    const { reg, deps } = setup({
      saveMessage: vi.fn(async () => {
        throw new ApiError(500, null, '/x', 'down')
      }),
    })
    await reg.send({ sessionId: 's', agentId: 'a', text: 'x' })
    await settle(reg, 's')
    expect(reg.get('s')?.error).toMatchObject({
      key: 'saveUserFailed',
      certainty: 'not-dispatched',
    })
    expect(deps.dispatch).not.toHaveBeenCalled()
  })

  it('Stop receiving before any text saves nothing and lands in no_reply', async () => {
    const { reg, saved } = setup({
      dispatch: vi.fn(
        async (_b: unknown, signal: AbortSignal) =>
          new Response(
            new ReadableStream({
              start(c) {
                signal.addEventListener('abort', () =>
                  c.error(new DOMException('Aborted', 'AbortError')),
                )
              },
            }),
          ),
      ),
    })
    await reg.send({ sessionId: 's', agentId: 'a', text: 'x' })
    await vi.waitFor(() => expect(reg.get('s')?.phase).toBe('waiting'))
    reg.stop('s')
    await settle(reg, 's')
    expect(reg.get('s')?.phase).toBe('no_reply')
    expect(saved.map((s) => s.body.role)).toEqual(['user'])
  })

  it('a definite (4xx) save failure saves again without checking history', async () => {
    let n = 0
    const { reg, deps } = setup({
      saveMessage: vi.fn(async (sessionId: string, body: SaveMessageBody) => {
        if (++n === 2) throw new ApiError(422, null, '/x', 'bad')
        return {
          id: `m-${n}`,
          session_id: sessionId,
          role: body.role,
          content: body.content,
          timestamp: '',
        } as ChatMessage
      }),
    })
    await reg.send({ sessionId: 's', agentId: 'a', text: 'x' })
    await settle(reg, 's')
    expect(reg.get('s')?.error).toMatchObject({ key: 'saveDefinite' })
    await reg.saveAgain('s')
    expect(deps.replyExists).not.toHaveBeenCalled()
    expect(reg.get('s')?.phase).toBe('done')
  })

  it.each([400, 404, 409, 429])(
    'a %i on resume reconciles through history instead of failing',
    async (code) => {
      const { reg } = setup({ dispatch: vi.fn(async () => new Response('nope', { status: code })) })
      await reg.resume('s', 'h-1', 'a')
      await settle(reg, 's')
      expect(reg.get('s')).toMatchObject({ phase: 'done', error: null })
    },
  )
})

describe('review fixes, cycle 2', () => {
  it('a send while a resume waits for its lock is refused, not swallowed', async () => {
    let grant!: () => void
    const locks = {
      request: (_n: string, _o: unknown, cb: (l: { name: string }) => Promise<void>) =>
        new Promise<void>((r) => {
          grant = () => void cb({ name: 'x' }).then(r)
        }),
    } as unknown as Pick<LockManager, 'request'>
    const { reg, deps } = setup({
      locks,
      dispatch: vi.fn(() => new Promise<Response>(() => undefined)),
    })
    const resuming = reg.resume('s', 'h-1', 'a')
    await expect(reg.send({ sessionId: 's', agentId: 'a', text: 'mine' })).rejects.toMatchObject({
      key: 'busy',
    })
    grant()
    await resuming
    expect(deps.saveMessage).not.toHaveBeenCalled()
  })

  it('onStart gets the minted id before create runs', async () => {
    const order: string[] = []
    const { reg } = setup({
      createSession: vi.fn(async (b) => {
        order.push('create')
        return {
          session_id: b.session_id,
          agent_id: b.agent_id,
          title: '',
          created_at: '',
          updated_at: '',
        } as ChatSessionRow
      }),
    })
    const t = await reg.send({
      agentId: 'a',
      text: 'x',
      onStart: (id) => order.push(`start:${id}`),
    })
    await settle(reg, t.sessionId)
    expect(order).toEqual([`start:${t.sessionId}`, 'create'])
  })
})

describe('/ship review fixes', () => {
  const never = () => new Promise<never>(() => undefined)

  it('Stop receiving on a paused turn does nothing and saves nothing', async () => {
    const { reg, saved } = setup({}, CHAT_SCENARIOS['hitl-options'])
    await reg.send({ sessionId: 's', agentId: 'a', text: 'x' })
    await settle(reg, 's')
    reg.stop('s')
    expect(reg.get('s')?.phase).toBe('paused')
    expect(saved.map((s) => s.body.role)).toEqual(['user'])
  })

  it('a chat deleted while its reply is saving stays aborted and never holds the leave prompt', async () => {
    let failSave!: (e: Error) => void
    let n = 0
    const { reg } = setup({
      saveMessage: vi.fn((sessionId: string, body: SaveMessageBody) => {
        if (++n === 1)
          return Promise.resolve({
            id: 'm-1',
            session_id: sessionId,
            role: body.role,
            content: body.content,
            timestamp: '',
          } as ChatMessage)
        return new Promise<ChatMessage>((_r, reject) => {
          failSave = reject
        })
      }),
    })
    await reg.send({ sessionId: 's', agentId: 'a', text: 'x' })
    await vi.waitFor(() => expect(n).toBe(2))
    reg.abort('s')
    failSave(new ApiError(404, null, '/x', 'gone'))
    // Proves an absence (no phase change after the late failure): let its handlers run first.
    await new Promise((r) => setTimeout(r, 20))
    expect(reg.get('s')?.phase).toBe('aborted')
    expect(reg.busy()).toBe(false)
  })

  it('a second, different message while the first is starting is refused, not swallowed', async () => {
    let grant!: () => void
    const locks = {
      request: (_n: string, _o: unknown, cb: (l: { name: string }) => Promise<void>) =>
        new Promise<void>((r) => {
          grant = () => void cb({ name: 'x' }).then(r)
        }),
    } as unknown as Pick<LockManager, 'request'>
    const { reg } = setup({ locks, dispatch: vi.fn(never) })
    const first = reg.send({ sessionId: 's', agentId: 'a', text: 'first' })
    await expect(reg.send({ sessionId: 's', agentId: 'a', text: 'second' })).rejects.toMatchObject({
      key: 'busy',
    })
    grant()
    await first
  })

  it('a Run again that cannot start removes the history turn it made', async () => {
    const locks = {
      request: (_n: string, _o: unknown, cb: (l: null) => unknown) => Promise.resolve(cb(null)),
    } as unknown as Pick<LockManager, 'request'>
    const { reg } = setup({ locks })
    await expect(
      reg.runAgain('s', { agentId: 'a', userText: 'from history', userMessageId: 'm-9' }),
    ).rejects.toMatchObject({ key: 'otherTab' })
    expect(reg.get('s')).toBeUndefined()
  })

  it('Save again whose history check fails stays unsaved with that error (and a 401 goes to login)', async () => {
    let n = 0
    const { reg, deps } = setup({
      saveMessage: vi.fn(async (sessionId: string, body: SaveMessageBody) => {
        if (++n === 2) throw new ApiError(500, null, '/x', 'down')
        return {
          id: `m-${n}`,
          session_id: sessionId,
          role: body.role,
          content: body.content,
          timestamp: '',
        } as ChatMessage
      }),
      replyExists: vi.fn(async () => {
        throw new ApiError(401, null, '/x', 'expired')
      }),
    })
    await reg.send({ sessionId: 's', agentId: 'a', text: 'x' })
    await settle(reg, 's')
    expect(reg.get('s')?.error).toMatchObject({ key: 'saveUnknown' })
    await expect(reg.saveAgain('s')).resolves.toBeUndefined()
    expect(reg.get('s')?.phase).toBe('unsaved')
    expect(deps.onUnauthorized).toHaveBeenCalledOnce()
  })
})
