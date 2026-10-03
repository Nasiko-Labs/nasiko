// @vitest-environment node
/** Overview pure logic: the edges logic.test.ts leaves out (ship coverage audit, plans/feat-overview.md §5, §6, §10). */
import { describe, expect, it } from 'vitest'
import { overviewNarrative } from '@/features/narrative/overview'
import { rateAgent, spikeDrivers, type AgentHealthInput } from './health'
import { mergeNeeds } from './needs'

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

describe('rateAgent edges', () => {
  it('skips comparisons it cannot make honestly', () => {
    // No previous spend: a rise from $0 is never a percentage.
    expect(
      rate({ previous: { cost: 0, operations: 100, p95: 1000 }, costPerOp30d: null }).rating,
    ).toBe('healthy')
    // Cost per turn needs MIN_BASE_OPS turns this week.
    expect(rate({ current: { cost: 5, operations: 10, p95: 1000 }, previous: null }).rating).toBe(
      'healthy',
    )
    // A spike under SPIKE_ACTION_FACTOR is not a reason.
    expect(rate({ spike: { date: '2026-09-27', factor: 2.9 } }).rating).toBe('healthy')
    // Running for 7 days with a null current window stays Unknown (not idle).
    expect(rate({ current: null }).rating).toBe('unknown')
  })
})

describe('spikeDrivers edges', () => {
  const resolve = (n: string) => (n === 'cr' ? 'id-cr' : undefined)

  it('needs at least 3 days', () => {
    expect(
      spikeDrivers(
        [
          { date: '2026-09-28', spend: 1, topAgent: 'cr' },
          { date: '2026-09-29', spend: 100, topAgent: 'cr' },
        ],
        resolve,
        NOW,
      ).size,
    ).toBe(0)
  })

  it('keeps the highest factor when one agent drove two spike days', () => {
    const days = Array.from({ length: 10 }, (_, k) => ({
      date: iso(NOW - (9 - k) * D).slice(0, 10),
      spend: 10,
      topAgent: 'quiet' as string | null,
    }))
    days[7] = { ...days[7]!, spend: 40, topAgent: 'cr' }
    days[8] = { ...days[8]!, spend: 60, topAgent: 'cr' }
    days[9] = { ...days[9]!, spend: 35, topAgent: 'cr' }
    expect(spikeDrivers(days, resolve, NOW).get('id-cr')).toEqual({
      date: days[8]!.date,
      factor: 6,
    })
  })
})

describe('overviewNarrative edges', () => {
  const month = { mtd: 500, low: 1000, high: 1200, show: false, vsLastMonthPct: -12.4 }

  it('says "down" for a lower month and leaves out the pace until the forecast shows', () => {
    expect(
      overviewNarrative({
        month,
        actionCount: null,
        waitingCount: null,
        otherCount: 0,
        empty: false,
      }).money,
    ).toBe('The fleet has spent $500.00 this month (down 12% vs the same days last month).')
    expect(
      overviewNarrative({
        month: { ...month, vsLastMonthPct: null },
        actionCount: null,
        waitingCount: null,
        otherCount: 0,
        empty: false,
      }).money,
    ).toBe('The fleet has spent $500.00 this month.')
  })

  it('joins three clauses with commas and "and", and capitalizes a lone waiting clause', () => {
    expect(
      overviewNarrative({
        month: null,
        actionCount: 1,
        waitingCount: 2,
        otherCount: 1,
        empty: false,
      }).attention,
    ).toBe('1 agent needs action, 2 requests are waiting for you and 1 other item needs a look.')
    expect(
      overviewNarrative({
        month: null,
        actionCount: 0,
        waitingCount: 1,
        otherCount: 0,
        empty: false,
      }).attention,
    ).toBe('1 request is waiting for you.')
  })
})

describe('mergeNeeds edges', () => {
  it('adds no row for zero outside requests or zero failed sessions, and counts absent sources as neither loading nor failed', () => {
    const n = mergeNeeds({
      requests: { state: 'ok', value: { chats: [], outside: 0 } },
      agents: { state: 'loading' },
      budgets: { state: 'absent' },
      sessions: { state: 'ok', value: { failed: 0, checked: 25, agents: [] } },
    })
    expect(n).toMatchObject({
      rows: [],
      failed: [],
      loading: true,
      empty: false,
      waitingCount: 0,
      actionCount: null,
    })
  })
})
