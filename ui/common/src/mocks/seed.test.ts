import { describe, expect, it } from 'vitest'
import {
  dashboard,
  dayDrilldown,
  MockHttpError,
  spendCalendar,
  spendTimeseries,
  topTraces,
} from './aggregate'
import { generateSeed } from './seed'

const anchor = new Date('2026-03-20T15:00:00Z')
const now = anchor.getTime()
const seed = generateSeed({ anchor })

describe('seed', () => {
  it('is deterministic for a fixed anchor', () => {
    const again = generateSeed({ anchor })
    expect(again.traces.length).toBe(seed.traces.length)
    expect(again.traces[100]).toEqual(seed.traces[100])
    expect(seed.traces.length).toBeGreaterThan(2000)
  })

  it('has the shapes the UI must handle: unpriced calls, a deleted agent, a spike day, raw ≠ display names', () => {
    expect(seed.traces.some((t) => t.cost_usd === 0 && t.input_tokens > 0)).toBe(true)
    expect(seed.agents.filter((a) => a.deleted)).toHaveLength(1)
    expect(seed.agents.every((a) => a.name !== a.display_name)).toBe(true)
    const spikeDay = seed.traces.filter(
      (t) => t.started_at.startsWith(seed.spikeDate) && t.agent_name === seed.spikeAgentName,
    )
    expect(spikeDay.length).toBeGreaterThan(20)
    expect(seed.traces.every((t) => Date.parse(t.started_at) <= now)).toBe(true)
  })
})

describe('cross-endpoint consistency (mirrors server semantics)', () => {
  const march = spendCalendar(seed, '2026-03', {}, now)
  const calendarSum = march.days.reduce((s, d) => s + d.spend_usd, 0)
  const mtdParams = { start_time: '2026-03-01T00:00:00.000Z', end_time: anchor.toISOString() }

  it('calendar sum equals timeseries sum over the same window', () => {
    const ts = spendTimeseries(seed, mtdParams, now)
    const tsSum = ts.points.reduce((s, p) => s + p.spend_usd, 0)
    expect(ts.bucket).toBe('day')
    expect(tsSum).toBeCloseTo(calendarSum, 4)
  })

  it('dashboard KPI total is lower than the calendar because the deleted agent is excluded (A13)', () => {
    const d = dashboard(seed, mtdParams, now)
    const deleted = seed.agents.find((a) => a.deleted)!
    const deletedSpend = seed.traces
      .filter((t) => t.agent_id === deleted.id && t.started_at >= '2026-03-01')
      .reduce((s, t) => s + t.cost_usd, 0)
    expect(deletedSpend).toBeGreaterThan(0)
    expect(d.kpis.total_spend.current).toBeCloseTo(calendarSum - deletedSpend, 3)
    expect(d.summary.total_agents).toBe(seed.agents.length - 1)
    expect(d.agents[0].agent_name).not.toMatch(/^seed-/) // display names
    expect(d.agents.every((r) => r.is_capped === false)).toBe(true)
  })

  it('only range=24h is hourly; timeseries top agent uses raw names', () => {
    expect(spendTimeseries(seed, { range: '24h' }, now).bucket).toBe('hour')
    expect(spendTimeseries(seed, { range: '7d' }, now).bucket).toBe('day')
    const pt = spendTimeseries(seed, { range: '7d' }, now).points.find((p) => p.top_agent_name)
    expect(pt?.top_agent_name).toMatch(/^seed-/)
  })

  it('day drill-down has 24 hours, top-4 display names (like the server), and consistent totals', () => {
    const d = dayDrilldown(seed, seed.spikeDate, {})
    expect(d.hours).toHaveLength(24)
    const spike = seed.agents.find((a) => a.name === seed.spikeAgentName)!
    expect(d.top_agents[0].agent_name).toBe(spike.display_name)
    expect(d.top_agents.every((a) => a.spend_usd > 0)).toBe(true)
    expect(
      d.hours.every((h) =>
        h.top_agents.every((a) => a.spend_usd > 0 && !a.agent_name.startsWith('seed-')),
      ),
    ).toBe(true)
    const hoursSum = d.hours.reduce((s, h) => s + h.spend_usd, 0)
    const topSum = d.top_agents.reduce((s, a) => s + a.spend_usd, 0)
    expect(topSum + d.others_spend_usd).toBeCloseTo(hoursSum, 4)
  })

  it('agent filter accepts UUID or raw name; unknown → 400 like the server', () => {
    const a = seed.agents[0]
    expect(dashboard(seed, { ...mtdParams, agent_id: a.id }, now).agents).toHaveLength(1)
    expect(dashboard(seed, { ...mtdParams, agent_id: a.name }, now).agents).toHaveLength(1)
    expect(() => dashboard(seed, { agent_id: 'nope' }, now)).toThrow(MockHttpError)
    expect(() => spendTimeseries(seed, { range: '90d' }, now)).toThrow(/invalid range/)
  })

  it('top-traces (proposed contract) sorts by cost with a stable tie-break and pages', () => {
    const page1 = topTraces(seed, { range: '30d', limit: '10' }, now)
    expect(page1.rows).toHaveLength(10)
    expect(page1.has_more).toBe(true)
    for (let i = 1; i < page1.rows.length; i++)
      expect(page1.rows[i - 1].cost_usd).toBeGreaterThanOrEqual(page1.rows[i].cost_usd)
    const page2 = topTraces(seed, { range: '30d', limit: '10', offset: '10' }, now)
    expect(page2.rows[0].trace_id).not.toBe(page1.rows[9].trace_id)
    expect(() => topTraces(seed, { limit: '500' }, now)).toThrow(/limit/)
  })
})
