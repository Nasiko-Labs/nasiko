import { describe, expect, it } from 'vitest'
import { harnessNarrative } from '@/features/narrative/harness'
import { MOCK_VARIANTS } from '@/mocks/handlers'
import { myAgentDashboardRows, fleetUnpriced, usage } from '@/mocks/harnessUsage'
import { generateHarnessSeed } from '@/mocks/seed-harness'
import { isEndpointAbsent, isNotVisible } from './api'
import { CALLOUT_MIN_SCOPE_DEVS, MOSTLY_UNPRICED_RATIO } from './constants'
import { copy } from './copy'
import { buildLiveIndividual, harnessCandidates, type CatalogAgent } from './liveIndividual'
import {
  adoption,
  cellState,
  costPerActiveDev,
  costPerSession,
  costView,
  harnessStyle,
  mostlyUnpriced,
  orderHarnesses,
  pickCallout,
  sortRows,
  splitRows,
  windowDays,
  clampFrom,
} from './rollup'
import { HARNESS_MOCK_VARIANTS } from './search'
import type { HarnessTotals, UsageRow } from './types'
import { ApiError } from '@/lib/api/client'

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
const row = (label: string, kind: UsageRow['kind'], active: number): UsageRow => ({
  key: label,
  label,
  kind,
  harness_breakdown: {},
  totals: T({ active_devs: active }),
})

describe('rollup', () => {
  it('harnessStyle: fixed tokens for known ids, "Other" + --chart-other for unknown', () => {
    expect(harnessStyle('claude')).toMatchObject({
      name: 'Claude Code',
      color: 'var(--chart-1)',
      edge: 'var(--chart-1-edge)',
      known: true,
    })
    expect(harnessStyle('cursor').color).toBe('var(--chart-4)')
    expect(harnessStyle('windsurf')).toMatchObject({
      name: 'Other',
      color: 'var(--chart-other)',
      known: false,
    })
  })

  it('orders known harnesses first, unknown after', () => {
    expect(orderHarnesses(['windsurf', 'cursor', 'claude', 'aider'])).toEqual([
      'claude',
      'cursor',
      'aider',
      'windsurf',
    ])
  })

  it('zero denominators give null ("—")', () => {
    expect(costPerActiveDev(T({ active_devs: 0 }))).toBeNull()
    expect(costPerSession(T({ sessions: 0 }))).toBeNull()
    expect(adoption(T({ scope_devs: 0 }))).toBeNull()
    expect(costPerActiveDev(T())).toBe(10)
    expect(costPerSession(T())).toBe(2.5)
  })

  it(`cost reads "unpriced" above ${MOSTLY_UNPRICED_RATIO} unpriced turns, except in the live fallback`, () => {
    expect(mostlyUnpriced({ turns: 10, unpriced_calls: 6 })).toBe(true)
    expect(mostlyUnpriced({ turns: 10, unpriced_calls: 5 })).toBe(false)
    expect(costView(T({ unpriced_calls: 60 }))).toEqual({ kind: 'unpriced' })
    expect(costView(T({ unpriced_calls: 60 }), false).kind).toBe('value')
    // Zero turns is never "mostly unpriced" (no division by zero).
    expect(mostlyUnpriced({ turns: 0, unpriced_calls: 3 })).toBe(false)
  })

  it('team cell states: value, idle, —', () => {
    expect(cellState(T())).toBe('value')
    expect(cellState(T({ active_devs: 0 }))).toBe('idle')
    expect(cellState(T({ active_devs: 0, registered_devs: 0 }))).toBe('none')
    expect(cellState(undefined)).toBe('none')
  })

  it('sorting never moves residual rows', () => {
    const rows = [row('Unassigned', 'unassigned', 99), row('B', 'unit', 5), row('A', 'unit', 9)]
    expect(splitRows(rows).residual.map((r) => r.label)).toEqual(['Unassigned'])
    expect(sortRows(rows, { metric: 'active', dir: 'desc' }).map((r) => r.label)).toEqual([
      'A',
      'B',
      'Unassigned',
    ])
    expect(sortRows(rows, { metric: 'active', dir: 'asc' }).map((r) => r.label)).toEqual([
      'B',
      'A',
      'Unassigned',
    ])
  })

  it('callout precedence: fastest-growing harness, then lowest-adoption unit; idle is never a callout (the sentence says it)', () => {
    const base = { totals: T({ idle_seats: 0 }), by_harness: [], rows: [] }
    expect(pickCallout({ ...base, totals: T({ idle_seats: 4 }) })).toBeNull()
    const growth = {
      ...base,
      by_harness: [
        { ...T({ delta_pct: 12 }), harness: 'codex', top_models: [] },
        { ...T({ delta_pct: 30 }), harness: 'claude', top_models: [] },
      ],
    }
    expect(pickCallout(growth)).toEqual({ kind: 'growth', harness: 'claude', pct: 30 })
    const small = {
      ...row('Tiny', 'unit', 0),
      totals: T({ scope_devs: CALLOUT_MIN_SCOPE_DEVS - 1, active_devs: 0 }),
    }
    const low = { ...row('Low', 'unit', 1), totals: T({ scope_devs: 10, active_devs: 1 }) }
    expect(pickCallout({ ...base, rows: [small, low] })).toMatchObject({
      kind: 'lowest',
      label: 'Low',
    })
    expect(pickCallout({ ...base, totals: T({ scope_devs: 0 }) })).toBeNull()
    // Never "turns up 0%", and nothing to single out when nothing ran.
    expect(
      pickCallout({
        ...base,
        by_harness: [{ ...T({ delta_pct: 0.4 }), harness: 'codex', top_models: [] }],
      }),
    ).toBeNull()
    expect(pickCallout({ ...base, totals: T({ active_devs: 0 }), rows: [low] })).toBeNull()
  })
})

describe('narrative', () => {
  const res = (p: Partial<HarnessTotals>, harnesses: [string, Partial<HarnessTotals>][]) => ({
    totals: T(p),
    by_harness: harnesses.map(([h, t]) => ({ ...T(t), harness: h, top_models: [] })),
  })
  it('group: adoption, lead harness, then idle and estimated cost; never more than 2 sentences', () => {
    const s = harnessNarrative({
      windowLabel: 'Last 30 days',
      res: res({ active_devs: 41, scope_devs: 60, idle_seats: 12 }, [
        ['claude', { active_devs: 31, cost_usd: 900 }],
        ['codex', { active_devs: 10, cost_usd: 100 }],
      ]),
    })
    expect(s).toHaveLength(2)
    expect(s[0]).toBe(
      'In the last 30 days, 41 of 60 developers used a harness connected to OpenRuntime; Claude Code leads with 31 active developers.',
    )
    expect(s[1]).toBe(
      '12 registered seats had no activity; estimated cost $1,000.00 (API list price).',
    )
  })
  it('drops the cost clause when everything is unpriced', () => {
    const s = harnessNarrative({
      windowLabel: 'Last 7 days',
      res: res({ idle_seats: 0 }, [['cursor', { turns: 10, unpriced_calls: 10, cost_usd: 0 }]]),
    })
    expect(s.join(' ')).not.toMatch(/cost/)
  })
  it('0 active and 1 harness', () => {
    expect(
      harnessNarrative({
        windowLabel: 'Last 7 days',
        res: res({ active_devs: 0, registered_devs: 0 }, []),
      }),
    ).toEqual(['No harnesses connected to OpenRuntime yet.'])
    expect(
      harnessNarrative({
        windowLabel: 'Last 7 days',
        res: res({ idle_seats: 0 }, [['claude', {}]]),
      })[0],
    ).not.toMatch(/leads/)
  })
  it('individual speaks to "your account", never "single user"', () => {
    const s = harnessNarrative({
      windowLabel: 'Last 30 days',
      res: res({ idle_seats: 1 }, [
        ['claude', { active_devs: 1, registered_devs: 1 }],
        ['cursor', { active_devs: 0, registered_devs: 1 }],
        ['codex', { active_devs: 1, registered_devs: 1 }],
      ]),
      individual: { self: true, name: 'Ada' },
    })
    expect(s[0]).toBe('In the last 30 days, you used 2 of your 3 connected harnesses.')
  })
})

describe('copy', () => {
  it('says "Registered", explains auto-registration, and every notice carries an action', () => {
    expect(copy.registered).toBe('Registered')
    expect(copy.registeredTip).toMatch(/auth login/)
    for (const n of [copy.serverMissing]) expect(n.action.length).toBeGreaterThan(0)
    // The OSS page says nothing about another edition (docs/lab-vs-react-migration-review.md §10.1).
    expect(JSON.stringify(copy)).not.toMatch(/Enterprise|edition/i)
    expect(copy.csvCostHeader).toBe('Estimated cost (API list price, USD)')
  })
  it('keeps the route mock list equal to MOCK_VARIANTS', () => {
    expect([...HARNESS_MOCK_VARIANTS]).toEqual([...MOCK_VARIANTS])
  })
})

describe('api error routing', () => {
  it('a bare 404 means the endpoint is absent; a coded 404 means not visible', () => {
    expect(isEndpointAbsent(new ApiError(404, 'Not Found', '/x', ''))).toBe(true)
    const coded = new ApiError(
      404,
      { error: 'unit not found or not visible', code: 'unit_not_visible' },
      '/x',
      '',
    )
    expect(isEndpointAbsent(coded)).toBe(false)
    expect(isNotVisible(coded)).toBe(true)
  })
})

describe('live individual adapter', () => {
  const seed = generateHarnessSeed({ anchor: new Date('2026-03-20T15:00:00Z') })
  const admin = seed.users.find((u) => u.username === 'admin')!
  const end = Date.parse('2026-03-20T15:00:00Z')
  const start = end - 30 * 86_400_000
  const mine = seed.agents.filter((a) => a.owner_id === admin.id && !a.deleted)
  const catalog: CatalogAgent[] = mine.map((a) => ({
    id: a.id,
    name: a.name,
    tags: ['local', 'coding-agent'],
    metadata: {
      source: 'nasiko-cli-integration',
      integration_id: a.spoofed ? 'claude' : a.harness,
    },
  }))
  const confirmed = new Map(mine.map((a) => [a.id, a.spoofed ? null : a.harness] as const))
  const current = {
    agents: myAgentDashboardRows(seed, admin, start, end),
    summary: { unpriced_calls: fleetUnpriced(seed, start, end) } as never,
  }

  it('ignores a spoofed-metadata agent (only the detail call decides)', () => {
    expect(harnessCandidates(catalog).length).toBe(mine.length) // the spoof is a candidate…
    // Give the spoof real dashboard activity; none of it may land in the harness it claims.
    const spoof = mine.find((a) => a.spoofed)!
    const withSpoof = {
      ...current,
      agents: [
        ...current.agents,
        {
          ...current.agents[0]!,
          agent_id: spoof.id,
          agent_name: spoof.name,
          operations: 999,
          total_cost: 99,
        },
      ],
    }
    const live = buildLiveIndividual({
      confirmed,
      current: withSpoof,
      previous: null,
      prevFailed: false,
      sessions: [],
    })
    const realClaude = new Set(
      mine.filter((a) => !a.spoofed && a.harness === 'claude').map((a) => a.id),
    )
    const realTurns = current.agents
      .filter((r) => r.agent_id && realClaude.has(r.agent_id))
      .reduce((n, r) => n + r.operations, 0)
    expect(live.harnesses.find((h) => h.harness === 'claude')!.turns).toBe(realTurns)
    expect(live.totals.turns).toBe(
      current.agents
        .filter((r) => confirmed.get(r.agent_id!))
        .reduce((n, r) => n + r.operations, 0),
    )
  })

  it('agrees with the mocked usage endpoint for the same viewer and window (T1 invariant)', () => {
    const live = buildLiveIndividual({
      confirmed,
      current,
      previous: null,
      prevFailed: false,
      sessions: [],
    })
    const res = usage(
      seed,
      {
        user_id: admin.id,
        start_time: new Date(start).toISOString(),
        end_time: new Date(end).toISOString(),
      },
      admin,
      end,
    )
    for (const h of live.harnesses) {
      const u = res.by_harness.find((b) => b.harness === h.harness)!
      // The live path sees live agents only; the endpoint also counts deleted rows (none for admin).
      expect(h.turns).toBe(u.turns)
      expect(h.cost_usd).toBeCloseTo(u.cost_usd, 5)
    }
  })

  it('Δ on turns, "Δ unavailable" when the previous call failed, no Other bucket', () => {
    const prev = {
      agents: current.agents.map((r) => ({
        ...r,
        operations: Math.max(1, Math.round(r.operations / 2)),
      })),
    }
    const live = buildLiveIndividual({
      confirmed,
      current,
      previous: prev,
      prevFailed: false,
      sessions: [],
    })
    expect(live.harnesses.find((h) => h.active)!.delta_pct).toBeGreaterThan(0)
    const failed = buildLiveIndividual({
      confirmed,
      current,
      previous: null,
      prevFailed: true,
      sessions: [],
    })
    expect(failed.prevUnavailable).toBe(true)
    expect(failed.harnesses.every((h) => h.delta_pct === null)).toBe(true)
  })

  it('keeps only chat sessions of confirmed harness agents', () => {
    const a = mine.find((x) => !x.spoofed)!
    const spoof = mine.find((x) => x.spoofed)!
    const live = buildLiveIndividual({
      confirmed,
      current,
      previous: null,
      prevFailed: false,
      sessions: [
        {
          session_id: 's1',
          agent_id: a.id,
          created_at: '2026-03-19T10:00:00Z',
          updated_at: '2026-03-19T10:00:00Z',
          is_coding_agent: true,
          message_count: 4,
          total_tokens: 900,
        },
        {
          session_id: 's2',
          agent_id: spoof.id,
          created_at: '2026-03-19T11:00:00Z',
          updated_at: '2026-03-19T11:00:00Z',
          is_coding_agent: true,
          message_count: 2,
          total_tokens: 100,
        },
        {
          session_id: 's3',
          agent_id: 'x',
          created_at: '2026-03-19T12:00:00Z',
          updated_at: '2026-03-19T12:00:00Z',
          is_coding_agent: false,
          message_count: 2,
          total_tokens: 100,
        },
      ],
    })
    expect(live.sessions.map((s) => s.session_id)).toEqual(['s1'])
  })
})

describe('review fixes', () => {
  it('windowDays covers every UTC day the window touches, including today', () => {
    expect(windowDays(new Date('2026-03-18T15:00:00Z'), new Date('2026-03-20T15:00:00Z'))).toEqual([
      '2026-03-18',
      '2026-03-19',
      '2026-03-20',
    ])
    expect(windowDays(new Date('2026-03-01T00:00:00Z'), new Date('2026-03-01T00:30:00Z'))).toEqual([
      '2026-03-01',
    ])
    // An end at exactly midnight does not add that (empty) day.
    expect(windowDays(new Date('2026-03-01T00:00:00Z'), new Date('2026-03-03T00:00:00Z'))).toEqual([
      '2026-03-01',
      '2026-03-02',
    ])
    // At most MAX_WINDOW_DAYS, the newest ones.
    const long = windowDays(new Date('0001-01-01T00:00:00Z'), new Date('2026-03-10T12:00:00Z'))
    expect(long).toHaveLength(366)
    expect(long.at(-1)).toBe('2026-03-10')
  })

  it('clampFrom keeps a custom from within 366 days of to', () => {
    expect(clampFrom('0001-01-01', '2026-03-10')).toBe('2025-03-10')
    expect(clampFrom('2026-01-01', '2026-03-10')).toBe('2026-01-01')
    expect(clampFrom(undefined, '2026-03-10')).toBeUndefined()
    // The page passes min(to, today): a future `to` never pushes `from` past today (which would fall back to 30d).
    expect(clampFrom('2026-01-01', '2026-03-10')).toBe('2026-01-01')
  })

  it('the Individual sentence never says "used 2 of your 1" (activity through a removed registration)', () => {
    const h = (harness: string, p: Partial<HarnessTotals>) => ({ ...T(p), harness, top_models: [] })
    const by_harness = [
      h('claude', { active_devs: 1, registered_devs: 1 }),
      h('codex', { active_devs: 1, registered_devs: 0 }),
    ]
    const [first] = harnessNarrative({
      windowLabel: 'Last 30 days',
      res: { totals: T({ scope_devs: 1, active_devs: 1, registered_devs: 1 }), by_harness },
      individual: { self: true, name: 'You' },
    })
    expect(first).toBe('In the last 30 days, you used all 2 of your connected harnesses.')
  })
})
