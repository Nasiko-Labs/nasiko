/**
 * Edge cases the coverage audit found untested: request shaping, error routing, URL
 * parsing fallbacks, window labels in the summary, and the live adapter's Δ at zero.
 */
import { describe, expect, it } from 'vitest'
import { harnessNarrative } from '@/features/narrative/harness'
import { ApiError } from '@/lib/api/client'
import { isEndpointAbsent, isNotVisible, usageErrorCode, usagePath } from './api'
import { USER_PAGE_LIMIT } from './constants'
import { buildLiveIndividual, harnessCandidates } from './liveIndividual'
import { metricValue, sortRows } from './rollup'
import { harnessesSearchSchema } from './search'
import type { HarnessTotals, UsageRow } from './types'

const T = (p: Partial<HarnessTotals> = {}): HarnessTotals => ({
  scope_devs: 10,
  active_devs: 5,
  registered_devs: 8,
  idle_seats: 3,
  sessions: 20,
  turns: 100,
  tokens: 1e6,
  cost_usd: 50,
  unpriced_calls: 0,
  delta_pct: null,
  ...p,
})

describe('usagePath', () => {
  it('sends compare=1 and a page limit only for developer lists', () => {
    const team = new URL(
      usagePath({ scope: 'unit', unit_id: 'u1', group_by: 'user', range: '7d', compare: true }),
      'http://x',
    )
    expect(team.searchParams.get('compare')).toBe('1')
    expect(team.searchParams.get('limit')).toBe(String(USER_PAGE_LIMIT))
    const units = new URL(
      usagePath({ scope: 'org', group_by: 'unit', range: '30d', compare: false }),
      'http://x',
    )
    expect(units.searchParams.has('compare')).toBe(false)
    expect(units.searchParams.has('limit')).toBe(false)
  })
})

describe('usage error routing', () => {
  it('reads a code only from an ApiError with an object body', () => {
    expect(usageErrorCode(new Error('x'))).toBeNull()
    expect(usageErrorCode(new ApiError(404, 'Not Found', '/x', ''))).toBeNull()
    expect(usageErrorCode(new ApiError(400, { error: 'e', code: 7 }, '/x', ''))).toBeNull()
    expect(
      usageErrorCode(new ApiError(400, { error: 'e', code: 'invalid_cursor' }, '/x', '')),
    ).toBe('invalid_cursor')
  })

  it('every "not visible" code is a coded 404; a coded 500 or 400 is neither absent nor not visible', () => {
    for (const code of ['unit_not_visible', 'user_not_visible', 'not_found']) {
      expect(isNotVisible(new ApiError(404, { error: 'e', code }, '/x', ''))).toBe(true)
    }
    const internal = new ApiError(500, { error: 'e', code: 'internal' }, '/x', '')
    expect(isNotVisible(internal)).toBe(false)
    expect(isEndpointAbsent(internal)).toBe(false)
    expect(isNotVisible(new ApiError(404, { error: 'e', code: 'invalid_scope' }, '/x', ''))).toBe(
      false,
    )
    // A bare 500 is an error, not an absent endpoint.
    expect(isEndpointAbsent(new ApiError(500, 'boom', '/x', ''))).toBe(false)
  })
})

describe('URL state', () => {
  it('junk values fall back instead of throwing', () => {
    const s = harnessesSearchSchema.parse({
      preset: 'forever',
      mock: 'nope',
      from: '2026-02-31',
      compare: 'maybe',
    })
    expect(s).toMatchObject({
      preset: '30d',
      mock: undefined,
      from: undefined,
      compare: undefined,
    })
  })

  it("keeps a layer's own keys (the schema is loose), for that layer's schema to parse", () => {
    expect(harnessesSearchSchema.parse({ scope: 'unit', unit_id: 'u1' })).toMatchObject({
      scope: 'unit',
      unit_id: 'u1',
    })
  })

  it.each([
    [1, true],
    [0, false],
    ['1', true],
    ['false', false],
    [true, true],
  ])('compare=%s parses as %s', (raw, want) => {
    expect(harnessesSearchSchema.parse({ compare: raw }).compare).toBe(want)
  })
})

describe('summary window wording', () => {
  const res = {
    totals: T({ idle_seats: 0 }),
    by_harness: [{ ...T(), harness: 'claude', top_models: [] }],
  }
  it.each([
    ['Last 7 days', /^In the last 7 days, /],
    ['This month', /^This month, /],
    ['March 2026', /^In March 2026, /],
  ])('%s', (label, re) => {
    expect(harnessNarrative({ windowLabel: label, res })[0]).toMatch(re)
  })

  it('nobody in scope, and an Individual with nothing connected (self and other)', () => {
    expect(
      harnessNarrative({
        windowLabel: 'Last 7 days',
        res: { totals: T({ scope_devs: 0 }), by_harness: [] },
      }),
    ).toEqual(['No developers in this scope.'])
    expect(
      harnessNarrative({
        windowLabel: 'Last 7 days',
        res: { totals: T(), by_harness: [] },
        individual: { self: true, name: 'Ada' },
      }),
    ).toEqual(['You have no harness connected to OpenRuntime yet.'])
    expect(
      harnessNarrative({
        windowLabel: 'Last 7 days',
        res: { totals: T(), by_harness: [] },
        individual: { self: false, name: 'Ada' },
      }),
    ).toEqual(['Ada has no harness connected to OpenRuntime yet.'])
  })

  it('another developer with one connected harness reads "their connected harness"', () => {
    const [first] = harnessNarrative({
      windowLabel: 'Last 7 days',
      res: {
        totals: T({ idle_seats: 0 }),
        by_harness: [
          { ...T({ active_devs: 1, registered_devs: 1 }), harness: 'claude', top_models: [] },
        ],
      },
      individual: { self: false, name: 'Ada' },
    })
    expect(first).toBe('In the last 7 days, Ada used their connected harness.')
  })
})

describe('rollup edges', () => {
  it('a missing cell counts as 0 for every metric', () => {
    expect(metricValue(undefined, 'cost')).toBe(0)
    expect(metricValue(T(), 'sessions')).toBe(20)
    expect(metricValue(T(), 'cost')).toBe(50)
  })

  it('sorts by a harness column, ties broken by label', () => {
    const r = (label: string, claude?: number): UsageRow => ({
      key: label,
      label,
      kind: 'unit',
      totals: T(),
      harness_breakdown: claude === undefined ? {} : { claude: T({ active_devs: claude }) },
    })
    const rows = [r('B', 2), r('A', 2), r('C'), r('D', 9)]
    expect(
      sortRows(rows, { metric: 'active', column: 'claude', dir: 'desc' }).map((x) => x.label),
    ).toEqual(['D', 'A', 'B', 'C'])
    expect(
      sortRows(rows, { metric: 'active', byLabel: true, dir: 'desc' }).map((x) => x.label),
    ).toEqual(['D', 'C', 'B', 'A'])
  })
})

describe('live adapter edges', () => {
  const summary = { unpriced_calls: 3 } as never
  const row = (agent_id: string, operations: number) =>
    ({
      agent_id,
      agent_name: agent_id,
      operations,
      total_cost: operations,
      total_tokens: operations * 10,
    }) as never

  it('candidates are tagged coding-agent OR CLI-sourced; anything else is not a candidate', () => {
    const out = harnessCandidates([
      { id: 'a', name: 'a', tags: ['coding-agent'] },
      { id: 'b', name: 'b', metadata: { source: 'nasiko-cli-integration' } },
      { id: 'c', name: 'c', tags: ['local'], metadata: { source: 'upload' } },
      { id: 'd', name: 'd' },
    ])
    expect(out.map((a) => a.id)).toEqual(['a', 'b'])
  })

  it('Δ is null when the previous window had no turns; a registered harness with no turns is idle', () => {
    const confirmed = new Map<string, string | null>([
      ['a', 'claude'],
      ['b', 'codex'],
      ['x', null],
    ])
    const live = buildLiveIndividual({
      confirmed,
      current: { agents: [row('a', 4), row('x', 50)], summary },
      previous: { agents: [row('a', 0)] },
      prevFailed: false,
      sessions: [],
    })
    const claude = live.harnesses.find((h) => h.harness === 'claude')!
    expect(claude.delta_pct).toBeNull()
    expect(claude.active).toBe(true)
    expect(live.harnesses.find((h) => h.harness === 'codex')).toMatchObject({
      registered: true,
      active: false,
      turns: 0,
    })
    // The unconfirmed agent's 50 turns never count.
    expect(live.totals).toMatchObject({ registered: 2, active: 1, idle: 1, turns: 4 })
  })
})
