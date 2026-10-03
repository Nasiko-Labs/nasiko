// @vitest-environment node
// The budgets mock is the proposed R-L10 contract's reference behaviour (plans/feat-llm-router.md §5).
import { describe, expect, it } from 'vitest'
import { buildAgentsState } from './agents'
import {
  budgetAlerts,
  budgetStatus,
  buildBudgetState,
  createBudget,
  deleteBudget,
  listBudgets,
  updateBudget,
} from './budgets'
import { ADMIN_ID, generateHarnessSeed } from './seed-harness'
import { generateSeed } from './seed'

const FIXED = new Date('2026-03-20T15:00:00Z')
const now = FIXED.getTime()
const seed = generateSeed({ anchor: FIXED })
const agents = buildAgentsState(seed, generateHarnessSeed({ anchor: FIXED }), now).agents
const fresh = () => buildBudgetState(seed, agents, ADMIN_ID, now)
const agentOwner = (id: string) => agents.find((a) => !a.deleted && a.id === id)?.owner_id
const mine = agents.filter((a) => !a.deleted && a.owner_id === ADMIN_ID)
const other = agents.find((a) => !a.deleted && a.owner_id !== ADMIN_ID)!

describe('seeded budgets', () => {
  it('show all three states: the owner past 80%, the busiest agent stopped, the next one OK', () => {
    const s = fresh()
    const st = budgetStatus(s, seed, agents, ADMIN_ID, now)
    expect(st.covers).toBe('routed_calls')
    const byId = new Map(listBudgets(s, ADMIN_ID).map((b) => [b.id, b]))
    const [owner, stopped, ok] = st.data
    expect(byId.get(owner!.budget_id)!.scope).toBe('owner')
    expect(owner).toMatchObject({ state: 'warning', crossed: 80, stopped: false })
    expect(stopped).toMatchObject({ state: 'exceeded', crossed: 100, stopped: true })
    expect(byId.get(stopped!.budget_id)!.action).toBe('stop')
    expect(ok!.state).toBe('ok')
  })

  it('hold early on the 1st too (limits in cents, no integer floors)', () => {
    const t = new Date('2026-03-01T06:00:00Z').getTime()
    const s = buildBudgetState(seed, agents, ADMIN_ID, t)
    const states = budgetStatus(s, seed, agents, ADMIN_ID, t).data.map((d) => d.state)
    expect(states).toContain('exceeded')
  })

  it('daily covers each UTC day so far, including today at midnight, and sums to used', () => {
    for (const s of budgetStatus(fresh(), seed, agents, ADMIN_ID, now).data) {
      expect(s.daily).toHaveLength(20)
      expect(s.daily[0]!.date).toBe('2026-03-01')
      expect(s.daily.reduce((t, d) => t + d.cost_usd, 0)).toBeCloseTo(s.used_usd, 1)
      expect(s.resets_at).toBe('2026-04-01T00:00:00.000Z')
    }
    const midnight = Date.UTC(2026, 2, 20)
    expect(
      budgetStatus(
        buildBudgetState(seed, agents, ADMIN_ID, midnight),
        seed,
        agents,
        ADMIN_ID,
        midnight,
      ).data[0]!.daily,
    ).toHaveLength(20)
  })

  it('a stop budget stops counting once spent (refused calls never reached a provider)', () => {
    const s = fresh()
    const b = listBudgets(s, ADMIN_ID).find((x) => x.action === 'stop')!
    const st = budgetStatus(s, seed, agents, ADMIN_ID, now).data.find((d) => d.budget_id === b.id)!
    // At most one call past the limit: the one that crossed it.
    const maxCall = Math.max(
      ...seed.traces.filter((t) => t.agent_id === b.agent_id).map((t) => t.cost_usd),
    )
    expect(st.used_usd).toBeLessThanOrEqual(b.limit_usd + maxCall + 0.01)
  })

  it('counts unpriced calls', () => {
    const st = budgetStatus(fresh(), seed, agents, ADMIN_ID, now).data
    expect(st.some((d) => d.unpriced_calls > 0)).toBe(true)
  })

  it('alerts: one per budget per mark per period, newest first; stopped exactly at 100% on a stop budget', () => {
    const s = fresh()
    const alerts = budgetAlerts(s, seed, agents, ADMIN_ID, now)
    const keys = alerts.map((a) => `${a.budget_id}:${a.at.slice(0, 7)}:${a.threshold}`)
    expect(new Set(keys).size).toBe(keys.length)
    expect([...alerts].sort((a, b) => b.at.localeCompare(a.at)).map((a) => a.at)).toEqual(
      alerts.map((a) => a.at),
    )
    const byId = new Map(listBudgets(s, ADMIN_ID).map((b) => [b.id, b]))
    for (const a of alerts)
      expect(a.stopped).toBe(a.threshold >= 100 && byId.get(a.budget_id)!.action === 'stop')
    expect(alerts.some((a) => a.stopped)).toBe(true)
    for (const a of alerts) expect(a.at <= FIXED.toISOString()).toBe(true)
  })

  it('alerts are records: raising a limit or switching to Alert only doesn’t rewrite them', () => {
    const s = fresh()
    const before = budgetAlerts(s, seed, agents, ADMIN_ID, now)
    const b = listBudgets(s, ADMIN_ID).find((x) => x.action === 'stop')!
    updateBudget(
      s,
      b.id,
      ADMIN_ID,
      {
        limit_usd: 1000,
        thresholds: [50, 80, 100],
        action: 'alert',
        expected_updated_at: b.updated_at,
      },
      now,
    )
    expect(budgetAlerts(s, seed, agents, ADMIN_ID, now)).toEqual(before)
  })

  it('an empty state has no budgets', () => {
    expect(
      listBudgets(buildBudgetState(seed, agents, ADMIN_ID, now, { empty: true }), ADMIN_ID),
    ).toEqual([])
  })
})

describe('writes', () => {
  const empty = () => buildBudgetState(seed, agents, ADMIN_ID, now, { empty: true })

  it('create defaults to Alert only at 50/80/100; one owner budget per user', () => {
    const s = empty()
    const b = createBudget(s, ADMIN_ID, { scope: 'owner', limit_usd: 10 }, agentOwner, false, now)
    expect(b).toMatchObject({
      scope: 'owner',
      agent_id: null,
      owner_id: ADMIN_ID,
      set_by: ADMIN_ID,
      action: 'alert',
      thresholds: [50, 80, 100],
      limit_usd: 10,
    })
    expect(() =>
      createBudget(s, ADMIN_ID, { scope: 'owner', limit_usd: 5 }, agentOwner, false, now),
    ).toThrow('budget already exists for this scope')
  })

  it('one budget per agent across all users; a superuser’s budget belongs to the agent’s owner', () => {
    const s = empty()
    const b = createBudget(
      s,
      ADMIN_ID,
      { scope: 'agent', agent_id: other.id, limit_usd: 5 },
      agentOwner,
      true,
      now,
    )
    expect(b).toMatchObject({ owner_id: other.owner_id, set_by: ADMIN_ID })
    expect(listBudgets(s, other.owner_id).map((x) => x.id)).toEqual([b.id])
    expect(listBudgets(s, ADMIN_ID)).toEqual([])
    expect(() =>
      createBudget(
        s,
        other.owner_id,
        { scope: 'agent', agent_id: other.id, limit_usd: 5 },
        agentOwner,
        false,
        now,
      ),
    ).toThrow('budget already exists for this scope')
  })

  it('a stop budget always alerts at 100', () => {
    const b = createBudget(
      empty(),
      ADMIN_ID,
      { scope: 'agent', agent_id: mine[0]!.id, limit_usd: 5, thresholds: [50], action: 'stop' },
      agentOwner,
      false,
      now,
    )
    expect(b.thresholds).toEqual([50, 100])
  })

  it('validates the limit, marks, scope, action and agent ownership (unknown agents look the same as others’)', () => {
    const s = empty()
    const c =
      (body: object, su = false) =>
      () =>
        createBudget(s, ADMIN_ID, body as never, agentOwner, su, now)
    expect(c({ scope: 'owner', limit_usd: 0 })).toThrow('limit_usd must be greater than 0')
    expect(c({ scope: 'owner', limit_usd: 0.004 })).toThrow('limit_usd must be greater than 0')
    expect(c({ scope: 'owner', limit_usd: 5, thresholds: [50, 50] })).toThrow(
      'thresholds must be whole percents from 1 to 100',
    )
    expect(c({ scope: 'team', limit_usd: 5 })).toThrow('scope must be owner or agent')
    expect(c({ scope: 'owner', limit_usd: 5, action: 'pause' })).toThrow(
      'action must be alert or stop',
    )
    expect(c({ scope: 'agent', agent_id: other.id, limit_usd: 5 })).toThrow('not the agent owner')
    expect(c({ scope: 'agent', agent_id: 'no-such-agent', limit_usd: 5 })).toThrow(
      'not the agent owner',
    )
    expect(c({ scope: 'agent', limit_usd: 5 })).toThrow('not the agent owner')
  })

  it('PUT is a guarded full replace; DELETE removes; an unknown id is a 404', () => {
    const s = fresh()
    const b = listBudgets(s, ADMIN_ID)[1]!
    const readAt = b.updated_at
    expect(() =>
      updateBudget(
        s,
        b.id,
        ADMIN_ID,
        { limit_usd: 99, thresholds: [100], action: 'alert', expected_updated_at: 'stale' },
        now,
      ),
    ).toThrow('budget changed elsewhere')
    expect(() =>
      updateBudget(
        s,
        b.id,
        ADMIN_ID,
        { limit_usd: 99, thresholds: [100], action: 'nope' as never, expected_updated_at: readAt },
        now,
      ),
    ).toThrow('action must be alert or stop')
    expect(
      updateBudget(
        s,
        b.id,
        ADMIN_ID,
        { limit_usd: 99, thresholds: [90, 10], action: 'alert', expected_updated_at: readAt },
        now + 1000,
      ),
    ).toMatchObject({ limit_usd: 99, thresholds: [10, 90], action: 'alert' })
    expect(listBudgets(s, ADMIN_ID).find((x) => x.id === b.id)!.updated_at).not.toBe(readAt)
    deleteBudget(s, b.id, ADMIN_ID)
    expect(listBudgets(s, ADMIN_ID).some((x) => x.id === b.id)).toBe(false)
    expect(() => deleteBudget(s, b.id, ADMIN_ID)).toThrow('budget not found')
    expect(() =>
      updateBudget(
        s,
        b.id,
        'someone-else',
        { limit_usd: 1, thresholds: [100], action: 'alert', expected_updated_at: '' },
        now,
      ),
    ).toThrow('budget not found')
  })
})
