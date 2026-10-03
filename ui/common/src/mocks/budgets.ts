/**
 * The R2 budgets mock (plans/feat-llm-router.md §5, the proposed R-L10 contract in
 * docs/designs/openruntime-llm-router-recommendations.md). No server has these endpoints yet: this module IS the
 * contract's reference behaviour. Spend comes from the same seed traces as the router's spend column
 * (`usageByAgent`): an owner budget sums the calls billed to the owner (no flow attribution in the seed, so a call is
 * billed to the agent's owner), an agent budget sums every call to that agent. The period is the UTC calendar month.
 *
 * - A budget belongs to one user (`owner_id`: the agent's owner for an agent budget, whoever set it); one owner budget
 *   per user and one budget per agent, across all users.
 * - A `stop` budget refuses calls once spent: calls after the crossing aren't counted (they never reached a provider).
 * - Alerts are records: written when spend crosses a mark (at the crossing call's time) and never rewritten by an edit.
 *
 * Seeded limits are set from this month's spend, so the three states show on any date: the owner budget sits past
 * 80% (warning), the busiest agent is over its limit with Stop calls (exceeded, stopped), the next one is well under.
 */
import { DEFAULT_THRESHOLDS } from '@/features/router/tuning'
import type {
  Budget,
  BudgetAlert,
  BudgetStatus,
  BudgetStatusResponse,
  CreateBudgetBody,
  UpdateBudgetBody,
} from '@/features/router/types'
import type { MockAgent } from './agents'
import { MockHttpError } from './aggregate'
import type { Seed } from './seed'

const DAY = 86_400_000
const pad = (n: number) => String(n).padStart(12, '0')
const budgetId = (n: number) => `5eed000b-0000-4000-8000-${pad(n)}`

export interface BudgetMockStore {
  /** `stoppedUntil`: mock-only, when a stop budget was switched off (calls it refused before then stay uncounted). */
  budgets: (Budget & { stoppedUntil?: number })[]
  alerts: BudgetAlert[]
  next: number
}

const monthStart = (t: number) => {
  const d = new Date(t)
  return Date.UTC(d.getUTCFullYear(), d.getUTCMonth(), 1)
}
const nextMonth = (t: number) => {
  const d = new Date(t)
  return Date.UTC(d.getUTCFullYear(), d.getUTCMonth() + 1, 1)
}
const cents = (v: number) => Math.round(v * 100) / 100
/** A `stop` budget always alerts at 100 (R-L10). */
const marks = (thresholds: readonly number[], action: Budget['action']) =>
  [...new Set([...thresholds, ...(action === 'stop' ? [100] : [])])].sort((a, b) => a - b)

/**
 * The calls a budget counts in [from, to], oldest first, as [ts, cost]. While a stop budget is spent the router
 * refuses calls, so they aren't counted: through `to` while it is `stop`, and up to `stoppedUntil` after a switch to
 * Alert only (refused calls never become spend after the fact).
 */
function counted(
  b: Budget & { stoppedUntil?: number },
  seed: Seed,
  agents: readonly MockAgent[],
  from: number,
  to: number,
): [number, number][] {
  const owner = new Map(agents.map((a) => [a.id, a.owner_id]))
  const rows = seed.traces
    .filter(
      (t) =>
        t.ts >= from &&
        t.ts <= to &&
        (b.scope === 'agent' ? t.agent_id === b.agent_id : owner.get(t.agent_id) === b.owner_id),
    )
    .map((t): [number, number] => [t.ts, t.cost_usd])
    .sort((x, y) => x[0] - y[0])
  const refusing = b.action === 'stop' ? Infinity : (b.stoppedUntil ?? -Infinity)
  const out: [number, number][] = []
  let sum = 0
  for (const r of rows) {
    if (sum >= b.limit_usd && r[0] <= refusing) continue
    out.push(r)
    sum += r[1]
  }
  return out
}

/** One entry per UTC day of the period up to and including `to`'s day, zeros included. */
function dailySpend(rows: readonly [number, number][], from: number, to: number) {
  const days = Math.floor((to - from) / DAY) + 1
  const out = Array.from({ length: days }, (_, i) => ({
    date: new Date(from + i * DAY).toISOString().slice(0, 10),
    cost_usd: 0,
  }))
  for (const [ts, cost] of rows)
    out[Math.min(days - 1, Math.floor((ts - from) / DAY))]!.cost_usd += cost
  return out.map((d) => ({ ...d, cost_usd: cents(d.cost_usd) }))
}

/** Append an alert for each mark this budget's spend crossed in [from, to] and that has none yet this period. */
function recordCrossings(
  s: BudgetMockStore,
  b: Budget,
  seed: Seed,
  agents: readonly MockAgent[],
  from: number,
  to: number,
) {
  const period = new Date(from).toISOString().slice(0, 7)
  const have = new Set(
    s.alerts.filter((a) => a.budget_id === b.id && a.at.startsWith(period)).map((a) => a.threshold),
  )
  let sum = 0
  for (const [ts, cost] of counted(b, seed, agents, from, to)) {
    sum += cost
    for (const t of b.thresholds) {
      if (have.has(t) || sum < (b.limit_usd * t) / 100) continue
      have.add(t)
      s.alerts.push({
        id: `${b.id}:${period}:${t}`,
        budget_id: b.id,
        threshold: t,
        amount_usd: cents((b.limit_usd * t) / 100),
        at: new Date(ts).toISOString(),
        stopped: t >= 100 && b.action === 'stop',
      })
    }
  }
}

export function buildBudgetState(
  seed: Seed,
  agents: readonly MockAgent[],
  user: string,
  now: number,
  opts: { empty?: boolean } = {},
): BudgetMockStore {
  const s: BudgetMockStore = { budgets: [], alerts: [], next: 1 }
  if (opts.empty) return s
  // Created at the start of last month, so both periods' alerts are this budget's own history.
  const created = new Date(monthStart(monthStart(now) - 1)).toISOString()
  const mk = (
    n: number,
    scope: Budget['scope'],
    agent_id: string | null,
    limit: number,
    action: Budget['action'],
  ): Budget => ({
    id: budgetId(n),
    scope,
    agent_id,
    owner_id: user,
    set_by: user,
    period: 'month',
    limit_usd: limit,
    thresholds: marks(DEFAULT_THRESHOLDS, action),
    action,
    created_at: created,
    updated_at: created,
  })
  const from = monthStart(now)
  const used = (b: Budget) =>
    counted({ ...b, action: 'alert' }, seed, agents, from, now).reduce((t, [, c]) => t + c, 0)
  // Limits from this month's spend, in cents (no integer floors), so the states hold even early on the 1st.
  const owner = mk(1, 'owner', null, 0, 'alert')
  owner.limit_usd = Math.max(0.01, cents(used(owner) / 0.85))
  const mine = agents
    .filter((a) => !a.deleted && a.owner_id === user && !a.tags.includes('coding-agent'))
    .map((a) => ({ a, u: used(mk(0, 'agent', a.id, 0, 'alert')) }))
    .sort((x, y) => y.u - x.u || x.a.name.localeCompare(y.a.name))
  s.budgets.push(owner)
  if (mine[0] && mine[0].u > 0)
    s.budgets.push(mk(2, 'agent', mine[0].a.id, Math.max(0.01, cents(mine[0].u * 0.9)), 'stop'))
  if (mine[1])
    s.budgets.push(mk(3, 'agent', mine[1].a.id, Math.max(1, cents(mine[1].u * 3)), 'alert'))
  s.next = 4
  const prev = monthStart(monthStart(now) - 1)
  for (const b of s.budgets) {
    recordCrossings(s, b, seed, agents, prev, from - 1)
    recordCrossings(s, b, seed, agents, from, now)
  }
  return s
}

const own = (s: BudgetMockStore, user: string) => s.budgets.filter((b) => b.owner_id === user)

export const listBudgets = (s: BudgetMockStore, user: string): Budget[] =>
  own(s, user).map(({ stoppedUntil: _u, ...b }) => b)

function find(s: BudgetMockStore, id: string, user: string) {
  const b = own(s, user).find((x) => x.id === id)
  if (!b) throw new MockHttpError(404, 'budget not found')
  return b
}

function validate(limit: unknown, thresholds: unknown, action: unknown) {
  if (typeof limit !== 'number' || !Number.isFinite(limit) || cents(limit) <= 0)
    throw new MockHttpError(400, 'limit_usd must be greater than 0')
  if (
    !Array.isArray(thresholds) ||
    !thresholds.length ||
    thresholds.some((t) => !Number.isInteger(t) || t < 1 || t > 100) ||
    new Set(thresholds).size !== thresholds.length
  ) {
    throw new MockHttpError(400, 'thresholds must be whole percents from 1 to 100')
  }
  if (action !== 'alert' && action !== 'stop')
    throw new MockHttpError(400, 'action must be alert or stop')
}

/** `agentOwner(id)` is the agent's owner id, or undefined for an unknown or deleted agent (both answer 403). */
export function createBudget(
  s: BudgetMockStore,
  user: string,
  body: CreateBudgetBody,
  agentOwner: (id: string) => string | undefined,
  superuser: boolean,
  now: number,
): Budget {
  if (body.scope !== 'owner' && body.scope !== 'agent')
    throw new MockHttpError(400, 'scope must be owner or agent')
  const thresholds = body.thresholds ?? [...DEFAULT_THRESHOLDS]
  const action = body.action ?? 'alert'
  validate(body.limit_usd, thresholds, action)
  const agent_id = body.scope === 'agent' ? (body.agent_id ?? null) : null
  const owner = body.scope === 'owner' ? user : agent_id ? agentOwner(agent_id) : undefined
  if (!owner || (body.scope === 'agent' && owner !== user && !superuser))
    throw new MockHttpError(403, 'not the agent owner')
  // One owner budget per user, one budget per agent across all users.
  if (
    s.budgets.some((b) =>
      body.scope === 'agent'
        ? b.agent_id === agent_id
        : b.scope === 'owner' && b.owner_id === owner,
    )
  )
    throw new MockHttpError(409, 'budget already exists for this scope')
  const iso = new Date(now).toISOString()
  const b: Budget = {
    id: budgetId(s.next++),
    scope: body.scope,
    agent_id,
    owner_id: owner,
    set_by: user,
    period: 'month',
    limit_usd: cents(body.limit_usd),
    thresholds: marks(thresholds, action),
    action,
    created_at: iso,
    updated_at: iso,
  }
  s.budgets.push(b)
  return b
}

export function updateBudget(
  s: BudgetMockStore,
  id: string,
  user: string,
  body: UpdateBudgetBody,
  now: number,
): Budget {
  const b = find(s, id, user)
  validate(body.limit_usd, body.thresholds, body.action)
  if (body.expected_updated_at !== b.updated_at)
    throw new MockHttpError(409, 'budget changed elsewhere')
  if (b.action === 'stop' && body.action === 'alert') b.stoppedUntil = now
  Object.assign(b, {
    limit_usd: cents(body.limit_usd),
    thresholds: marks(body.thresholds, body.action),
    action: body.action,
    set_by: user,
    updated_at: new Date(now).toISOString(),
  })
  return b
}

export function deleteBudget(s: BudgetMockStore, id: string, user: string) {
  const b = find(s, id, user)
  s.budgets = s.budgets.filter((x) => x !== b)
}

/** The highest mark at or under `used / limit`, or null. */
const crossedAt = (b: Budget, used: number) =>
  [...b.thresholds].reverse().find((t) => used >= (b.limit_usd * t) / 100) ?? null

export function budgetStatus(
  s: BudgetMockStore,
  seed: Seed,
  agents: readonly MockAgent[],
  user: string,
  now: number,
): BudgetStatusResponse {
  const from = monthStart(now)
  const owner = new Map(agents.map((a) => [a.id, a.owner_id]))
  const data: BudgetStatus[] = own(s, user).map((b) => {
    recordCrossings(s, b, seed, agents, from, now)
    const daily = dailySpend(counted(b, seed, agents, from, now), from, now)
    const used = cents(daily.reduce((t, d) => t + d.cost_usd, 0))
    const crossed = crossedAt(b, used)
    const exceeded = used >= b.limit_usd
    const unpriced_calls = seed.traces.filter(
      (t) =>
        t.ts >= from &&
        t.ts <= now &&
        t.cost_usd === 0 &&
        (b.scope === 'agent' ? t.agent_id === b.agent_id : owner.get(t.agent_id) === b.owner_id),
    ).length
    return {
      budget_id: b.id,
      used_usd: used,
      unpriced_calls,
      resets_at: new Date(nextMonth(now)).toISOString(),
      state: exceeded ? 'exceeded' : crossed !== null ? 'warning' : 'ok',
      crossed,
      daily,
      stopped: exceeded && b.action === 'stop',
    }
  })
  return { data, covers: 'routed_calls' }
}

/** This period's and last period's alerts on the caller's budgets, newest first. */
export function budgetAlerts(
  s: BudgetMockStore,
  seed: Seed,
  agents: readonly MockAgent[],
  user: string,
  now: number,
): BudgetAlert[] {
  for (const b of own(s, user)) recordCrossings(s, b, seed, agents, monthStart(now), now)
  const ids = new Set(own(s, user).map((b) => b.id))
  const since = new Date(monthStart(monthStart(now) - 1)).toISOString()
  return s.alerts
    .filter((a) => ids.has(a.budget_id) && a.at >= since && a.at <= new Date(now).toISOString())
    .sort((x, y) => y.at.localeCompare(x.at) || y.threshold - x.threshold)
}
