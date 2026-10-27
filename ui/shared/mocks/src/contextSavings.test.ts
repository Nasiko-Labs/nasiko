/**
 * The proposed context-savings reads (plans/feat-optimization-page.md eng E1, C2–C4): pure rules here, plus the
 * handlers through MSW for today's server, the viewer's redaction and the CX-H settings history.
 */
import { afterEach, describe, expect, it } from 'vitest'
import { configureMocks } from './handlers'
import {
  contextSavings,
  contextSavingsAgents,
  recordedSince,
  reportOf,
  topRequests,
  RECORDED_DAYS,
} from './contextSavings'
import { generateSeed } from './seed'
import { now as pinnedNow, seed as pinnedSeed, setupPinnedSeed } from '@nasiko/testing/pinnedSeed'

const NOW = Date.parse('2026-10-02T12:00:00Z')
const seed = generateSeed({ anchor: new Date(NOW) })
const opts = { now: NOW, compressOf: (id: string) => id.endsWith('1') }
const err = (fn: () => unknown) => {
  try {
    fn()
  } catch (e) {
    return e as { status: number; message: string }
  }
  throw new Error('expected an error')
}

describe('context savings (CX-V3, V3a)', () => {
  it('reports one per request, from the recording start, with coverage below 100% (C2, C3)', () => {
    const s = contextSavings(seed, { range: '30d' }, opts)
    expect(s.eligible_requests).toBeGreaterThan(s.reports)
    expect(s.reports).toBeGreaterThan(0)
    expect(s.pool_tokens).toBeGreaterThan(s.sent_tokens)
    expect(Date.parse(s.recorded_since)).toBe(recordedSince(NOW))
    // Nothing reported before recording began.
    const before = seed.traces.filter((t) => t.ts < recordedSince(NOW))
    expect(before.every((t) => !reportOf(t, NOW, false).reported)).toBe(true)
  })

  it('prices only reports with a known input price, once per request (C3)', () => {
    const s = contextSavings(seed, { range: '7d' }, opts)
    expect(s.priced_reports).toBeLessThanOrEqual(s.reports)
    const t = seed.traces.find((x) => x.prompt_cost_usd > 0 && reportOf(x, NOW, false).reported)!
    const r = reportOf(t, NOW, false)
    expect(r.price).toBeCloseTo(t.prompt_cost_usd / t.input_tokens)
    expect(r.sent).toBe(t.input_tokens)
  })

  it('compressed bytes come only from agents with Token optimization on', () => {
    const t = seed.traces.find((x) => reportOf(x, NOW, true).reported)!
    expect(reportOf(t, NOW, false).compressed).toBe(0)
    expect(reportOf(t, NOW, true).compressed).toBeGreaterThan(0)
  })

  it('series: daily buckets that sum to the totals; 24h is hourly (E1)', () => {
    const s = contextSavings(seed, { range: '30d', series: 'daily' }, opts)
    if (!('series' in s)) throw new Error('no series')
    expect(s.series.bucket).toBe('day')
    expect(s.series.points.reduce((a, p) => a + p.reports, 0)).toBe(s.reports)
    expect(s.series.points.reduce((a, p) => a + p.sent_tokens, 0)).toBe(s.sent_tokens)
    // Days before recording began are present, with no eligible requests and no reports (drawn "not recorded").
    const early = s.series.points.filter((p) => Date.parse(p.bucket_start) < recordedSince(NOW))
    expect(early.length).toBeGreaterThan(0)
    expect(early.every((p) => p.eligible_requests === 0 && p.reports === 0)).toBe(true)
    const h = contextSavings(seed, { range: '24h', series: 'hourly' }, opts)
    if (!('series' in h)) throw new Error('no series')
    expect(h.series.bucket).toBe('hour')
    expect(err(() => contextSavings(seed, { series: 'weekly' }, opts)).status).toBe(400)
  })

  it(`reports start ${RECORDED_DAYS} days back`, () => {
    expect(NOW - recordedSince(NOW)).toBeGreaterThan(RECORDED_DAYS * 86_400_000 - 86_400_000)
  })
})

describe('per agent (CX-V3b)', () => {
  it('sorts by history volume and carries each agent’s switch', () => {
    const rows = contextSavingsAgents(
      seed,
      { range: '30d' },
      { ...opts, nameOf: (_id, raw) => raw },
    )
    expect(rows.length).toBeGreaterThan(1)
    for (let i = 1; i < rows.length; i++)
      expect(rows[i - 1]!.pool_tokens).toBeGreaterThanOrEqual(rows[i]!.pool_tokens)
    for (const r of rows) expect(r.compress_enabled).toBe(r.agent_id.endsWith('1'))
  })
})

describe('top requests (CX-V3c)', () => {
  const base = {
    ...opts,
    sessionOf: (traceId: string) => ({ id: `s-${traceId}`, title: 'A chat' }),
    ownerOf: (sid: string) => (sid.endsWith('0') ? 'x' : 'me'),
  }

  it('a member sees only their own chats in full; another user’s row has no title, session or trace (C4)', () => {
    const rows = topRequests(
      seed,
      { range: '30d', limit: '50' },
      { ...base, viewer: { id: 'me', superuser: false } },
    )
    const other = rows.find((r) => !r.own)
    const mine = rows.find((r) => r.own)
    expect(other).toMatchObject({ chat_title: null, session_id: null, trace_id: null })
    expect(mine?.chat_title).toBe('A chat')
    for (let i = 1; i < rows.length; i++)
      expect(rows[i - 1]!.pool_tokens).toBeGreaterThanOrEqual(rows[i]!.pool_tokens)
  })

  it('a superuser sees every chat', () => {
    const rows = topRequests(
      seed,
      { range: '30d', limit: '50' },
      { ...base, viewer: { id: 'me', superuser: true } },
    )
    expect(rows.every((r) => r.own && r.chat_title === 'A chat')).toBe(true)
  })

  it('filters by a bar’s interval (from/to) and validates its params (C5)', () => {
    const day = new Date(NOW - 3 * 86_400_000)
    day.setUTCHours(0, 0, 0, 0)
    const from = day.toISOString()
    const to = new Date(day.getTime() + 86_400_000).toISOString()
    const rows = topRequests(
      seed,
      { range: '30d', from, to },
      { ...base, viewer: { id: 'me', superuser: true } },
    )
    for (const r of rows) {
      const t = Date.parse(r.started_at)
      expect(t).toBeGreaterThanOrEqual(Date.parse(from))
      expect(t).toBeLessThan(Date.parse(to))
    }
    expect(
      err(() =>
        topRequests(seed, { limit: '51' }, { ...base, viewer: { id: 'me', superuser: true } }),
      ).status,
    ).toBe(400)
    expect(
      err(() =>
        topRequests(
          seed,
          { from: 'yesterday' },
          { ...base, viewer: { id: 'me', superuser: true } },
        ),
      ).status,
    ).toBe(400)
  })
})

describe('the handlers (MSW)', () => {
  setupPinnedSeed()
  afterEach(() =>
    configureMocks({
      seed: pinnedSeed,
      now: pinnedNow,
      loggedIn: true,
      variant: null,
      superuser: null,
    }),
  )
  const get = async (path: string) => {
    const r = await fetch(new URL(path, globalThis.location.origin))
    return { status: r.status, body: r.status === 200 ? ((await r.json()) as unknown) : null }
  }

  it('answers all three reads in the finops envelope', async () => {
    const s = await get('/api/observability/finops/context-savings?range=7d&series=daily')
    expect((s.body as { data: { series: { bucket: string } } }).data.series.bucket).toBe('day')
    const a = await get('/api/observability/finops/context-savings/agents?range=7d')
    expect(Array.isArray((a.body as { data: unknown[] }).data)).toBe(true)
    const t = await get('/api/observability/finops/context-savings/top-requests?range=7d')
    expect(Array.isArray((t.body as { data: unknown[] }).data)).toBe(true)
  })

  it('redacts other users’ chats for a member, not for a superuser (C4)', async () => {
    configureMocks({ superuser: false })
    const m = (
      await get('/api/observability/finops/context-savings/top-requests?range=30d&limit=50')
    ).body as {
      data: { own: boolean; chat_title: string | null }[]
    }
    expect(m.data.some((r) => !r.own && r.chat_title === null)).toBe(true)
    configureMocks({ superuser: true })
    const s = (
      await get('/api/observability/finops/context-savings/top-requests?range=30d&limit=50')
    ).body as {
      data: { own: boolean }[]
    }
    expect(s.data.every((r) => r.own)).toBe(true)
  })

  it("today's server has none of them (bare 404s), nor the settings history", async () => {
    configureMocks({ variant: 'optimization-classic' })
    for (const p of ['', '/agents', '/top-requests'])
      expect((await get(`/api/observability/finops/context-savings${p}?range=7d`)).status).toBe(404)
    expect((await get('/api/me/settings-history?range=30d')).status).toBe(404)
  })

  it('records setting changes in the history, newest first (CX-H)', async () => {
    const before = (await get('/api/me/settings-history?range=30d')).body as {
      field: string
      to: string
    }[]
    expect(before).toEqual([expect.objectContaining({ field: 'strategy', to: 'pacms' })])
    await fetch(new URL('/api/me/pacms-budget', globalThis.location.origin), {
      method: 'PATCH',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ level: 'high' }),
    })
    const after = (await get('/api/me/settings-history?range=30d')).body as {
      field: string
      from: string
      to: string
    }[]
    expect(after[0]).toMatchObject({ field: 'level', from: 'medium', to: 'high' })
    expect(after).toHaveLength(2)
  })
})
