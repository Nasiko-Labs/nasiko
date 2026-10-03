/**
 * The harness narrative's cost clause. Regression: ISSUE-002 (/qa 2026-09-26,
 * .gstack/qa-reports/qa-report-localhost-2026-09-26.md): the summary said "estimated cost $0.00" when the only
 * active harness was unpriced.
 */
import { describe, expect, it } from 'vitest'
import type { HarnessTotals } from '@/features/harnesses/types'
import { harnessNarrative } from './harness'

const T = (p: Partial<HarnessTotals> = {}): HarnessTotals => ({
  scope_devs: 1,
  active_devs: 0,
  registered_devs: 1,
  idle_seats: 0,
  sessions: 0,
  turns: 0,
  tokens: 0,
  cost_usd: 0,
  unpriced_calls: 0,
  delta_pct: null,
  ...p,
})
const h = (harness: string, p: Partial<HarnessTotals>) => ({ ...T(p), harness, top_models: [] })

describe('harness narrative cost clause', () => {
  it('drops the cost clause when every harness that ran is mostly unpriced (an idle one does not count)', () => {
    const by_harness = [
      h('claude', { idle_seats: 1 }),
      h('cursor', { active_devs: 1, turns: 10, unpriced_calls: 7, cost_usd: 0.4 }),
    ]
    const out = harnessNarrative({
      windowLabel: 'Last 30 days',
      res: { totals: T({ active_devs: 1, idle_seats: 1, turns: 10 }), by_harness },
      individual: { self: false, name: 'Aiko' },
    })
    expect(out.join(' ')).not.toMatch(/estimated cost/i)
    expect(out.join(' ')).toMatch(/1 registered harness had no activity/)
  })

  it('keeps the cost of priced harnesses that ran, ignoring idle ones', () => {
    const by_harness = [
      h('claude', { active_devs: 1, turns: 5, cost_usd: 12.5 }),
      h('codex', { idle_seats: 1 }),
    ]
    const out = harnessNarrative({
      windowLabel: 'Last 30 days',
      res: { totals: T({ active_devs: 1, turns: 5 }), by_harness },
      individual: { self: true, name: 'You' },
    })
    expect(out.join(' ')).toMatch(/estimated cost \$12\.50 \(API list price\)/i)
  })

  it('an org with no activity at all says nothing about cost', () => {
    const out = harnessNarrative({
      windowLabel: 'Last 30 days',
      res: {
        totals: T({ scope_devs: 10, registered_devs: 4, idle_seats: 4 }),
        by_harness: [h('claude', { registered_devs: 4, idle_seats: 4 })],
      },
    })
    expect(out.join(' ')).not.toMatch(/estimated cost/i)
  })
})

describe('harness narrative cost clause, mixed priced and mostly-unpriced (ship review)', () => {
  it('sums every known charge and says "at least" when a harness that ran is mostly unpriced', () => {
    const by_harness = [
      h('claude', { active_devs: 1, turns: 5, cost_usd: 10 }),
      h('cursor', { active_devs: 1, turns: 10, unpriced_calls: 8, cost_usd: 40 }),
    ]
    const out = harnessNarrative({
      windowLabel: 'Last 30 days',
      res: { totals: T({ active_devs: 1, turns: 15 }), by_harness },
      individual: { self: true, name: 'You' },
    })
    expect(out.join(' ')).toMatch(
      /estimated cost at least \$50\.00 \(API list price; some turns unpriced\)/i,
    )
  })

  it('group level: the idle-seats clause and the cost clause share the second sentence', () => {
    const by_harness = [
      h('claude', { active_devs: 4, registered_devs: 5, idle_seats: 1, turns: 40, cost_usd: 20 }),
    ]
    const out = harnessNarrative({
      windowLabel: 'Last 30 days',
      res: {
        totals: T({ scope_devs: 6, active_devs: 4, registered_devs: 5, idle_seats: 1, turns: 40 }),
        by_harness,
      },
    })
    expect(out[1]).toBe(
      '1 registered seat had no activity; estimated cost $20.00 (API list price).',
    )
  })
})
