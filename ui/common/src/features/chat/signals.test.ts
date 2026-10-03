/**
 * Chat v1c M3 (plans/feat-chat-v1c.md §5.8, §7 tests 6, 8 and 20): direct turn ends in the registry, the
 * signals store (seen, completions, Reply ready, the open chat) and the hidden-tab title count.
 */
import { afterEach, describe, expect, it, vi } from 'vitest'
import { CHAT_SCENARIOS, sseResponse, artifact, status, type MockFrame } from '@/mocks/chat'
import { createSignals } from './signals'
import { tuning } from './tuning'
import {
  createTurnRegistry,
  isLivePhase,
  type ChatDeps,
  type LiveTurn,
  type TurnEnd,
  type TurnRegistry,
} from './turnRegistry'
import type { ChatMessage, ChatSessionRow, SaveMessageBody } from './types'

// ─── The registry: direct ends (test 8) ───────────────────────────────────────

function setup(over: Partial<ChatDeps> = {}, frames: MockFrame[] = CHAT_SCENARIOS['direct-plain']) {
  let n = 0
  let m = 0
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
    saveMessage: vi.fn(
      async (sessionId: string, body: SaveMessageBody) =>
        ({
          id: `m-${++m}`,
          session_id: sessionId,
          role: body.role,
          content: body.content,
          timestamp: '',
        }) as ChatMessage,
    ),
    dispatch: vi.fn(async () => sseResponse(frames)),
    replyExists: vi.fn(async () => false),
    onUnauthorized: vi.fn(),
    newId: () => `id-${++n}`,
    now: () => Date.now(),
    locks: null,
    warn: vi.fn(),
    onTurnEnd: vi.fn(),
    onDirectSaved: vi.fn(),
    ...over,
  }
  const reg = createTurnRegistry(deps)
  const got: TurnEnd[] = []
  reg.subscribeEnds((e) => got.push(e))
  return { deps, reg, got }
}

const settle = (reg: TurnRegistry, id: string, phase?: LiveTurn['phase']) =>
  vi.waitFor(
    () => {
      const t = reg.get(id)
      if (!t || isLivePhase(t.phase) || (phase && t.phase !== phase))
        throw new Error(`still ${t?.phase}`)
    },
    { timeout: 2000 },
  )

/** A save that waits for `release()`; `fail` makes it reject then. */
function gatedSave() {
  let release!: (fail?: boolean) => void
  const gate = new Promise<boolean>((r) => {
    release = (fail = false) => r(fail)
  })
  let n = 0
  const saveMessage = vi.fn(async (sessionId: string, body: SaveMessageBody) => {
    if (body.role === 'assistant' && (await gate)) throw new Error('save failed')
    return {
      id: `m-${++n}`,
      session_id: sessionId,
      role: body.role,
      content: body.content,
      timestamp: '',
    } as ChatMessage
  })
  return { saveMessage, release: (fail?: boolean) => release(fail) }
}

const kinds = (got: TurnEnd[]) => got.map((e) => [e.attemptKey, e.kind])

afterEach(() => {
  vi.useRealTimers()
  vi.restoreAllMocks()
})

describe('direct turn ends (test 8)', () => {
  it('done with reply text: one reply end, only after the save settles; never onTurnEnd, never endFor', async () => {
    const { saveMessage, release } = gatedSave()
    const { reg, got, deps } = setup({ saveMessage })
    await reg.send({ sessionId: 's', agentId: 'a', text: 'hi' })
    await vi.waitFor(() => expect(saveMessage).toHaveBeenCalledTimes(2))
    expect(got).toEqual([])
    release()
    await settle(reg, 's', 'done')
    expect(kinds(got)).toEqual([['m-1:send', 'reply']])
    expect(got[0]).toMatchObject({ chatMode: 'direct', sessionId: 's', userMessageId: 'm-1' })
    expect(deps.onTurnEnd).not.toHaveBeenCalled()
    expect(reg.endFor('s', 'm-1')).toBeUndefined()
    expect(reg.directEnds()).toHaveLength(1)
    expect(deps.onDirectSaved).toHaveBeenCalledWith('s')
  })

  it('no_reply records empty; the page keeps its E5 status (endFor stays routed-only)', async () => {
    const { reg, got } = setup({}, CHAT_SCENARIOS['empty-reply'])
    await reg.send({ sessionId: 's', agentId: 'a', text: 'hi' })
    await settle(reg, 's', 'no_reply')
    expect(kinds(got)).toEqual([['m-1:send', 'empty']])
    expect(reg.endFor('s', 'm-1')).toBeUndefined()
  })

  it('error records error; Run again is a new attempt with its own reply', async () => {
    let first = true
    const { reg, got } = setup({
      dispatch: vi.fn(async () =>
        sseResponse(
          first ? ((first = false), CHAT_SCENARIOS.failed) : CHAT_SCENARIOS['direct-plain'],
        ),
      ),
    })
    await reg.send({ sessionId: 's', agentId: 'a', text: 'hi' })
    await settle(reg, 's', 'error')
    await reg.runAgain('s')
    await settle(reg, 's', 'done')
    expect(kinds(got)).toEqual([
      ['m-1:send', 'error'],
      ['m-1:rerun:1', 'reply'],
    ])
  })

  it('unsaved, then a Save again that succeeds, supersedes the error with <key>:reply', async () => {
    let fail = true
    const { reg, got } = setup({
      saveMessage: vi.fn(async (sessionId: string, body: SaveMessageBody) => {
        if (body.role === 'assistant' && fail) {
          fail = false
          throw new Error('boom')
        }
        return {
          id: body.role === 'user' ? 'm-1' : 'm-2',
          session_id: sessionId,
          role: body.role,
          content: body.content,
          timestamp: '',
        } as ChatMessage
      }),
    })
    await reg.send({ sessionId: 's', agentId: 'a', text: 'hi' })
    await settle(reg, 's', 'unsaved')
    await reg.saveAgain('s')
    await settle(reg, 's', 'done')
    expect(kinds(got)).toEqual([
      ['m-1:send', 'error'],
      ['m-1:send:reply', 'reply'],
    ])
  })

  it('a save still pending at the cap ends as a reply; the later save adds nothing, a later failure adds <key>:error', async () => {
    const cap = tuning.DIRECT_END_SAVE_CAP_MS
    tuning.DIRECT_END_SAVE_CAP_MS = 40
    try {
      const ok = gatedSave()
      const a = setup({ saveMessage: ok.saveMessage })
      await a.reg.send({ sessionId: 's', agentId: 'a', text: 'hi' })
      await vi.waitFor(() => expect(kinds(a.got)).toEqual([['m-1:send', 'reply']]))
      ok.release()
      await settle(a.reg, 's', 'done')
      expect(kinds(a.got)).toEqual([['m-1:send', 'reply']])
      // Persistence still refreshes once the save settles (E4).
      expect(a.deps.onDirectSaved).toHaveBeenCalledWith('s')

      const bad = gatedSave()
      const b = setup({ saveMessage: bad.saveMessage })
      await b.reg.send({ sessionId: 's', agentId: 'a', text: 'hi' })
      await vi.waitFor(() => expect(b.got).toHaveLength(1))
      bad.release(true)
      await settle(b.reg, 's', 'unsaved')
      expect(kinds(b.got)).toEqual([
        ['m-1:send', 'reply'],
        ['m-1:send:error', 'error'],
      ])
    } finally {
      tuning.DIRECT_END_SAVE_CAP_MS = cap
    }
  })

  it('paused, then a direct resume: paused, then a reply for the resume attempt; a silent resume is empty', async () => {
    let call = 0
    const replies = [
      CHAT_SCENARIOS['hitl-options'],
      CHAT_SCENARIOS['direct-plain'],
      [{ data: status('TASK_STATE_COMPLETED') }],
    ]
    const { reg, got } = setup({ dispatch: vi.fn(async () => sseResponse(replies[call++]!)) })
    await reg.send({ sessionId: 's', agentId: 'a', text: 'hi' })
    await settle(reg, 's', 'paused')
    await reg.resume('s', 'h-1', 'a', 'm-1')
    await settle(reg, 's', 'done')
    await reg.resume('s', 'h-2', 'a', 'm-1')
    await vi.waitFor(() => expect(got).toHaveLength(3))
    expect(kinds(got)).toEqual([
      ['m-1:send', 'paused'],
      ['m-1:resume:h-1', 'reply'],
      ['m-1:resume:h-2', 'empty'],
    ])
  })

  it('a stopped attempt records nothing, cap and Save again included; an aborted one neither', async () => {
    const cap = tuning.DIRECT_END_SAVE_CAP_MS
    tuning.DIRECT_END_SAVE_CAP_MS = 30
    try {
      const slow: MockFrame[] = [
        { data: status('TASK_STATE_WORKING') },
        { data: artifact('part', { append: false }) },
        { data: artifact(' more', { append: true, lastChunk: true }), delayMs: 300 },
        { data: status('TASK_STATE_COMPLETED') },
      ]
      const a = setup({}, slow)
      await a.reg.send({ sessionId: 's', agentId: 'a', text: 'hi' })
      await vi.waitFor(() => expect(a.reg.get('s')?.phase).toBe('streaming'))
      a.reg.stop('s')
      await settle(a.reg, 's')
      // Proves an absence (no end after the stop), past the save cap.
      await new Promise((r) => setTimeout(r, 60))
      expect(a.got).toEqual([])

      const b = setup({}, slow)
      await b.reg.send({ sessionId: 's', agentId: 'a', text: 'hi' })
      await vi.waitFor(() => expect(b.reg.get('s')?.phase).toBe('streaming'))
      b.reg.abort('s')
      // Proves an absence (no end after the abort), past the slow scenario's last frame.
      await new Promise((r) => setTimeout(r, 400))
      expect(b.got).toEqual([])

      // Stopped, then its partial reply's save hangs past the cap, fails (unsaved) and Save again succeeds.
      let fail = true
      const gate = gatedSave()
      const c = setup(
        {
          saveMessage: vi.fn(async (sessionId: string, body: SaveMessageBody) => {
            if (body.role === 'assistant') {
              // A save slower than the cap: the scenario, not a wait for the code under test.
              await new Promise((r) => setTimeout(r, 60))
              if (fail) {
                fail = false
                throw new Error('boom')
              }
            }
            return gate.saveMessage(sessionId, { ...body, role: 'user' })
          }),
        },
        slow,
      )
      await c.reg.send({ sessionId: 's', agentId: 'a', text: 'hi' })
      await vi.waitFor(() => expect(c.reg.get('s')?.phase).toBe('streaming'))
      c.reg.stop('s')
      await settle(c.reg, 's')
      if (c.reg.get('s')?.phase === 'unsaved') {
        await c.reg.saveAgain('s')
        await settle(c.reg, 's')
      }
      // Proves an absence (no end for the stopped, re-saved reply).
      await new Promise((r) => setTimeout(r, 60))
      expect(c.got).toEqual([])
    } finally {
      tuning.DIRECT_END_SAVE_CAP_MS = cap
    }
  })

  it('a chat whose server writes the user row: one end per attempt, keyed by the turn (E12)', async () => {
    const transcript = { user: 'server', assistant: 'server' } as const
    const { reg, got, deps } = setup()
    await reg.send({ sessionId: 's', agentId: 'a', text: 'hi', transcript })
    await settle(reg, 's', 'done')
    expect(deps.saveMessage).not.toHaveBeenCalled()
    const turn = reg.get('s')!.id
    expect(kinds(got)).toEqual([[`${turn}:send`, 'reply']])
    expect(got[0]!.userMessageId).toBeUndefined()
    // Run again is its own attempt on the same turn.
    await reg.runAgain('s')
    await settle(reg, 's', 'done')
    expect(kinds(got)).toEqual([
      [`${turn}:send`, 'reply'],
      [`${turn}:rerun:1`, 'reply'],
    ])
  })

  it('a server-written user row, but the create fails: no end (E12)', async () => {
    const { reg, got } = setup({
      createSession: vi.fn(async () => {
        throw new Error('create failed')
      }),
    })
    await reg.send({
      agentId: 'a',
      text: 'hi',
      transcript: { user: 'server', assistant: 'server' },
    })
    await vi.waitFor(() => expect(reg.snapshot()[0]?.phase).toBe('error'))
    expect(got).toEqual([])
  })

  it('no end before the user row exists (E12): a failed create records none', async () => {
    const { reg, got } = setup({
      createSession: vi.fn(async () => {
        throw new Error('create failed')
      }),
    })
    await reg.send({ agentId: 'a', text: 'hi' })
    await vi.waitFor(() => expect(reg.snapshot()[0]?.phase).toBe('error'))
    expect(got).toEqual([])
  })

  it('clearing the registry during a pending save stops the cap: no end, ever (E2)', async () => {
    const cap = tuning.DIRECT_END_SAVE_CAP_MS
    // Longer than waitFor's 50 ms poll, so the clear lands while the save is still pending.
    tuning.DIRECT_END_SAVE_CAP_MS = 300
    try {
      const { saveMessage } = gatedSave()
      const { reg, got } = setup({ saveMessage })
      await reg.send({ sessionId: 's', agentId: 'a', text: 'hi' })
      await vi.waitFor(() => expect(saveMessage).toHaveBeenCalledTimes(2))
      expect(got).toEqual([])
      reg.clearAll()
      // Proves an absence (the cleared cap never fires), past DIRECT_END_SAVE_CAP_MS.
      await new Promise((r) => setTimeout(r, 400))
      expect(got).toEqual([])
    } finally {
      tuning.DIRECT_END_SAVE_CAP_MS = cap
    }
  })
})

// ─── The signals store (test 6) ───────────────────────────────────────────────

function fakeDoc(title = 'Base') {
  const listeners = new Set<() => void>()
  const doc = {
    visibilityState: 'visible' as DocumentVisibilityState,
    title,
    addEventListener: (_: 'visibilitychange', l: () => void) => void listeners.add(l),
    removeEventListener: (_: 'visibilitychange', l: () => void) => void listeners.delete(l),
    set(v: DocumentVisibilityState) {
      this.visibilityState = v
      listeners.forEach((l) => l())
    },
  }
  return doc
}

function signalsWith(doc = fakeDoc()) {
  // Date.now plus a nudge per end, so fake timers move the clock too.
  let nudge = 0
  const clock = () => Date.now() + nudge
  const endListeners = new Set<(e: TurnEnd) => void>()
  const turnListeners = new Set<() => void>()
  let live: LiveTurn[] = []
  const registry = {
    subscribe: (l: () => void) => {
      turnListeners.add(l)
      return () => turnListeners.delete(l)
    },
    subscribeEnds: (l: (e: TurnEnd) => void) => {
      endListeners.add(l)
      return () => endListeners.delete(l)
    },
    snapshot: () => live,
  } as unknown as Pick<TurnRegistry, 'subscribe' | 'subscribeEnds' | 'snapshot'>
  const invalidate = vi.fn()
  const onBackgroundError = vi.fn()
  const s = createSignals({ registry, now: clock, invalidate, onBackgroundError, doc })
  const end = (
    sessionId: string,
    kind: TurnEnd['kind'],
    attemptKey = `${sessionId}-u:send`,
    chatMode: 'direct' | 'routed' = 'direct',
  ) => {
    nudge += 10
    endListeners.forEach((l) =>
      l({ id: 't', sessionId, attemptKey, kind, finishedAt: clock(), traceId: null, chatMode }),
    )
  }
  const setLive = (ids: string[]) => {
    live = ids.map((sessionId) => ({ sessionId, phase: 'streaming' }) as LiveTurn)
    turnListeners.forEach((l) => l())
  }
  return { s, end, setLive, invalidate, onBackgroundError, doc }
}

describe('signals (test 6)', () => {
  it('order B, open then finish: no dot; order A, finish with no page open: a dot and Reply ready', () => {
    const { s, end } = signalsWith()
    const token = s.setOpenChat('a')
    end('a', 'reply')
    expect(s.row('a')).toMatchObject({ unseen: false })
    expect(s.replyReady()).toBeNull()
    s.releaseOpenChat(token)
    end('b', 'reply')
    expect(s.row('b')).toMatchObject({ unseen: true })
    expect(s.replyReady()).toMatchObject({ sessionId: 'b' })
    expect(s.unseenChats()).toBe(1)
  })

  it('keeps at most TURN_ENDS_MAX completions, dropping the oldest', () => {
    const { s, end } = signalsWith()
    const max = tuning.TURN_ENDS_MAX
    for (let i = 0; i <= max; i++) end(`c${i}`, 'reply')
    expect(s.row('c0')).toMatchObject({ unseen: false })
    expect(s.row(`c${max}`)).toMatchObject({ unseen: true })
    expect(s.unseenChats()).toBe(max)
  })

  it('a routed Run again earns its own dot; the same end twice counts once', () => {
    const { s, end } = signalsWith()
    end('a', 'reply', 'a-u:send', 'routed')
    const t = s.setOpenChat('a')
    s.releaseOpenChat(t)
    expect(s.row('a').unseen).toBe(false)
    end('a', 'reply', 'a-u:rerun:1', 'routed')
    end('a', 'reply', 'a-u:rerun:1', 'routed')
    expect(s.row('a').unseen).toBe(true)
    expect(s.titlePrefix()).toBe('')
  })

  it('only a reply earns the dot and Reply ready; a failure earns the failed mark and is announced once', () => {
    const { s, end, onBackgroundError } = signalsWith()
    end('a', 'error')
    end('a', 'error')
    expect(s.row('a')).toEqual({ live: false, failed: true, unseen: false })
    expect(s.replyReady()).toBeNull()
    expect(onBackgroundError).toHaveBeenCalledTimes(1)
  })

  it("keeps each attempt's current outcome: done → lost swaps the dot for the failed mark; a late saved reply earns it back (E3)", () => {
    const { s, end } = signalsWith()
    end('a', 'reply', 'a-u:send', 'routed')
    end('a', 'error', 'a-u:send', 'routed')
    expect(s.row('a')).toMatchObject({ failed: true, unseen: false })
    end('a', 'reply', 'a-u:send', 'routed')
    expect(s.row('a')).toMatchObject({ failed: false, unseen: true })
    // A superseding :reply belongs to the same attempt.
    end('b', 'error', 'b-u:send')
    end('b', 'reply', 'b-u:send:reply')
    expect(s.row('b')).toMatchObject({ failed: false, unseen: true })
  })

  it('refreshes a direct chat that ended with no page open, once; never the open one or a routed one', () => {
    const { s, end, invalidate } = signalsWith()
    end('a', 'reply')
    expect(invalidate).toHaveBeenCalledWith('a')
    s.setOpenChat('b')
    end('b', 'reply')
    end('c', 'reply', 'c-u:send', 'routed')
    expect(invalidate).toHaveBeenCalledTimes(1)
  })

  it('a reply that lands while the tab is hidden counts, and clears once the tab is visible with that chat open', () => {
    const doc = fakeDoc()
    const { s, end } = signalsWith(doc)
    s.setOpenChat('a')
    doc.set('hidden')
    end('a', 'reply')
    expect(s.row('a').unseen).toBe(true)
    doc.set('visible')
    expect(s.row('a').unseen).toBe(false)
  })

  it('the live spinner follows the registry', () => {
    const { s, setLive } = signalsWith()
    setLive(['a'])
    expect(s.row('a').live).toBe(true)
    setLive([])
    expect(s.row('a').live).toBe(false)
  })

  it('Reply ready shows the newest unseen reply; dismiss hides every reply unseen then, their dots stay', () => {
    const { s, end } = signalsWith()
    end('a', 'reply')
    end('b', 'reply')
    expect(s.replyReady()?.sessionId).toBe('b')
    s.dismissReplyReady()
    expect(s.replyReady()).toBeNull()
    expect(s.row('a').unseen && s.row('b').unseen).toBe(true)
    end('c', 'reply')
    expect(s.replyReady()?.sessionId).toBe('c')
    expect(s.firstNotice('c-u:send')).toBe(true)
    expect(s.firstNotice('c-u:send')).toBe(false)
  })

  it('Reply ready times out after REPLY_READY_MS of visible time; the clock pauses while hidden (DS4)', () => {
    vi.useFakeTimers()
    const doc = fakeDoc()
    const { s, end } = signalsWith(doc)
    end('a', 'reply')
    const r = s.replyReady()!
    s.replyReadyShown(r.attempt)
    vi.advanceTimersByTime(tuning.REPLY_READY_MS - 10)
    doc.set('hidden')
    vi.advanceTimersByTime(tuning.REPLY_READY_MS * 3)
    doc.set('visible')
    expect(s.replyReady()).not.toBeNull()
    vi.advanceTimersByTime(20)
    expect(s.replyReady()).toBeNull()
    expect(s.row('a').unseen).toBe(true)
  })

  it('the open-chat token: /chat/a → /chat/b never clears b (E10)', () => {
    const { s } = signalsWith()
    const ta = s.setOpenChat('a')
    const tb = s.setOpenChat('b')
    s.releaseOpenChat(ta)
    expect(s.openChat()).toBe('b')
    s.releaseOpenChat(tb)
    expect(s.openChat()).toBeNull()
  })
})

describe('hidden-tab title count (test 20, C4, E11)', () => {
  it('prefixes the title while hidden, counting replies that finished before, and restores it', () => {
    const doc = fakeDoc('Agents · OpenRuntime')
    const { end } = signalsWith(doc)
    end('a', 'reply')
    expect(doc.title).toBe('Agents · OpenRuntime')
    doc.set('hidden')
    expect(doc.title).toBe('(1) Agents · OpenRuntime')
    end('b', 'reply')
    expect(doc.title).toBe('(2) Agents · OpenRuntime')
    doc.set('visible')
    expect(doc.title).toBe('Agents · OpenRuntime')
  })

  it('a title someone else changed while hidden is never overwritten back', () => {
    const doc = fakeDoc('Base')
    const { end } = signalsWith(doc)
    doc.set('hidden')
    end('a', 'reply')
    doc.title = 'Theirs'
    end('b', 'reply')
    expect(doc.title).toBe('(2) Theirs')
    doc.title = 'Changed again'
    doc.set('visible')
    expect(doc.title).toBe('Changed again')
  })

  it('while a chat page owns the title it gives the prefix instead, and leaving never double-prefixes', () => {
    const doc = fakeDoc('Chat · OpenRuntime')
    const { s, end } = signalsWith(doc)
    const release = s.ownTitle()
    doc.set('hidden')
    end('a', 'reply')
    expect(s.titlePrefix()).toBe('(1) ')
    expect(doc.title).toBe('Chat · OpenRuntime')
    // React rendered the prefix; the page unmounts while hidden.
    doc.title = '(1) Chat · OpenRuntime'
    release()
    expect(doc.title).toBe('(1) Chat · OpenRuntime')
    end('b', 'reply')
    expect(doc.title).toBe('(2) Chat · OpenRuntime')
  })

  it('dispose restores the title (clearChatRegistry)', () => {
    const doc = fakeDoc('Base')
    const { s, end } = signalsWith(doc)
    doc.set('hidden')
    end('a', 'reply')
    s.dispose()
    expect(doc.title).toBe('Base')
  })
})
