import { afterEach, describe, expect, it, vi } from 'vitest'
import { buildAttribution, searchRows, sortRows, type AttributionRow } from './attribution'
import { dedupeCatalog } from './catalog'
import { buildConcentration, OTHERS_KEY, segmentAt } from './concentration'
import { csvCell, downloadCsv, toCsv } from './csv'
import { describeDelta, POLARITY } from '@/lib/delta'
import { summarizeMonth } from './forecast'
import {
  fmtCostPerOp,
  fmtDuration,
  fmtHours,
  fmtInt,
  fmtLatency,
  fmtLongDay,
  fmtMoney,
  fmtMoneyAxis,
  fmtMoneyFloor,
  fmtMoneyShort,
  fmtMonthYear,
  fmtPct,
  fmtShortDay,
  fmtTokens,
  fmtUtcDayTime,
  fmtUtcHour,
  fmtUtcTime,
} from '@/lib/format'
import { traceLink } from './links'
import { isUnconfigured, summarizeOptimisation } from './optimisation'
import type { Savings, SavingsData } from './types'
import { isRealDate } from '@/lib/search'
import { tokenopsSearchSchema } from './search'
import { MAX_BUCKETS, toTimeline, zeroFillTimeline } from './series'
import { logTicks } from './stats'
import type { AgentFinopsRow, FinopsDayDrilldown, WorkflowFinopsRow } from './types'
import { resolveWindow } from './window'
import { readEnv, validHttpUrl } from '@/lib/env'
import { safeRedirect } from '@/lib/api/auth'

describe('resolveWindow', () => {
  const now = new Date('2026-03-15T10:30:45.123Z')

  it('rolling presets send range and round now to the minute', () => {
    const w = resolveWindow({ preset: '7d' }, now)
    expect(w.params).toEqual({ range: '7d' })
    expect(w.end.toISOString()).toBe('2026-03-15T10:30:00.000Z')
    expect(w.end.getTime() - w.start.getTime()).toBe(7 * 24 * 3600_000)
    // Same minute → same key (stable query keys).
    expect(resolveWindow({ preset: '7d' }, new Date('2026-03-15T10:30:59Z')).key).toBe(w.key)
  })

  it('previous window is [start − len, start), matching the server', () => {
    const w = resolveWindow({ preset: '24h' }, now)
    expect(w.prevEnd).toEqual(w.start)
    expect(w.start.getTime() - w.prevStart.getTime()).toBe(24 * 3600_000)
    expect(w.prevParams.start_time).toBe(w.prevStart.toISOString())
  })

  it('this month uses UTC bounds regardless of local timezone', () => {
    // 20:00 local on the 1st in a negative-offset zone is still the 1st/2nd UTC; the month is UTC.
    const w = resolveWindow({ preset: 'mtd' }, new Date('2026-04-01T03:00:00Z'))
    expect(w.params.start_time).toBe('2026-04-01T00:00:00.000Z')
    expect(w.label).toBe('This month')
  })

  it('last month spans the full previous UTC month, including a leap February', () => {
    const w = resolveWindow({ preset: 'last-month' }, new Date('2024-03-10T00:00:00Z'))
    expect(w.params.start_time).toBe('2024-02-01T00:00:00.000Z')
    expect(w.params.end_time).toBe('2024-03-01T00:00:00.000Z')
    expect((w.end.getTime() - w.start.getTime()) / 86_400_000).toBe(29)
  })

  it('custom clamps to now and falls back to 30d when invalid', () => {
    const w = resolveWindow({ preset: 'custom', from: '2026-03-10', to: '2026-03-20' }, now)
    expect(w.params.start_time).toBe('2026-03-10T00:00:00.000Z')
    expect(w.end.toISOString()).toBe('2026-03-15T10:30:00.000Z')
    expect(
      resolveWindow({ preset: 'custom', from: '2026-03-20', to: '2026-03-10' }, now).preset,
    ).toBe('30d')
    expect(resolveWindow({ preset: 'custom' }, now).preset).toBe('30d')
  })
})

describe('tokenopsSearchSchema', () => {
  it('falls back to defaults for junk values', () => {
    const s = tokenopsSearchSchema.parse({
      preset: 'forever',
      view: 'robots',
      sort: 'vibes',
      day: 'yesterday',
      traces: 'maybe',
    })
    expect(s.preset).toBe('30d')
    expect(s.view).toBe('agent')
    expect(s.sort).toBe('cost')
    expect(s.day).toBeUndefined()
    expect(s.traces).toBeUndefined()
  })
  it('keeps valid values', () => {
    const s = tokenopsSearchSchema.parse({
      preset: '24h',
      day: '2026-03-01',
      traces: 'true',
      agent: 'seed-support-bot',
    })
    expect(s).toMatchObject({
      preset: '24h',
      day: '2026-03-01',
      traces: true,
      agent: 'seed-support-bot',
    })
  })
})

describe('summarizeMonth (forecast)', () => {
  const day = (date: string, spend: number) => ({
    date,
    spend_usd: spend,
    operations: 1,
    intensity: 0,
  })

  it('hides the forecast before 3 elapsed days and when MTD is zero', () => {
    expect(summarizeMonth([day('2026-03-01', 10)], [], new Date('2026-03-02T12:00:00Z')).show).toBe(
      false,
    )
    expect(summarizeMonth([], [], new Date('2026-03-20T12:00:00Z')).show).toBe(false)
  })

  it('zero-fills quiet days and gives a median..mean band', () => {
    // 10 days elapsed; spend on 4 days only, one spike.
    const days = [
      day('2026-03-01', 10),
      day('2026-03-02', 10),
      day('2026-03-05', 10),
      day('2026-03-08', 200),
    ]
    const s = summarizeMonth(days, [], new Date('2026-03-11T00:00:00Z'))
    expect(s.show).toBe(true)
    expect(s.mtd).toBe(230)
    // median of [10,10,0,0,10,0,0,200,0,0] = 0; mean = 23; 21 days remain.
    expect(s.low).toBeCloseTo(230, 5)
    expect(s.high).toBeCloseTo(230 + 21 * 23, 5)
    expect(Number.isFinite(s.high as number)).toBe(true)
  })

  it('compares against the same elapsed days of last month', () => {
    const last = [day('2026-02-01', 50), day('2026-02-02', 50), day('2026-02-20', 999)]
    const s = summarizeMonth(
      [day('2026-03-01', 30), day('2026-03-02', 30)],
      last,
      new Date('2026-03-03T00:00:00Z'),
    )
    expect(s.lastMonthSameDays).toBe(100)
    expect(s.vsLastMonthPct).toBeCloseTo(-40, 5)
    expect(s.lastMonthTotal).toBe(1099)
  })
})

describe('buildConcentration', () => {
  const drill: FinopsDayDrilldown = {
    date: '2026-03-01',
    avg_hourly_spend_usd: 1,
    others_spend_usd: 3,
    top_agents: [
      { agent_name: 'a', spend_usd: 10 },
      { agent_name: 'b', spend_usd: 5 },
    ],
    hours: Array.from({ length: 24 }, (_, hour) => ({
      hour,
      spend_usd: hour === 9 ? 10 : 0,
      // Slices slightly exceed the hour total (rounding) → Others must clamp at 0.
      top_agents:
        hour === 9
          ? [
              { agent_name: 'a', spend_usd: 7 },
              { agent_name: 'b', spend_usd: 3.5 },
            ]
          : [],
      others_spend_usd: 0,
    })),
  }

  it('builds top-N + Others with Others never negative', () => {
    const c = buildConcentration(drill)
    expect(c.series.map((s) => s.label)).toEqual(['a', 'b', 'Others'])
    expect(c.rows[9].s0).toBe(7)
    expect(c.rows[9][OTHERS_KEY]).toBe(0)
    expect(c.rows[0].label).toBe('12am')
    expect(c.rows[13].label).toBe('1pm')
    expect(c.isEmpty).toBe(false)
  })
})

describe('buildAttribution', () => {
  const row = (
    id: string,
    name: string,
    cost: number,
    extra: Partial<AgentFinopsRow> = {},
  ): AgentFinopsRow => ({
    agent_id: id,
    agent_name: name,
    total_cost: cost,
    operations: 10,
    is_capped: false,
    avg_cost_per_operation: cost / 10,
    prompt_tokens: 1000,
    completion_tokens: 100,
    cache_read_tokens: 250,
    cache_creation_tokens: 0,
    total_tokens: 1100,
    avg_latency_ms: 500,
    avg_latency_p95_ms: 900,
    avg_latency_p99_ms: 1200,
    tool_call_count: 0,
    version: null,
    container_hours: 5,
    ...extra,
  })

  it('joins by id (renamed agents still match), computes share and Δ kinds', () => {
    const cur = {
      view: 'agent' as const,
      rows: [row('1', 'Renamed Bot', 75), row('2', 'New Bot', 25), row('3', 'Quiet', 0)],
    }
    const prev = { view: 'agent' as const, rows: [row('1', 'Old Name', 50), row('3', 'Quiet', 10)] }
    const rows = buildAttribution(cur, prev)
    expect(rows[0]).toMatchObject({
      name: 'Renamed Bot',
      sharePct: 75,
      deltaKind: 'change',
      deltaPct: 50,
      cacheRatioPct: 25,
    })
    expect(rows[1].deltaKind).toBe('new')
    expect(rows[2]).toMatchObject({ deltaKind: 'no-spend', deltaPct: -100 })
  })

  it('marks Δ unavailable when the previous query failed', () => {
    const rows = buildAttribution({ view: 'agent', rows: [row('1', 'x', 1)] }, undefined)
    expect(rows[0].deltaKind).toBe('unavailable')
  })

  it('sorts and searches', () => {
    const rows = buildAttribution(
      { view: 'agent', rows: [row('1', 'beta', 1), row('2', 'Alpha', 9)] },
      undefined,
    )
    expect(sortRows(rows, 'cost').map((r) => r.name)).toEqual(['Alpha', 'beta'])
    expect(sortRows(rows, 'name').map((r) => r.name)).toEqual(['Alpha', 'beta'])
    expect(searchRows(rows, 'ALP').map((r) => r.name)).toEqual(['Alpha'])
  })
})

describe('describeDelta (per-metric polarity)', () => {
  it('spend going up is bad, cache ratio going up is good, operations are neutral', () => {
    expect(describeDelta(12.34, POLARITY.spend)).toMatchObject({
      tone: 'bad',
      arrow: '▲',
      text: '+12.3%',
    })
    expect(describeDelta(-5, POLARITY.spend)).toMatchObject({
      tone: 'good',
      arrow: '▼',
      text: '−5.0%',
    })
    expect(describeDelta(8, POLARITY.cacheRatio).tone).toBe('good')
    expect(describeDelta(8, POLARITY.operations).tone).toBe('neutral')
  })
  it('null change_pct means "new", never a fabricated number', () => {
    expect(describeDelta(null, POLARITY.spend)).toMatchObject({ tone: 'none', text: 'new' })
  })
})

describe('formatters', () => {
  it('formats per plan A10', () => {
    expect(fmtMoney(1234.56)).toBe('$1,234.56')
    expect(fmtMoney(0.004)).toBe('<$0.01')
    expect(fmtMoney(0)).toBe('$0.00')
    expect(fmtTokens(1_234_567)).toBe('1.2M')
    expect(fmtTokens(850)).toBe('850')
    expect(fmtLatency(850)).toBe('850 ms')
    expect(fmtLatency(2400)).toBe('2.4 s')
    expect(fmtPct(12.345)).toBe('12.3%')
    expect(fmtCostPerOp(0.012345)).toBe('$0.0123')
    expect(fmtMoneyFloor(10, true)).toBe('≥ $10.00')
  })
})

describe('csv', () => {
  it('neutralises formula prefixes, doubles quotes, keeps numbers raw', () => {
    expect(csvCell('=HYPERLINK("x")')).toBe(`"'=HYPERLINK(""x"")"`)
    for (const p of ['+', '-', '@', '\t', '\r'])
      expect(csvCell(`${p}cmd`).startsWith(`"'`)).toBe(true)
    expect(csvCell(-12.5)).toBe('"-12.5"')
    expect(csvCell(null)).toBe('""')
    expect(toCsv([{ header: 'a', value: (r: { a: string }) => r.a }], [{ a: 'x' }])).toBe(
      '"a"\r\n"x"',
    )
  })
})

describe('zeroFillTimeline', () => {
  it('fills missing buckets with zero and keeps raw ISO for click resolution', () => {
    const pts = zeroFillTimeline(
      [
        {
          bucket_start: '2026-03-02T00:00:00Z',
          spend_usd: 5,
          operations: 2,
          tool_calls: 0,
          top_agent_name: 'a',
          top_agent_spend_usd: 4,
          p50_latency_ms: 1,
          p95_latency_ms: 2,
          p99_latency_ms: 3,
        },
      ],
      'day',
      new Date('2026-03-01T00:00:00Z'),
      new Date('2026-03-04T00:00:00Z'),
    )
    expect(pts.map((p) => p.spend)).toEqual([0, 5, 0])
    expect(pts[1].iso).toBe('2026-03-02T00:00:00.000Z')
    expect(pts[1].topAgent).toBe('a')
  })
})

describe('links and redirects (A18)', () => {
  it('builds trace links only from a valid http(s) base with encoded ids', () => {
    expect(traceLink('http://localhost:8080', 's 1', 't&2')).toBe(
      'http://localhost:8080/observability-session?session_id=s+1&trace_id=t%262',
    )
    expect(traceLink(null, 's', 't')).toBeNull()
    expect(validHttpUrl('javascript:alert(1)')).toBeNull()
    expect(validHttpUrl('http://localhost:8080/')).toBe('http://localhost:8080')
  })
  it('allows only same-app relative redirect targets', () => {
    expect(safeRedirect('/tokenops?preset=7d')).toBe('/tokenops?preset=7d')
    expect(safeRedirect('//evil.example')).toBe('/')
    expect(safeRedirect('https://evil.example')).toBe('/')
    expect(safeRedirect('/\\evil')).toBe('/')
    expect(safeRedirect('/login?redirect=/x')).toBe('/')
    expect(safeRedirect(undefined)).toBe('/')
  })
})

describe('review follow-ups', () => {
  const now = new Date('2026-03-15T10:30:45.123Z')

  it('search: rejects impossible dates, truncates q instead of dropping it', () => {
    expect(isRealDate('2026-02-28')).toBe(true)
    expect(isRealDate('2026-02-30')).toBe(false)
    expect(isRealDate('2026-13-01')).toBe(false)
    const s = tokenopsSearchSchema.parse({
      from: '2026-02-30',
      day: '2026-13-01',
      q: 'x'.repeat(250),
    })
    expect(s.from).toBeUndefined()
    expect(s.day).toBeUndefined()
    expect(s.q).toHaveLength(200)
  })

  it('window: future custom ranges fall back to 30d', () => {
    expect(
      resolveWindow({ preset: 'custom', from: '2026-03-20', to: '2026-03-25' }, now).preset,
    ).toBe('30d')
    expect(
      resolveWindow({ preset: 'custom', from: '2026-02-30', to: '2026-03-02' }, now).preset,
    ).toBe('30d')
  })

  it('delta: 0 → 0 is unchanged, not "new"', () => {
    expect(describeDelta(null, POLARITY.spend, 0)).toMatchObject({ tone: 'neutral', text: '0.0%' })
    expect(describeDelta(null, POLARITY.spend, 5).text).toBe('new')
  })

  it('csv: neutralises formulas behind whitespace, pipes and full-width prefixes', () => {
    for (const v of [' =1+1', '\n=cmd', '|cmd', '＝1', '＋1', '－1', '＠x'])
      expect(csvCell(v).startsWith(`"'`)).toBe(true)
    expect(csvCell('plain')).toBe('"plain"')
  })

  it('traceLink keeps a sub-path base', () => {
    expect(traceLink('https://host/nasiko', null, 't')).toBe(
      'https://host/nasiko/observability-session?trace_id=t',
    )
    expect(traceLink('https://host/nasiko/', 's', 't')).toBe(
      'https://host/nasiko/observability-session?session_id=s&trace_id=t',
    )
  })

  it('safeRedirect rejects /login variants but not look-alikes', () => {
    expect(safeRedirect('/login/')).toBe('/')
    expect(safeRedirect('/login#x')).toBe('/')
    expect(safeRedirect('/loginhelp')).toBe('/loginhelp')
  })
})

describe('readEnv', () => {
  it('defaults to mock in dev and live in production', () => {
    expect(readEnv({ DEV: true }, 'https://x').mode).toBe('mock')
    expect(readEnv({ PROD: true }, 'https://x').mode).toBe('live')
  })

  it('a production build ignores mock mode unless explicitly allowed', () => {
    expect(readEnv({ PROD: true, VITE_NASIKO_API_MODE: 'mock' }, 'https://x').mode).toBe('live')
    expect(
      readEnv(
        { PROD: true, VITE_NASIKO_API_MODE: 'mock', VITE_NASIKO_ALLOW_MOCK_BUILD: 'true' },
        'https://x',
      ).mode,
    ).toBe('mock')
    expect(
      readEnv(
        { PROD: true, VITE_NASIKO_API_MODE: 'live', VITE_NASIKO_MOCK: 'top-traces' },
        'https://x',
      ).partialMocks,
    ).toEqual([])
  })

  it('partial mocks in live mode never include auth or unknown keys', () => {
    const cfg = readEnv(
      {
        DEV: true,
        VITE_NASIKO_API_MODE: 'live',
        VITE_NASIKO_MOCK: 'auth, top-traces,agent-hours,bogus',
      },
      'https://x',
    )
    expect(cfg.partialMocks).toEqual(['top-traces'])
  })

  it('legacy UI: dev falls back to localhost:8080, prod to origin; invalid URLs disable links', () => {
    expect(readEnv({ DEV: true }, 'https://x').legacyUiUrl).toBe('http://localhost:8080')
    expect(readEnv({ PROD: true }, 'https://app.example').legacyUiUrl).toBe('https://app.example')
    expect(
      readEnv({ DEV: true, VITE_NASIKO_LEGACY_UI_URL: 'javascript:alert(1)' }, 'https://x')
        .legacyUiUrl,
    ).toBeNull()
  })
})

describe('attribution edge cases', () => {
  const agent = (id: string, cost: number): AgentFinopsRow => ({
    agent_id: id,
    agent_name: id,
    total_cost: cost,
    operations: 1,
    is_capped: false,
    avg_cost_per_operation: cost,
    prompt_tokens: 0,
    completion_tokens: 0,
    cache_read_tokens: 0,
    cache_creation_tokens: 0,
    total_tokens: 0,
    avg_latency_ms: 100,
    avg_latency_p95_ms: 200,
    avg_latency_p99_ms: 300,
    tool_call_count: 0,
    version: null,
    container_hours: 0,
  })
  const wf = (id: string, cost: number): WorkflowFinopsRow => ({
    maf_id: id,
    workflow_name: `wf-${id}`,
    total_cost: cost,
    executions: 2,
    avg_cost_per_execution: cost / 2,
    prompt_tokens: 0,
    completion_tokens: 0,
    cache_read_tokens: 0,
    cache_creation_tokens: 0,
    total_tokens: 0,
    avg_latency_ms: 250,
  })

  it('a previous window from the other view means Δ unavailable', () => {
    const rows = buildAttribution(
      { view: 'agent', rows: [agent('1', 5)] },
      { view: 'workflow', rows: [wf('1', 5)] },
    )
    expect(rows[0].deltaKind).toBe('unavailable')
  })

  it('workflows join by maf_id and carry the server mean latency', () => {
    const rows = buildAttribution(
      { view: 'workflow', rows: [wf('m1', 30)] },
      { view: 'workflow', rows: [wf('m1', 20)] },
    )
    expect(rows[0]).toMatchObject({
      name: 'wf-m1',
      deltaKind: 'change',
      latency: 250,
      operations: 2,
    })
    expect(rows[0].deltaPct).toBeCloseTo(50, 5)
  })

  it('zero totals produce no NaN shares', () => {
    const rows = buildAttribution(
      { view: 'agent', rows: [agent('1', 0), agent('2', 0)] },
      { view: 'agent', rows: [] },
    )
    for (const r of rows) {
      expect(Number.isNaN(r.sharePct)).toBe(false)
      expect(r.deltaKind).toBe('no-spend')
    }
  })
})

describe('forecast boundaries', () => {
  const day = (date: string, spend: number) => ({
    date,
    spend_usd: spend,
    operations: 1,
    intensity: 0,
  })

  it('shows from exactly three elapsed days', () => {
    const days = [day('2026-03-01', 10), day('2026-03-02', 10), day('2026-03-03', 10)]
    expect(summarizeMonth(days, [], new Date('2026-03-03T23:59:00Z')).show).toBe(false)
    expect(summarizeMonth(days, [], new Date('2026-03-04T00:00:00Z')).show).toBe(true)
  })

  it('counts a fraction of the matching day last month, and null when last month had nothing', () => {
    const s = summarizeMonth(
      [day('2026-03-01', 10)],
      [day('2026-02-01', 40), day('2026-02-02', 40)],
      new Date('2026-03-01T12:00:00Z'),
    )
    expect(s.lastMonthSameDays).toBeCloseTo(20, 5)
    expect(
      summarizeMonth([day('2026-03-01', 10)], [], new Date('2026-03-05T00:00:00Z')).vsLastMonthPct,
    ).toBeNull()
  })
})

describe('concentration edge cases', () => {
  const hours = (fill: (h: number) => FinopsDayDrilldown['hours'][number]) =>
    Array.from({ length: 24 }, (_, h) => fill(h))

  it('caps named series at four agents plus Others', () => {
    const names = ['a', 'b', 'c', 'd', 'e', 'f']
    const c = buildConcentration({
      date: '2026-03-01',
      avg_hourly_spend_usd: 1,
      others_spend_usd: 0,
      top_agents: names.map((n) => ({ agent_name: n, spend_usd: 1 })),
      hours: hours((hour) => ({
        hour,
        spend_usd: 6,
        top_agents: names.map((n) => ({ agent_name: n, spend_usd: 1 })),
        others_spend_usd: 0,
      })),
    })
    expect(c.series.map((s) => s.label)).toEqual(['a', 'b', 'c', 'd', 'Others'])
    expect(c.rows[0][OTHERS_KEY]).toBe(2)
  })

  it('a zero-spend day is empty', () => {
    const c = buildConcentration({
      date: '2026-03-01',
      avg_hourly_spend_usd: 0,
      others_spend_usd: 0,
      top_agents: [],
      hours: hours((hour) => ({ hour, spend_usd: 0, top_agents: [], others_spend_usd: 0 })),
    })
    expect(c.isEmpty).toBe(true)
    expect(c.series.map((s) => s.label)).toEqual(['Others'])
  })
})

describe('timeline buckets', () => {
  const point = (iso: string, spend: number) => ({
    bucket_start: iso,
    spend_usd: spend,
    operations: 1,
    tool_calls: 0,
    top_agent_name: null,
    top_agent_spend_usd: null,
    p50_latency_ms: null,
    p95_latency_ms: null,
    p99_latency_ms: null,
  })

  it('zero-fills hourly buckets on UTC hours', () => {
    const pts = zeroFillTimeline(
      [point('2026-03-01T02:00:00Z', 4)],
      'hour',
      new Date('2026-03-01T00:30:00Z'),
      new Date('2026-03-01T04:00:00Z'),
    )
    expect(pts.map((p) => p.iso)).toEqual([
      '2026-03-01T00:00:00.000Z',
      '2026-03-01T01:00:00.000Z',
      '2026-03-01T02:00:00.000Z',
      '2026-03-01T03:00:00.000Z',
    ])
    expect(pts.map((p) => p.spend)).toEqual([0, 0, 4, 0])
  })

  it('never renders more than 800 buckets', () => {
    expect(
      zeroFillTimeline(
        [],
        'hour',
        new Date('2020-01-01T00:00:00Z'),
        new Date('2026-01-01T00:00:00Z'),
      ),
    ).toHaveLength(MAX_BUCKETS)
  })

  it('toTimeline keeps points as returned (no fill)', () => {
    expect(
      toTimeline([point('2026-03-01T00:00:00Z', 1), point('2026-03-05T00:00:00Z', 2)], 'day').map(
        (p) => p.spend,
      ),
    ).toEqual([1, 2])
  })
})

describe('dedupeCatalog (step 0 drift 1: overlapping open pricing rows)', () => {
  const m = (model: string, notes: string, from: string | null, inPerM = 1) => ({
    model,
    input_price_per_1m: inPerM,
    output_price_per_1m: 2,
    cache_creation_price_per_1m: null,
    cache_read_price_per_1m: null,
    currency: 'USD',
    notes,
    effective_from: from,
    effective_until: null,
    pricing_available: true,
  })
  it('keeps one entry per model, the row with the latest effective_from', () => {
    const [g] = dedupeCatalog([
      {
        provider: 'gemini',
        models: [
          m('gemini-2.5-flash', 'boot seed (static list)', '2026-09-16T17:53:14.750837Z', 0.15),
          m('gemini-2.5-pro', 'boot seed (static list)', '2026-09-16T17:53:14.750360Z'),
          m('gemini-2.5-flash', 'Gemini 2.5 Flash', '2026-09-16T17:53:14.661966Z', 0.3),
          m('gemini-2.5-pro', 'Gemini 2.5 Pro', '2026-09-20T00:00:00Z'),
        ],
      },
    ])
    expect(g!.models.map((x) => [x.model, x.notes])).toEqual([
      ['gemini-2.5-flash', 'boot seed (static list)'],
      ['gemini-2.5-pro', 'Gemini 2.5 Pro'],
    ])
  })
  it('keeps the first row on a tie or with no dates, and leaves unique groups untouched', () => {
    const unique = { provider: 'openai', models: [m('gpt-4o', 'a', null)] }
    const [a, b] = dedupeCatalog([
      unique,
      { provider: 'x', models: [m('m', 'first', null), m('m', 'second', null)] },
    ])
    expect(a).toBe(unique)
    expect(b!.models.map((x) => x.notes)).toEqual(['first'])
  })
})

describe('logTicks (F5 log axis)', () => {
  it('ticks 1x and 3x each decade inside the domain', () => {
    expect(logTicks([0.001, 0.05], 1000)).toEqual([0.001, 0.003, 0.01, 0.03])
  })
  it('thins the ticks to the width (about one per 90 px, at least two)', () => {
    expect(logTicks([0.001, 0.05], 180)).toEqual([0.001, 0.01])
    expect(logTicks([0.001, 0.05], 10).length).toBe(2)
  })
})

describe('segmentAt (day panel clicks)', () => {
  // Three series stacked bottom-up in a 200 px plot: a spans 200-150, b is empty, c spans 150-90.
  const series = [{ key: 'a' }, { key: 'b' }, { key: 'c' }]
  const tops = { a: 150, b: 150, c: 90 }
  const row = { a: 2, b: 0, c: 1.5 }
  it('finds the segment under the pointer, skipping empty series', () => {
    expect(segmentAt(series, tops, row, 170)?.key).toBe('a')
    expect(segmentAt(series, tops, row, 150)?.key).toBe('a')
    expect(segmentAt(series, tops, row, 120)?.key).toBe('c')
    expect(segmentAt(series, tops, row, 90)?.key).toBe('c')
  })
  it('finds nothing above the stack', () => {
    expect(segmentAt(series, tops, row, 40)).toBeNull()
  })
})

describe('formatters: missing values and boundaries', () => {
  it('every formatter renders null, undefined and NaN as an em dash', () => {
    for (const f of [fmtMoney, fmtCostPerOp, fmtTokens, fmtInt, fmtLatency, fmtPct, fmtHours]) {
      expect(f(null)).toBe('—')
      expect(f(undefined)).toBe('—')
      expect(f(Number.NaN)).toBe('—')
    }
  })

  it('axis, short and per-op money pick the right shape at each threshold', () => {
    expect(fmtMoneyAxis(12.345)).toBe('$12.35')
    expect(fmtMoneyAxis(-12)).toBe('$-12')
    expect(fmtMoneyAxis(1500)).toBe('$1.5K')
    expect(fmtMoneyShort(0.004)).toBe('<$0.01')
    expect(fmtMoneyShort(2500)).toBe('$2.5K')
    expect(fmtMoneyShort(42.4)).toBe('$42')
    expect(fmtMoneyShort(3.14159)).toBe('$3.14')
    expect(fmtCostPerOp(0)).toBe('$0.00')
    expect(fmtCostPerOp(2.5)).toBe('$2.50')
    expect(fmtCostPerOp(0.012345)).toBe('$0.0123')
  })
})

describe('sortRows: rows without latency or container hours', () => {
  const row = (
    name: string,
    latency: number | null,
    containerHours: number | null,
  ): AttributionRow =>
    ({
      id: name,
      name,
      kind: 'agent',
      cost: 1,
      tokens: 1,
      operations: 1,
      latency,
      containerHours,
    }) as unknown as AttributionRow

  it('sorts missing latency and hours last, ties broken by name', () => {
    const rows = [row('b', null, null), row('a', 100, 2), row('c', 300, null), row('d', null, 5)]
    expect(sortRows(rows, 'latency').map((r) => r.name)).toEqual(['c', 'a', 'b', 'd'])
    expect(sortRows(rows, 'hours').map((r) => r.name)).toEqual(['d', 'a', 'b', 'c'])
  })
})

describe('csv download', () => {
  afterEach(() => {
    vi.restoreAllMocks()
    vi.useRealTimers()
  })

  it('non-finite numbers export as empty cells', () => {
    expect(csvCell(Number.POSITIVE_INFINITY)).toBe('""')
    expect(csvCell(Number.NaN)).toBe('""')
  })

  it('downloads a BOM-prefixed CSV blob under the given name and revokes the URL later', async () => {
    vi.useFakeTimers()
    let blob: Blob | undefined
    const create = vi.fn((b: Blob) => {
      blob = b
      return 'blob:mock'
    })
    const revoke = vi.fn()
    const original = { createObjectURL: URL.createObjectURL, revokeObjectURL: URL.revokeObjectURL }
    Object.assign(URL, { createObjectURL: create, revokeObjectURL: revoke })
    try {
      let clicked: { download: string; href: string } | undefined
      const click = vi.spyOn(HTMLAnchorElement.prototype, 'click').mockImplementation(function (
        this: HTMLAnchorElement,
      ) {
        clicked = { download: this.download, href: this.getAttribute('href') ?? '' }
      })
      downloadCsv('out.csv', '"a","b"')
      expect(click).toHaveBeenCalledTimes(1)
      expect(clicked).toEqual({ download: 'out.csv', href: 'blob:mock' })
      expect(document.querySelector('a[download]')).toBeNull()
      expect(blob?.type).toBe('text/csv;charset=utf-8')
      // Blob.text() decodes (and drops) the BOM, so check the raw bytes.
      expect([...new Uint8Array(await blob!.arrayBuffer())].slice(0, 3)).toEqual([0xef, 0xbb, 0xbf])
      expect(await blob!.text()).toBe('"a","b"')
      expect(revoke).not.toHaveBeenCalled()
      vi.advanceTimersByTime(1000)
      expect(revoke).toHaveBeenCalledWith('blob:mock')
    } finally {
      Object.assign(URL, original)
    }
  })
})

describe('fmtDuration boundaries', () => {
  it('switches units at a minute and an hour', () => {
    expect(fmtDuration(null)).toBe('—')
    expect(fmtDuration(59_999)).toBe(fmtLatency(59_999))
    expect(fmtDuration(60_000)).toBe('1 m 00 s')
    expect(fmtDuration(3_599_999)).toBe('59 m 59 s')
    expect(fmtDuration(3_600_000)).toBe('1 h 00 m')
  })
})

describe('UTC date formatters', () => {
  it('write one en-US UTC shape from a day key, an ISO string, ms or a Date', () => {
    const iso = '2026-03-04T00:05:09Z'
    expect(fmtShortDay('2026-03-04')).toBe('Mar 4')
    expect(fmtShortDay(new Date(iso))).toBe('Mar 4')
    expect(fmtLongDay(Date.parse(iso))).toBe('Wed, Mar 4, 2026')
    expect(fmtMonthYear(iso)).toBe('March 2026')
    expect(fmtUtcHour(iso)).toBe('12 AM')
    // Midnight is 00, not en-US hour12:false's "24".
    expect(fmtUtcTime(iso)).toBe('00:05')
    expect(fmtUtcTime(iso, true)).toBe('00:05:09')
    expect(fmtUtcDayTime(iso)).toBe('Mar 4 00:05')
  })
})

/**
 * A `Savings` block with every field present. The server flattens this into the total, each
 * program, each layer and each agent, so one builder covers all of them.
 */
function measured(over: Partial<Savings> = {}): Savings {
  return {
    saved_tokens: 0,
    saved_input_tokens: 0,
    saved_output_tokens: 0,
    saved_cost_usd: 0,
    actual_tokens: 0,
    actual_cost_usd: 0,
    baseline_tokens: 0,
    baseline_cost_usd: 0,
    token_reduction_pct: null,
    cost_reduction_pct: null,
    basis: 'measured',
    ...over,
  }
}

describe('summarizeOptimisation', () => {
  /** A savings payload with one agent, shaped exactly as the server flattens it. */
  const savings = (over: Partial<SavingsData> = {}): SavingsData => ({
    window: { start: '2026-03-01T00:00:00Z', end: '2026-03-31T00:00:00Z' },
    total: measured({
      saved_tokens: 250,
      saved_input_tokens: 250,
      actual_tokens: 1000,
      baseline_tokens: 1250,
      token_reduction_pct: 20,
      saved_cost_usd: 2.5,
      actual_cost_usd: 47.5,
      baseline_cost_usd: 50,
      cost_reduction_pct: 5,
    }),
    by_program: [
      {
        ...measured({ saved_tokens: 250, saved_cost_usd: 2.5, token_reduction_pct: 20 }),
        program: 'caveman',
        label: 'Payload optimisation',
        layers: [],
      },
    ],
    by_agent: [
      {
        ...measured({ saved_tokens: 250, saved_cost_usd: 2.5, token_reduction_pct: 20 }),
        agent_id: 'a1',
        agent_name: 'Support Bot',
        calls: 12,
        input_tokens_before: 1250,
        input_tokens_after: 1000,
      },
    ],
    by_session: [],
    coverage: {
      calls_in_window: 12,
      calls_with_any_layer_enabled: 12,
      agents_total: 4,
      agents_optimized: 1,
      agents_with_compress_enabled: 1,
      agents_with_minimal_code_enabled: 0,
      agents_with_prompt_comments: 0,
      optimized_spend_usd: 47.5,
      unoptimized_spend_usd: 52.5,
      top_unoptimized: { agent_id: 'a2', agent_name: 'Sales Assistant', spend_usd: 52.5 },
      calibrated_pct: 100,
    },
    ...over,
  })

  it('passes the server percentages through rather than recomputing them', () => {
    // The one arithmetic mistake that would discredit the feature is this panel and another
    // consumer disagreeing about the denominator, so these are never derived here.
    const s = summarizeOptimisation(savings())
    expect(s.savedPct).toBe(20)
    expect(s.costSavedPct).toBe(5)
    expect(s.tokensBefore).toBe(1250)
    expect(s.tokensSaved).toBe(250)
  })

  it('reports coverage and the biggest unoptimised spender', () => {
    const s = summarizeOptimisation(savings())
    expect(s.optimisedCount).toBe(1)
    expect(s.totalAgents).toBe(4)
    expect(s.unoptimisedCount).toBe(3)
    expect(s.unoptimisedSpend).toBe(52.5)
    expect(s.topUnoptimised?.agent_name).toBe('Sales Assistant')
  })

  it('keeps a zero category so it can explain itself', () => {
    // An absent row reads as "this feature does nothing"; a zero row with a reason is actionable.
    const s = summarizeOptimisation(
      savings({
        by_program: [
          {
            ...measured({ basis: 'seed_default' }),
            program: 'ponytail',
            label: 'Minimal code',
            note: 'No agent has minimal code enabled with a coding agent card.',
            layers: [],
          },
        ],
      }),
    )
    expect(s.categories).toHaveLength(1)
    expect(s.categories[0].savedTokens).toBe(0)
    expect(s.categories[0].note).toContain('minimal code')
    expect(s.categories[0].basis).toBe('seed_default')
  })

  it('treats a null percentage as unknown rather than zero', () => {
    const s = summarizeOptimisation(
      savings({ total: measured({ token_reduction_pct: null, cost_reduction_pct: null }) }),
    )
    expect(s.savedPct).toBeNull()
    expect(s.costSavedPct).toBeNull()
  })

  it('never divides by zero', () => {
    const s = summarizeOptimisation(
      savings({
        by_agent: [],
        coverage: { ...savings().coverage, optimized_spend_usd: 0, unoptimized_spend_usd: 0 },
      }),
    )
    expect([s.optimisedSharePct, s.unoptimisedSharePct]).toEqual([0, 0])
    expect(s.rows).toEqual([])
  })

  it('flags a fleet with nothing switched on as unconfigured', () => {
    const empty = savings({
      total: measured({}),
      by_agent: [],
      coverage: { ...savings().coverage, agents_optimized: 0 },
    })
    expect(isUnconfigured(summarizeOptimisation(empty))).toBe(true)
    expect(isUnconfigured(summarizeOptimisation(savings()))).toBe(false)
  })
})
