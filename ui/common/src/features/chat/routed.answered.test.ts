// Ship review D3 (round 2): an answered routed request with no reply after it. The wait follows
// the server's resume_status, ignores what this tab saw end, and can't be stretched by clock skew.
import { describe, expect, it } from 'vitest'
import { answeredResume, replyStatus, type DisplayTurn } from './turnModel'
import { tuning } from './tuning'
import { sawTerminal } from './turnRegistry'
import type { HitlDto } from './types'

const T0 = Date.parse('2026-09-27T12:00:00Z')
const req = (over: Partial<HitlDto>): HitlDto =>
  ({
    id: 'h1',
    kind: 'input_required',
    status: 'resolved',
    resume_status: 'completed',
    question: null,
    human_response: null,
    execution: {
      origin: 'orchestrator',
      agent_id: 'a1',
      task_id: null,
      context_id: null,
      chat_session_id: 's1',
      maf_execution_id: null,
      maf_step_index: null,
    },
    allowed_actions: [],
    expires_at: null,
    created_at: new Date(T0 - 60_000).toISOString(),
    resolved_at: new Date(T0).toISOString(),
    ...over,
  }) as HitlDto
const turn = (requests: HitlDto[], replies = 0): DisplayTurn => ({
  key: 'u1',
  requests,
  user: {
    id: 'u1',
    role: 'user',
    content: 'deploy',
    timestamp: new Date(T0 - 120_000).toISOString(),
  } as DisplayTurn['user'],
  replies: Array.from({ length: replies }, (_, i) => ({
    id: `r${i}`,
    role: 'assistant',
    content: 'ok',
    timestamp: new Date(T0 + 1000).toISOString(),
  })) as DisplayTurn['replies'],
})
const seenAt = (at: number) => () => at
const opts = { seenEnded: false, firstSeen: seenAt(T0) }

describe('answeredResume', () => {
  it('is null without an answered request, with a reply, or while a request is pending', () => {
    expect(answeredResume(turn([]), T0, opts)).toBeNull()
    expect(answeredResume(turn([req({})], 1), T0, opts)).toBeNull()
    expect(
      answeredResume(
        turn([req({}), req({ id: 'h2', status: 'pending', resolved_at: null })]),
        T0,
        opts,
      ),
    ).toBeNull()
    expect(answeredResume(turn([req({ status: 'canceled' })]), T0, opts)).toBeNull()
  })

  it('a delivered answer gets the whole delivery window (completed can land late in it)', () => {
    // Delivered on a retry 15 min after the answer: the orchestrator's turn is still to run.
    expect(answeredResume(turn([req({})]), T0 + 15 * 60_000, opts)).toEqual({ mayArrive: true })
    expect(answeredResume(turn([req({})]), T0 + tuning.RESUME_DELIVERY_MS, opts)).toEqual({
      mayArrive: false,
    })
  })

  it('an unknown delivery outcome waits the checking window only', () => {
    const r = req({ status: 'rejected', resume_status: 'delivery_outcome_unknown' })
    expect(answeredResume(turn([r]), T0 + tuning.LOST_REPLY_AFTER_MS - 1, opts)).toEqual({
      mayArrive: true,
    })
    expect(answeredResume(turn([r]), T0 + tuning.LOST_REPLY_AFTER_MS, opts)).toEqual({
      mayArrive: false,
    })
  })

  it('only the newest answer counts: an older one already ran (it asked again)', () => {
    const older = req({ id: 'h1', resume_status: 'completed' })
    const newer = req({
      id: 'h2',
      resume_status: 'failed',
      created_at: new Date(T0 + 1000).toISOString(),
      resolved_at: new Date(T0 + 2000).toISOString(),
    })
    expect(answeredResume(turn([older, newer]), T0 + 3000, opts)).toEqual({ mayArrive: false })
    expect(answeredResume(turn([newer, older]), T0 + 3000, opts)).toEqual({ mayArrive: false })
  })

  it('an undelivered answer waits for the delivery retries, far past the checking window', () => {
    const r = req({ resume_status: 'not_started' })
    expect(answeredResume(turn([r]), T0 + tuning.LOST_REPLY_AFTER_MS + 1000, opts)).toEqual({
      mayArrive: true,
    })
    expect(answeredResume(turn([r]), T0 + tuning.RESUME_DELIVERY_MS, opts)).toEqual({
      mayArrive: false,
    })
  })

  it('a failed or skipped resume never waits, and never offers to run again (still non-null)', () => {
    expect(answeredResume(turn([req({ resume_status: 'failed' })]), T0 + 1000, opts)).toEqual({
      mayArrive: false,
    })
    expect(answeredResume(turn([req({ resume_status: 'skipped' })]), T0 + 1000, opts)).toEqual({
      mayArrive: false,
    })
  })

  it('an attempt this tab saw end with nothing does not wait', () => {
    expect(answeredResume(turn([req({})]), T0 + 1000, { ...opts, seenEnded: true })).toEqual({
      mayArrive: false,
    })
  })

  it('a client clock behind the server waits no longer than the window from when this tab saw the answer', () => {
    // resolved_at is 1 h "in the future" for this client, which first saw the answer at T0.
    const skewed = req({ resolved_at: new Date(T0 + 3_600_000).toISOString() })
    const firstSeen = seenAt(T0)
    expect(
      answeredResume(turn([skewed]), T0 + tuning.RESUME_DELIVERY_MS - 1, {
        seenEnded: false,
        firstSeen,
      }),
    ).toEqual({ mayArrive: true })
    expect(
      answeredResume(turn([skewed]), T0 + tuning.RESUME_DELIVERY_MS, {
        seenEnded: false,
        firstSeen,
      }),
    ).toEqual({ mayArrive: false })
  })

  it('an unparseable resolved_at does not wait', () => {
    expect(answeredResume(turn([req({ resolved_at: null })]), T0, opts)).toEqual({
      mayArrive: false,
    })
  })
})

describe('replyStatus after a failed resume', () => {
  it('skips the checking wait when the server gave up resuming', () => {
    expect(
      replyStatus(
        [turn([req({ resume_status: 'failed' })])],
        T0 + 1000,
        tuning.LOST_REPLY_AFTER_MS,
      ),
    ).toBe('unconfirmed')
    expect(
      replyStatus(
        [turn([req({ resume_status: 'skipped' })])],
        T0 + 1000,
        tuning.LOST_REPLY_AFTER_MS,
      ),
    ).toBe('unconfirmed')
    expect(replyStatus([turn([req({})])], T0 + 1000, tuning.LOST_REPLY_AFTER_MS)).toBe('checking')
  })
})

describe('sawTerminal (a finished attempt holds nothing back)', () => {
  it("a resume's terminal counts only after the orchestrator's turn began (the sub-agent's replayed COMPLETED comes first)", () => {
    expect(sawTerminal({ taskState: 'completed', replyArtifactsFrom: null }, true)).toBe(false)
    expect(sawTerminal({ taskState: 'completed', replyArtifactsFrom: 1 }, true)).toBe(true)
    expect(sawTerminal({ taskState: 'failed', replyArtifactsFrom: 0 }, true)).toBe(true)
    expect(sawTerminal({ taskState: 'working', replyArtifactsFrom: 1 }, true)).toBe(false)
  })

  it('a first send ends on its own terminal', () => {
    expect(sawTerminal({ taskState: 'completed', replyArtifactsFrom: null }, false)).toBe(true)
    expect(sawTerminal({ taskState: 'input_required', replyArtifactsFrom: null }, false)).toBe(
      false,
    )
  })
})
