import { describe, expect, it } from 'vitest'
import { mergeTurns, replyStatus } from './turnModel'
import type { ChatMessage, HitlDto, MessagesPage } from './types'

const msg = (id: string, role: ChatMessage['role'], t: string, content = id): ChatMessage => ({
  id,
  session_id: 's',
  role,
  content,
  timestamp: `2026-09-27T10:${t}:00Z`,
})
const page = (data: ChatMessage[], hitl: HitlDto[] = []): MessagesPage => ({
  data,
  has_more: false,
  next_cursor: null,
  prev_cursor: null,
  hitl,
})
const req = (id: string, t: string, status: HitlDto['status'], resolved?: string): HitlDto => ({
  id,
  kind: 'input_required',
  status,
  resume_status: 'not_started',
  question: { message: '?' },
  human_response: null,
  execution: {
    origin: 'direct_chat',
    agent_id: 'a',
    task_id: null,
    context_id: 's',
    chat_session_id: null,
    maf_execution_id: null,
    maf_step_index: null,
  },
  allowed_actions: ['answer', 'cancel'],
  expires_at: '2026-10-04T10:00:00Z',
  created_at: `2026-09-27T10:${t}:00Z`,
  resolved_at: resolved ? `2026-09-27T10:${resolved}:00Z` : null,
})

describe('mergeTurns', () => {
  it('groups rows into turns and dedupes across pages', () => {
    const newest = page([msg('u2', 'user', '05'), msg('r2', 'assistant', '06')])
    const older = page([
      msg('u1', 'user', '01'),
      msg('r1', 'assistant', '02'),
      msg('u2', 'user', '05'),
    ])
    const turns = mergeTurns([newest, older])
    expect(turns.map((t) => [t.user?.id, t.replies.map((r) => r.id)])).toEqual([
      ['u1', ['r1']],
      ['u2', ['r2']],
    ])
  })

  it('puts rows before the first loaded user message under a boundary turn', () => {
    const turns = mergeTurns([page([msg('r0', 'assistant', '00'), msg('u1', 'user', '01')])])
    expect(turns[0]).toMatchObject({ boundary: true, user: null })
    expect(turns[1].user?.id).toBe('u1')
  })

  it('hides system rows', () => {
    expect(
      mergeTurns([page([msg('u1', 'user', '01'), msg('x', 'system', '02')])])[0].replies,
    ).toEqual([])
  })

  it('attaches requests to the turn they belong to', () => {
    const turns = mergeTurns([
      page([msg('u1', 'user', '01'), msg('u2', 'user', '05')], [req('h1', '03', 'resolved', '04')]),
    ])
    expect(turns[0].requests.map((r) => r.id)).toEqual(['h1'])
    expect(turns[1].requests).toEqual([])
  })

  it('shows a live turn before history has its user row, then attaches it, then drops it once saved', () => {
    const live = {
      userMessageId: undefined as string | undefined,
      userText: 'hi',
      finalized: false,
    }
    expect(mergeTurns([page([])], live).at(-1)).toMatchObject({ live, user: null })
    live.userMessageId = 'u1'
    const attached = mergeTurns([page([msg('u1', 'user', '01')])], live)
    expect(attached).toHaveLength(1)
    expect(attached[0].live).toBe(live)
    const done = mergeTurns([page([msg('u1', 'user', '01'), msg('r1', 'assistant', '02')])], {
      ...live,
      finalized: true,
    })
    expect(done[0].live).toBeUndefined()
    expect(done[0].replies.map((r) => r.id)).toEqual(['r1'])
  })
})

describe('replyStatus', () => {
  const at = (t: string) => Date.parse(`2026-09-27T10:${t}:00Z`)
  const window = 10 * 60_000

  it('is null when answered or live', () => {
    expect(
      replyStatus(
        mergeTurns([page([msg('u1', 'user', '01'), msg('r1', 'assistant', '02')])]),
        at('30'),
        window,
      ),
    ).toBeNull()
    expect(
      replyStatus(
        mergeTurns([page([msg('u1', 'user', '01')])], {
          userMessageId: 'u1',
          userText: 'x',
          finalized: false,
        }),
        at('30'),
        window,
      ),
    ).toBeNull()
  })

  it('checks inside the window and is unconfirmed after it', () => {
    const turns = mergeTurns([page([msg('u1', 'user', '01')])])
    expect(replyStatus(turns, at('05'), window)).toBe('checking')
    expect(replyStatus(turns, at('12'), window)).toBe('unconfirmed')
  })

  it('a pending request suppresses it; a canceled one closes the turn', () => {
    expect(
      replyStatus(
        mergeTurns([page([msg('u1', 'user', '01')], [req('h', '02', 'pending')])]),
        at('50'),
        window,
      ),
    ).toBeNull()
    expect(
      replyStatus(
        mergeTurns([page([msg('u1', 'user', '01')], [req('h', '02', 'canceled', '03')])]),
        at('50'),
        window,
      ),
    ).toBeNull()
  })

  it('a resolved or rejected request restarts the clock at resolution', () => {
    const turns = mergeTurns([page([msg('u1', 'user', '01')], [req('h', '02', 'rejected', '20')])])
    expect(replyStatus(turns, at('25'), window)).toBe('checking')
    expect(replyStatus(turns, at('31'), window)).toBe('unconfirmed')
  })
})

describe('timestamps with different fraction lengths', () => {
  it('orders by instant, not by string', () => {
    const m = (id: string, role: 'user' | 'assistant', timestamp: string) => ({
      id,
      session_id: 's',
      role,
      content: id,
      timestamp,
    })
    const page = {
      data: [
        m('reply', 'assistant', '2026-09-27T10:00:00.5Z'),
        m('ask', 'user', '2026-09-27T10:00:00Z'),
      ],
      has_more: false,
      next_cursor: null,
      prev_cursor: null,
      hitl: [],
    }
    const turns = mergeTurns([page])
    expect(turns).toHaveLength(1)
    expect(turns[0]!.user?.id).toBe('ask')
    expect(turns[0]!.replies.map((r) => r.id)).toEqual(['reply'])
  })
})
