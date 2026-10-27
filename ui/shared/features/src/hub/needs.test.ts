/**
 * Needs you for one agent and the hub's session sample (plans/feat-agent-centric.md PR1c; CEO R2-5, R2-6; DX one
 * verdict string; eng "Hub sessions"; /review of PR1c). Pure.
 */
import { describe, expect, it } from 'vitest'
import { hubNeeds, sessionSource, type HubNeedsInput } from './needs'
import { agentRows, sampleStart } from './sessions'

const ok = <T>(value: T) => ({ state: 'ok' as const, value })
const sessionsOk = (over: Partial<{ found: number; checked: number; failed: number }> = {}) =>
  ok({ found: 3, checked: 3, failed: 0, scanned: 25, ambiguous: false, ...over })
const base: HubNeedsInput = {
  requests: ok({ count: 0, firstChat: null, firstRequest: null, unproven: 0 }),
  status: ok({ rating: 'healthy', reason: null, incomplete: false }),
  sessions: sessionsOk(),
  budget: { state: 'absent' },
}

describe('hub Needs you', () => {
  it('says "No issues found in checked data" only when every source answered with nothing', () => {
    const n = hubNeeds(base)
    expect(n.clear).toBe(true)
    expect(n.coverage).toEqual({
      requests: true,
      status: true,
      sessions: { found: 3, checked: 3, failed: 0, scanned: 25, ambiguous: false },
      budget: false,
    })
    expect(hubNeeds({ ...base, sessions: { state: 'loading' } }).clear).toBe(false)
    expect(hubNeeds({ ...base, sessions: { state: 'loading' } }).coverage).toBeNull()
  })

  it.each(['requests', 'status', 'sessions'] as const)(
    'a failed %s source is its own line, never the verdict',
    (source) => {
      const n = hubNeeds({ ...base, [source]: { state: 'failed' } })
      expect(n.failed).toEqual([source])
      expect(n.clear).toBe(false)
    },
  )

  it('a failed source and a found row both show', () => {
    const n = hubNeeds({
      ...base,
      requests: { state: 'failed' },
      status: ok({ rating: 'action', reason: 'Crashed', incomplete: false }),
    })
    expect(n.rows.map((r) => r.kind)).toEqual(['status'])
    expect(n.failed).toEqual(['requests'])
    expect(n.clear).toBe(false)
  })

  it('an incomplete rating keeps its row and says the rating isn’t complete', () => {
    const n = hubNeeds({
      ...base,
      status: ok({ rating: 'action', reason: 'Crashed', incomplete: true }),
    })
    expect(n.rows.map((r) => r.kind)).toEqual(['status'])
    expect(n.failed).toEqual(['status'])
    expect(n.statusIncomplete).toBe(true)
  })

  it('an unrated agent (stopped, not deployed) is absent, never "checked" (review: adversarial)', () => {
    const n = hubNeeds({
      ...base,
      status: ok({ rating: 'unknown', reason: null, incomplete: false }),
    })
    expect(n.coverage?.status).toBe(false)
    expect(n.rows).toEqual([])
  })

  it('requests nobody can prove yet keep the verdict back (superusers)', () => {
    const n = hubNeeds({
      ...base,
      requests: ok({ count: 0, firstChat: null, firstRequest: null, unproven: 2 }),
    })
    expect(n.unproven).toBe(2)
    expect(n.clear).toBe(false)
  })

  it('orders action, then requests, then watch, then failed sessions; the sessions row carries its counts', () => {
    const run = (rating: 'action' | 'watch') =>
      hubNeeds({
        requests: ok({ count: 2, firstChat: 'c1', firstRequest: 'r1', unproven: 0 }),
        status: ok({ rating, reason: 'Crashed', incomplete: false }),
        sessions: ok({ found: 5, checked: 4, failed: 1, scanned: 25, ambiguous: true }),
        budget: { state: 'absent' },
      }).rows
    expect(run('action').map((r) => r.kind)).toEqual(['status', 'requests', 'sessions'])
    expect(run('watch').map((r) => r.kind)).toEqual(['requests', 'status', 'sessions'])
    expect(run('action').at(-1)).toEqual({
      kind: 'sessions',
      failed: 1,
      checked: 4,
      ambiguous: true,
    })
  })

  it('a harness (no rating) can still be clear on what was checked', () => {
    const n = hubNeeds({ ...base, status: { state: 'absent' } })
    expect(n.clear).toBe(true)
    expect(n.coverage?.status).toBe(false)
  })
})

describe('the sessions source', () => {
  const src = (verdicts: (string | undefined)[], over = {}) =>
    sessionSource({
      listError: false,
      listPending: false,
      found: verdicts.length,
      scanned: 25,
      verdicts: verdicts as never,
      ambiguous: false,
      ...over,
    })

  it('needs at least half of the checked sessions to answer before "no failures" counts', () => {
    expect(src(['ok', 'ok', 'unknown', 'unknown']).state).toBe('ok')
    expect(src(['ok', 'unknown', 'unknown', 'unknown']).state).toBe('failed')
  })

  it('a failure found always shows, however few answered', () => {
    const s = src(['failed', 'unknown', 'unknown', 'unknown'])
    expect(s).toMatchObject({ state: 'ok', value: { failed: 1, checked: 1 } })
  })

  it('checking is loading; a failed list is failed; an empty sample is answered', () => {
    expect(src(['ok', 'checking']).state).toBe('loading')
    expect(src([], { listError: true }).state).toBe('failed')
    expect(src([], { listPending: true }).state).toBe('loading')
    expect(src([])).toMatchObject({ state: 'ok', value: { found: 0, checked: 0 } })
  })
})

describe('the hub session sample', () => {
  const now = new Date('2026-03-20T15:00:30Z')
  it('starts at the later of creation and Sessions’ 7-day bound, floored to the minute', () => {
    expect(sampleStart('2026-03-18T00:00:00Z', now).toISOString()).toBe('2026-03-18T00:00:00.000Z')
    const old = sampleStart('2025-01-01T00:00:00Z', now)
    expect(old.getUTCSeconds()).toBe(0)
    expect(now.getTime() - old.getTime()).toBeLessThanOrEqual(7 * 86_400_000 + 30_000)
    expect(now.getTime() - old.getTime()).toBeGreaterThan(6.9 * 86_400_000)
  })

  it('keeps this agent’s rows (raw name) created after the start', () => {
    const start = new Date('2026-03-18T00:00:00Z')
    const row = (agent_id: string, start_time: string) =>
      ({ session_id: `${agent_id}-${start_time}`, agent_id, start_time }) as never
    const rows = agentRows(
      [
        row('a', '2026-03-19T00:00:00Z'),
        row('b', '2026-03-19T00:00:00Z'),
        row('a', '2026-03-17T00:00:00Z'),
      ],
      'a',
      start,
    )
    expect(rows).toHaveLength(1)
  })

  it('this agent’s budget (B5): stopped or over is action after health, a projected one watch; nothing when on track', () => {
    const run = (level: 'action' | 'watch' | null, rating: 'action' | 'watch' = 'action') =>
      hubNeeds({
        ...base,
        status: ok({ rating, reason: 'Crashed', incomplete: false }),
        requests: ok({ count: 1, firstChat: 'c1', firstRequest: 'r1', unproven: 0 }),
        budget: ok({ level, sentence: 'Ran out Oct 12; calls are stopped until Nov 1.' }),
      })
    expect(run('action').rows.map((r) => r.kind)).toEqual(['status', 'budget', 'requests'])
    expect(run('watch', 'watch').rows.map((r) => r.kind)).toEqual(['requests', 'status', 'budget'])
    expect(run(null).rows.some((r) => r.kind === 'budget')).toBe(false)
    expect(run(null).coverage?.budget).toBe(true)
    const failed = hubNeeds({ ...base, budget: { state: 'failed' } })
    expect(failed.failed).toEqual(['budget'])
    expect(failed.clear).toBe(false)
  })
})
