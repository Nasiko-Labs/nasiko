// @vitest-environment node
/** Overview pure logic (plans/feat-overview.md §5, §6, §10; eng review R3, R4, R8). */
import { describe, expect, it } from 'vitest'
import { overviewNarrative } from '@/features/narrative/overview'
import {
  rateAgent,
  spikeDrivers,
  summarizeFleet,
  uniqueByName,
  type AgentHealthInput,
} from './health'
import { mergeNeeds } from './needs'
import { previousWindow, rangeWindow } from './api'
import { overviewSearchSchema } from './search'
import { stackSpend } from './stack'

const NOW = Date.parse('2026-09-29T12:00:00Z')
const H = 3_600_000
const D = 24 * H
const iso = (ms: number) => new Date(ms).toISOString()

const base = (over: Partial<AgentHealthInput> = {}): AgentHealthInput => ({
  id: 'a1',
  name: 'Code Reviewer',
  display: 'running',
  raw: 'running',
  createdAt: iso(NOW - 60 * D),
  updatedAt: iso(NOW - 10 * D),
  current: { cost: 10, operations: 100, p95: 1000 },
  previous: { cost: 10, operations: 100, p95: 1000 },
  costPerOp30d: 0.1,
  ...over,
})
const rate = (over: Partial<AgentHealthInput> = {}) => rateAgent(base(over), NOW)

describe('rateAgent', () => {
  it('is Healthy when every known dimension is fine', () => {
    expect(rate()).toEqual({ id: 'a1', name: 'Code Reviewer', rating: 'healthy', reasons: [] })
  })

  it('never rates stopped, not-deployed, unknown or harness rows: Unknown, not Healthy', () => {
    for (const display of ['stopped', 'not-deployed', 'unknown', 'harness'] as const)
      expect(rate({ display }).rating).toBe('unknown')
  })

  it('is Unknown, not Healthy, when the cost data is unavailable and nothing else fired', () => {
    expect(rate({ current: null, previous: null }).rating).toBe('unknown')
  })

  describe('reliability', () => {
    it('needs action for crashed or failed status', () => {
      expect(rate({ display: 'attention', raw: 'crashed' })).toMatchObject({
        rating: 'action',
        reasons: [{ dimension: 'reliability', level: 'action', text: 'crashed' }],
      })
      expect(rate({ display: 'attention', raw: 'failed' }).reasons[0]!.text).toBe(
        'deployment failed',
      )
    })

    it('needs action when deploying longer than the watch cap, not before', () => {
      expect(
        rate({ display: 'deploying', raw: 'deploying', updatedAt: iso(NOW - 10 * 60_000) }).rating,
      ).toBe('action')
      expect(
        rate({ display: 'deploying', raw: 'deploying', updatedAt: iso(NOW - 60_000) }).rating,
      ).toBe('healthy')
    })
  })

  describe('cost', () => {
    it('watches a 7-day rise over the threshold, only with a minimum base', () => {
      expect(rate({ current: { cost: 16, operations: 100, p95: 1000 } }).reasons).toEqual([
        { dimension: 'cost', level: 'watch', text: 'cost ↑ 60% vs last week' },
      ])
      expect(rate({ current: { cost: 14, operations: 100, p95: 1000 } }).rating).toBe('healthy')
      // Below MIN_BASE_OPS in the previous week: "▲ 400%" on noise never rates.
      expect(
        rate({
          current: { cost: 50, operations: 10, p95: null },
          previous: { cost: 1, operations: 5, p95: null },
        }).rating,
      ).toBe('healthy')
    })

    it('watches cost per turn above 2× its 30-day average', () => {
      expect(
        rate({
          current: { cost: 25, operations: 100, p95: 1000 },
          previous: { cost: 25, operations: 100, p95: 1000 },
        }).reasons[0]!.text,
      ).toBe('cost per turn 2.5× its 30-day average')
    })

    it('needs action for a stopped budget, watches an exceeded one', () => {
      expect(rate({ budget: { stopped: true, exceeded: true } })).toMatchObject({
        rating: 'action',
        reasons: [{ text: 'budget stopped: calls are refused' }],
      })
      expect(rate({ budget: { stopped: false, exceeded: true } }).rating).toBe('watch')
    })

    it('needs action for the driver of a 3× spike', () => {
      expect(rate({ spike: { date: '2026-09-27', factor: 3.5 } }).reasons[0]).toEqual({
        dimension: 'cost',
        level: 'action',
        text: 'drove a 3.5× spend spike on Sep 27',
      })
    })
  })

  describe('activity (never Needs action)', () => {
    it('watches a running agent idle 7 days, unless it is newer than a week', () => {
      expect(rate({ current: { cost: 0, operations: 0, p95: null } }).reasons).toContainEqual({
        dimension: 'activity',
        level: 'watch',
        text: 'idle 7 days while deployed',
      })
      expect(
        rate({ current: { cost: 0, operations: 0, p95: null }, createdAt: iso(NOW - 2 * D) })
          .reasons,
      ).toEqual([])
    })

    it('watches a drop over the threshold with a minimum base', () => {
      expect(rate({ current: { cost: 2, operations: 20, p95: 1000 } }).reasons).toContainEqual({
        dimension: 'activity',
        level: 'watch',
        text: 'activity ↓ 80% vs last week',
      })
      expect(rate({ current: { cost: 5, operations: 50, p95: 1000 } }).rating).toBe('healthy')
    })
  })

  it('watches p95 latency up more than 50%, with a base', () => {
    expect(rate({ current: { cost: 10, operations: 100, p95: 1600 } }).reasons).toEqual([
      { dimension: 'latency', level: 'watch', text: 'p95 latency ↑ 60% vs last week' },
    ])
    expect(
      rate({
        current: { cost: 10, operations: 100, p95: 1600 },
        previous: { cost: 10, operations: 100, p95: null },
      }).rating,
    ).toBe('healthy')
  })

  it('takes the worst dimension and lists Needs action reasons first', () => {
    const r = rate({
      current: { cost: 16, operations: 100, p95: 1000 },
      budget: { stopped: true, exceeded: true },
    })
    expect(r.rating).toBe('action')
    expect(r.reasons.map((x) => x.level)).toEqual(['action', 'watch'])
  })

  it('tags a stopped budget with a stable code, so callers never compare reason text', () => {
    expect(rate({ budget: { stopped: true, exceeded: true } }).reasons[0]).toMatchObject({
      code: 'budgetStopped',
    })
  })
})

describe('summarizeFleet', () => {
  it('counts ratings and orders Watch by reasons, then name', () => {
    const a = rateAgent(
      base({ id: 'b', name: 'Bravo', current: { cost: 16, operations: 100, p95: 1600 } }),
      NOW,
    )
    const b = rateAgent(
      base({ id: 'a', name: 'Alpha', current: { cost: 16, operations: 100, p95: 1000 } }),
      NOW,
    )
    const c = rateAgent(
      base({ id: 'c', name: 'Charlie', display: 'attention', raw: 'crashed' }),
      NOW,
    )
    const d = rateAgent(base({ id: 'd', name: 'Delta', display: 'stopped' }), NOW)
    const s = summarizeFleet([b, a, c, d, rateAgent(base({ id: 'e' }), NOW)])
    expect(s.counts).toEqual({ healthy: 1, watch: 2, action: 1, unknown: 1 })
    expect(s.watch.map((x) => x.name)).toEqual(['Bravo', 'Alpha'])
    expect(s.action.map((x) => x.id)).toEqual(['c'])
  })
})

describe('spikeDrivers (R4)', () => {
  const days = (spikes: Record<string, [number, string | null]>) =>
    Array.from({ length: 30 }, (_, k) => {
      const date = iso(NOW - (29 - k) * D).slice(0, 10)
      const [spend, topAgent] = spikes[date] ?? [10, 'quiet']
      return { date, spend, topAgent }
    })
  const index = new Map([
    ['code-reviewer', [{ id: 'cr' }]],
    ['twin', [{ id: 't1' }, { id: 't2' }]],
  ])
  const resolve = uniqueByName(index)

  it('rates the agent whose raw name is unique', () => {
    expect([...spikeDrivers(days({ '2026-09-27': [35, 'code-reviewer'] }), resolve, NOW)]).toEqual([
      ['cr', { date: '2026-09-27', factor: 3.5 }],
    ])
  })

  it('rates nobody for a shared name, an unknown name or a null name', () => {
    expect(spikeDrivers(days({ '2026-09-27': [35, 'twin'] }), resolve, NOW).size).toBe(0)
    expect(spikeDrivers(days({ '2026-09-27': [35, 'someone-else'] }), resolve, NOW).size).toBe(0)
    expect(spikeDrivers(days({ '2026-09-27': [35, null] }), resolve, NOW).size).toBe(0)
  })

  it('ignores spikes older than the last week and days under 3×', () => {
    expect(spikeDrivers(days({ '2026-09-10': [50, 'code-reviewer'] }), resolve, NOW).size).toBe(0)
    expect(spikeDrivers(days({ '2026-09-27': [25, 'code-reviewer'] }), resolve, NOW).size).toBe(0)
  })

  it('needs a typical day to compare with', () => {
    expect(
      spikeDrivers(
        days({}).map((d) => ({ ...d, spend: 0 })),
        resolve,
        NOW,
      ).size,
    ).toBe(0)
  })
})

describe('mergeNeeds (design 5A, eng R5)', () => {
  const ok = <T>(value: T) => ({ state: 'ok' as const, value })
  const none = {
    requests: ok({ chats: [], outside: 0 }),
    agents: ok([]),
    budgets: ok([]),
    sessions: ok({ failed: 0, checked: 25, agents: [] }),
  }
  const chat = (sessionId: string, at: string) => ({
    sessionId,
    firstId: `r-${sessionId}`,
    kind: 'tool_approval' as const,
    count: 1,
    chatTitle: sessionId,
    at,
  })

  it('is empty only when every available source answered with nothing', () => {
    expect(mergeNeeds(none)).toMatchObject({
      empty: true,
      loading: false,
      failed: [],
      waitingCount: 0,
      actionCount: 0,
    })
    expect(mergeNeeds({ ...none, budgets: { state: 'absent' } }).empty).toBe(true)
    expect(mergeNeeds({ ...none, requests: { state: 'failed' } })).toMatchObject({
      empty: false,
      failed: ['requests'],
      waitingCount: null,
    })
    expect(mergeNeeds({ ...none, sessions: { state: 'loading' } })).toMatchObject({
      empty: false,
      loading: true,
    })
  })

  it('ranks requests (oldest first), outside Chat, agents, budgets, then failing sessions', () => {
    const n = mergeNeeds({
      requests: ok({
        chats: [chat('b', '2026-09-29T10:00:00Z'), chat('a', '2026-09-29T09:00:00Z')],
        outside: 2,
      }),
      agents: ok([{ id: 'x', name: 'X', reason: 'crashed', budget: false }]),
      budgets: ok([{ id: 'bud', label: 'Your monthly', crossed: 80, state: 'warning' as const }]),
      sessions: ok({ failed: 3, checked: 25, agents: ['X'] }),
    })
    expect(n.rows.map((r) => r.key)).toEqual([
      'request-a',
      'request-b',
      'outside',
      'agent-x',
      'budget-bud',
      'sessions',
    ])
    expect(n).toMatchObject({ waitingCount: 4, actionCount: 1, empty: false })
  })

  it('keeps rows from sources that answered next to a failed one', () => {
    const n = mergeNeeds({
      ...none,
      agents: ok([{ id: 'x', name: 'X', reason: 'crashed', budget: false }]),
      requests: { state: 'failed' },
    })
    expect(n.rows).toHaveLength(1)
    expect(n.failed).toEqual(['requests'])
  })
})

describe('overviewNarrative (§10, eng R2)', () => {
  const month = { mtd: 4186, low: 12900, high: 13600, show: true, vsLastMonthPct: 31 }

  it('says fleet spend, never "You\'ve spent"', () => {
    const n = overviewNarrative({
      month,
      actionCount: 2,
      waitingCount: 3,
      otherCount: 0,
      empty: false,
    })
    expect(n.money).toBe(
      'The fleet has spent $4,186.00 this month, on pace for $12,900.00–$13,600.00 (up 31% vs the same days last month).',
    )
    expect(n.attention).toBe('2 agents need action and 3 requests are waiting for you.')
  })

  it('leaves out a clause whose data is missing, and never says "nothing" unless every source answered', () => {
    expect(
      overviewNarrative({
        month: null,
        actionCount: null,
        waitingCount: null,
        otherCount: 0,
        empty: false,
      }),
    ).toEqual({ money: null, attention: null })
    expect(
      overviewNarrative({ month, actionCount: 0, waitingCount: 0, otherCount: 0, empty: false })
        .attention,
    ).toBeNull()
    expect(
      overviewNarrative({ month, actionCount: 0, waitingCount: 0, otherCount: 0, empty: true })
        .attention,
    ).toBe('Nothing needs you right now.')
    expect(
      overviewNarrative({
        month: { ...month, mtd: 0 },
        actionCount: 1,
        waitingCount: 0,
        otherCount: 2,
        empty: false,
      }),
    ).toEqual({
      money: 'The fleet has no spend yet this month.',
      attention: '1 agent needs action and 2 other items need a look.',
    })
  })
})

describe('stackSpend', () => {
  const pt = (date: string, spend: number) => ({
    iso: `${date}T00:00:00.000Z`,
    label: date,
    spend,
    operations: 0,
    p95: null,
    topAgent: null,
    topAgentSpend: null,
  })

  it('splits each day into the drivers and Other, which is never negative', () => {
    const { series, rows } = stackSpend(
      [pt('2026-09-28', 10), pt('2026-09-29', 5), pt('2026-09-30', 0)],
      [
        { id: 'a', name: 'A', days: [pt('2026-09-28', 6), pt('2026-09-29', 4)] },
        // Rounds a cent above the day on the 29th: capped at what is left.
        { id: 'b', name: 'B', days: [pt('2026-09-28', 1), pt('2026-09-29', 1.01)] },
      ],
    )
    expect(series.map((s) => s.key)).toEqual(['s0', 's1'])
    expect(rows.map((r) => [r.s0, r.s1, r.other])).toEqual([
      [6, 1, 3],
      [4, 1, 0],
      [0, 0, 0],
    ])
  })
})

describe('the page range', () => {
  const now = new Date('2026-10-01T15:30:00Z')

  it('uses the server range for 7d and 30d, and explicit UTC days for 90d', () => {
    expect(rangeWindow('30d', now).params).toEqual({ range: '30d' })
    const w = rangeWindow('90d', now)
    expect(w.params.start_time).toBe('2026-07-04T00:00:00.000Z')
    expect(w.params.end_time).toBe('2026-10-01T15:30:00.000Z')
    // The window before it is as long, ends where it starts, and has a key of its own.
    const p = previousWindow(w)
    expect(p.end.getTime()).toBe(w.start.getTime())
    expect(w.end.getTime() - w.start.getTime()).toBe(p.end.getTime() - p.start.getTime())
    expect(p.key).not.toBe(w.key)
  })

  it('drops a junk range rather than failing the page', () => {
    expect(overviewSearchSchema.parse({ range: '1y' })).toEqual({ range: undefined })
    expect(overviewSearchSchema.parse({ range: '90d' })).toEqual({ range: '90d' })
  })
})
