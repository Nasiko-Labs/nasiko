/**
 * The Map view's board over the OV-1 mock: every card's rows come from the estate (agents by spend with 18 day tiles,
 * a segment per workflow step, tools most used first with a recently turned-off one faded and an old one left out,
 * harness kinds by spend), the routers and providers reconcile with the agents' spend, the providers' model bars share
 * one scale, the bill adds up the same from both sides, and the spend line marks only the peaks over its line.
 */
import { describe, expect, it } from 'vitest'
import { setupPinnedSeed } from '@nasiko/testing/pinnedSeed'
import {
  AGENT_DAYS,
  HARNESS_CELLS,
  SHOWN,
  TOOL_OFF_RECENT_DAYS,
  billOf,
  buildBoard,
  dailyBudget,
  harnessLabel,
  spendCard,
  toneOf,
  uses,
} from './model'
import { estate, viewOf } from '@nasiko/testing/estateView'

setupPinnedSeed()

describe('the board', () => {
  it('shows the biggest agents with today and the last 18 days, and counts them all', async () => {
    const e = await estate()
    const b = buildBoard(viewOf(e))
    expect(b.agentCount).toBe(e.agents.length)
    expect(b.agents).toHaveLength(Math.min(SHOWN.agents, e.agents.length))
    const spend = b.agents.map((a) => a.spend)
    expect([...spend].sort((x, y) => y - x)).toEqual(spend)
    for (const a of b.agents) expect(a.days).toHaveLength(AGENT_DAYS)
    // Days before the window are empty, never healthy.
    if (e.days.length < AGENT_DAYS) expect(b.agents[0]?.days[0]).toBe('none')
    expect(b.agents.some((a) => a.now === 'bad')).toBe(true)
  })

  it('draws a segment per workflow step and counts what calls each tool', async () => {
    const e = await estate()
    const b = buildBoard(viewOf(e))
    for (const w of b.workflows)
      expect(w.steps).toHaveLength(e.workflows.find((x) => x.id === w.id)?.steps.length ?? -1)
    for (const t of b.tools) {
      const tool = e.tools.find((x) => x.id === t.id)
      expect(t.agents).toBe(e.agents.filter((a) => a.tool_ids.includes(t.id)).length)
      expect(t.connector).toBe(!!tool?.connector_id)
    }
    // Most used first, the ones turned off after them.
    const on = b.tools.filter((t) => !t.offSince).map(uses)
    expect([...on].sort((x, y) => y - x)).toEqual(on)
    expect(b.tools.findIndex((t) => t.offSince)).toBeGreaterThanOrEqual(on.length)
    expect(b.toolCount).toBe(e.tools.length)
  })

  it('fades a tool turned off lately and leaves out one off for longer', async () => {
    const e = await estate()
    const b = buildBoard(viewOf(e))
    const age = (t: (typeof e.tools)[number]) =>
      (Date.parse(e.as_of) - Date.parse(t.disabled_at ?? e.as_of)) / 86_400_000
    const recent = e.tools.filter((t) => t.disabled_at && age(t) <= TOOL_OFF_RECENT_DAYS)
    const old = e.tools.filter((t) => t.disabled_at && age(t) > TOOL_OFF_RECENT_DAYS)
    expect(recent.length).toBeGreaterThan(0)
    expect(old.length).toBeGreaterThan(0)
    for (const t of recent) expect(b.tools.find((x) => x.id === t.id)?.offSince).toBe(t.disabled_at)
    for (const t of old) expect(b.tools.some((x) => x.id === t.id)).toBe(false)
  })

  it('groups coding harnesses by kind, biggest first, forty installs a kind at most', async () => {
    const e = await estate()
    const b = buildBoard(viewOf(e))
    expect(b.harnessCount).toBe(e.harnesses.length)
    expect(b.harnesses.length).toBeLessThanOrEqual(SHOWN.harnesses)
    expect(b.harnesses[0]?.label).toBe('Claude Code')
    for (const c of b.harnesses) {
      expect(c.cells.length).toBeLessThanOrEqual(HARNESS_CELLS)
      const kind = e.harnesses.filter((h) => h.harness === c.key)
      expect(c.installs).toBe(kind.length)
      expect(c.spend).toBeCloseTo(kind.reduce((n, h) => n + (h.spend_usd ?? 0), 0))
    }
    expect(harnessLabel('Claude Code (Rae)')).toBe('Claude Code')
  })

  it('reconciles routers and their providers with the agents', async () => {
    const e = await estate()
    const b = buildBoard(viewOf(e))
    for (const r of b.routers) {
      const agents = e.agents.filter((a) => a.router_id === r.id)
      expect(r.spend).toBeCloseTo(agents.reduce((n, a) => n + a.spend_usd, 0))
      if (r.providers.length) {
        expect(r.providers.reduce((n, p) => n + p.spend, 0)).toBeCloseTo(r.spend)
        expect(r.providers.reduce((n, p) => n + p.share, 0)).toBeCloseTo(1)
      }
    }
    // One scale for every provider's column: the biggest shown model is the full bar.
    const bars = b.providers.flatMap((p) => p.models.map((m) => m.bar))
    expect(Math.max(...bars)).toBe(1)
    for (const p of b.providers) expect(p.models.length).toBeLessThanOrEqual(SHOWN.models)
  })

  it('adds the bill up the same from both sides', async () => {
    const e = await estate()
    const bill = billOf(e)
    const agents = e.agents.reduce((n, a) => n + a.spend_usd, 0)
    const harnesses = e.harnesses.reduce((n, h) => n + (h.spend_usd ?? 0), 0)
    expect(bill.agents).toBeCloseTo(agents)
    expect(bill.harnesses).toBeCloseTo(harnesses)
    expect(bill.total).toBeCloseTo(agents + harnesses)
    const byProvider =
      bill.providers.reduce((n, p) => n + p.spend, 0) + bill.otherProviders + bill.unattributed
    expect(byProvider).toBeCloseTo(bill.total, 1)
    const spend = bill.providers.map((p) => p.spend)
    expect([...spend].sort((x, y) => y - x)).toEqual(spend)
    expect(bill.providers.length).toBeLessThanOrEqual(SHOWN.billProviders)
  })

  it('lists every team in the Org, over pace first, with its people as tiles and the teamless last', async () => {
    const e = await estate()
    const b = buildBoard(viewOf(e))
    const named = b.teams.filter((t) => t.id)
    expect(named.map((t) => t.id).sort()).toEqual(e.groups.map((g) => g.id).sort())
    const paces = named.map((t) => t.pace ?? 1.001)
    expect([...paces].sort((x, y) => y - x)).toEqual(paces)
    for (const t of named)
      expect(t.people).toHaveLength(e.people.filter((p) => p.group_id === t.id).length)
    if (e.people.some((p) => !p.group_id)) expect(b.teams.at(-1)?.id).toBeNull()
    // What flows to the teams is every agent's spend and every priced harness install's, once.
    const flow = b.teams.reduce((n, t) => n + t.usd, 0)
    const spent =
      e.agents.reduce((n, a) => n + a.spend_usd, 0) +
      e.harnesses.reduce((n, h) => n + (h.spend_usd ?? 0), 0)
    expect(flow).toBeCloseTo(spent)
  })

  it('fills each harness column to forty seats, the free ones grey', async () => {
    const e = await estate()
    for (const c of buildBoard(viewOf(e)).harnesses)
      expect(c.cells.length + c.slots).toBe(HARNESS_CELLS)
  })
})

describe('total spend', () => {
  it('marks the peaks over the line and the change on the window before', () => {
    const c = spendCard([1, 5, 2, 2, 6, 1, 3], 20, 25, 2.5)
    expect(c.threshold).toBe(2.5)
    expect(c.spikes).toEqual([1, 4, 6])
    expect(c.changePct).toBeCloseTo(-20)
    // No budget: the line is the mean; no previous window: no change.
    const m = spendCard([2, 4], 6, null, null)
    expect(m.threshold).toBe(3)
    expect(m.changePct).toBeNull()
    expect(spendCard([], 0, 0, null).threshold).toBeNull()
  })

  it('spreads the month budget over the month', async () => {
    const e = await estate()
    const limit = e.groups.reduce((n, g) => n + (g.budget?.limit_usd ?? 0), 0)
    expect(dailyBudget(e, Date.UTC(2026, 1, 10))).toBeCloseTo(limit / 28)
    expect(dailyBudget({ ...e, groups: [] }, Date.now())).toBeNull()
  })

  it('names a tone for every state', () => {
    expect(toneOf('errors')).toBe('bad')
    expect(toneOf('slow')).toBe('warn')
    expect(toneOf('cost')).toBe('warn')
    expect(toneOf('ghost')).toBe('idle')
    expect(toneOf('future')).toBe('none')
  })
})
