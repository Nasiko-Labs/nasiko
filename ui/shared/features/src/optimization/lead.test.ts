/**
 * The lead's pure rules (plans/feat-optimization-page.md R1A, R2A, R2E, R4B; eng C1, C2, C3, C5, C6).
 */
import { describe, expect, it } from 'vitest'
import {
  coverageOf,
  leadState,
  pctOf,
  sliceLabel,
  tokensShort,
  trendOf,
  trendRows,
  weeklyPoints,
} from './lead'
import { parseSlice, sliceOf } from '@nasiko/app-core/slice'
import type { ContextSavings, SavingsPoint } from './types'

const savings = (over: Partial<ContextSavings> = {}): ContextSavings => ({
  reports: 900,
  eligible_requests: 1000,
  priced_reports: 900,
  pool_tokens: 1_000_000,
  sent_tokens: 380_000,
  pool_cost_usd: 10,
  sent_cost_usd: 3.8,
  messages_dropped: 100,
  compressed_bytes: 0,
  recorded_since: '2026-09-18T00:00:00.000Z',
  ...over,
})
const ready = (s: ContextSavings) => ({ kind: 'ready' as const, data: s })

describe('leadState (C1, C2, R2B, R2E)', () => {
  it('loads, fails, and says the server reason before any access state', () => {
    expect(leadState({ result: undefined, failed: false, agentCount: null }).kind).toBe('loading')
    expect(leadState({ result: undefined, failed: true, agentCount: 3 }).kind).toBe('error')
    expect(leadState({ result: { kind: 'absent' }, failed: false, agentCount: 0 }).kind).toBe(
      'absent',
    )
  })

  it('no accessible agents, then no reports', () => {
    expect(leadState({ result: ready(savings()), failed: false, agentCount: 0 }).kind).toBe(
      'no-agents',
    )
    expect(
      leadState({ result: ready(savings({ reports: 0 })), failed: false, agentCount: 2 }).kind,
    ).toBe('no-reports')
  })

  it('states no percentage under 50% coverage, and does at exactly 50%', () => {
    const low = leadState({
      result: ready(savings({ reports: 499, eligible_requests: 1000 })),
      failed: false,
      agentCount: 2,
    })
    expect(low.kind).toBe('low-coverage')
    const half = leadState({
      result: ready(savings({ reports: 500, eligible_requests: 1000 })),
      failed: false,
      agentCount: 2,
    })
    expect(half).toMatchObject({ kind: 'saving' })
    if (half.kind === 'saving') expect(pctOf(half.share)).toBe(62)
  })

  it('reads the heading from reports, never from switches: a server without coverage still answers', () => {
    const s = leadState({
      result: ready(savings({ eligible_requests: undefined })),
      failed: false,
      agentCount: null,
    })
    expect(s).toMatchObject({ kind: 'saving', coverage: null })
  })
})

describe('numbers', () => {
  it('coverage is reports over eligible, capped at 100%', () => {
    expect(coverageOf({ reports: 3, eligible_requests: 4 })).toMatchObject({
      share: 0.75,
      enough: true,
    })
    expect(coverageOf({ reports: 5, eligible_requests: 4 })?.share).toBe(1)
    expect(coverageOf({ reports: 0, eligible_requests: 0 })).toMatchObject({ enough: false })
  })

  it('token figures round before the unit is picked', () => {
    expect(tokensShort(63_512_345)).toBe('63.5M')
    expect(tokensShort(165_020_000)).toBe('165M')
    expect(tokensShort(999_600)).toBe('1M')
    expect(tokensShort(420_123)).toBe('420k')
    expect(tokensShort(9_412)).toBe('9,410')
    expect(tokensShort(42)).toBe('42')
    // review (adversarial): 950k–999k stay in k; tenths without float artefacts
    expect(tokensShort(950_000)).toBe('950k')
    expect(tokensShort(954_999)).toBe('955k')
    expect(tokensShort(1_450_000)).toBe('1.5M')
  })

  it('a real saving never reads 0%, a partial one never 100%', () => {
    expect(pctOf(0.001)).toBe(1)
    expect(pctOf(0.999)).toBe(99)
    expect(pctOf(0)).toBe(0)
    expect(pctOf(1)).toBe(100)
  })
})

const day = (i: number, sharePct: number, over: Partial<SavingsPoint> = {}): SavingsPoint => ({
  bucket_start: new Date(Date.UTC(2026, 8, 1 + i)).toISOString(),
  eligible_requests: 10,
  reports: 10,
  pool_tokens: 1000,
  sent_tokens: 1000 - sharePct * 10,
  ...over,
})

describe('trendOf (C6)', () => {
  const series = (points: SavingsPoint[]) => ({ bucket: 'day' as const, points })

  it('needs 14 recorded days', () => {
    const p = [
      ...Array.from({ length: 7 }, (_, i) => day(i, 40)),
      ...Array.from({ length: 6 }, (_, i) => day(7 + i, 60)),
    ]
    expect(trendOf(series(p))).toBeNull()
    p.push(day(13, 60))
    expect(trendOf(series(p))).toMatchObject({ direction: 'up' })
  })

  it('compares the savings share, so more traffic alone is no trend', () => {
    const p = [
      ...Array.from({ length: 7 }, (_, i) => day(i, 50)),
      ...Array.from({ length: 7 }, (_, i) =>
        day(7 + i, 50, { pool_tokens: 5000, sent_tokens: 2500 }),
      ),
    ]
    expect(trendOf(series(p))).toBeNull()
  })

  it('±10% is the edge: 10% relative is no trend, beyond it is', () => {
    const at = (last: number) => [
      ...Array.from({ length: 7 }, (_, i) => day(i, 50)),
      ...Array.from({ length: 7 }, (_, i) => day(7 + i, last)),
    ]
    expect(trendOf(series(at(55)))).toBeNull()
    expect(trendOf(series(at(45)))).toBeNull()
    expect(trendOf(series(at(56)))).toMatchObject({ direction: 'up', from: 0.5, to: 0.56 })
    expect(trendOf(series(at(44)))).toMatchObject({ direction: 'down' })
  })

  it('skips days under 50% coverage and hourly series', () => {
    const p = [
      ...Array.from({ length: 7 }, (_, i) => day(i, 40)),
      ...Array.from({ length: 7 }, (_, i) => day(7 + i, 60, { reports: 4 })),
    ]
    expect(trendOf(series(p))).toBeNull()
    expect(trendOf({ bucket: 'hour', points: p })).toBeNull()
    expect(trendOf(undefined)).toBeNull()
  })
})

describe('trendRows (R5A, C2)', () => {
  it('stacks sent + saved, hatches unrecorded buckets at the tallest recorded height', () => {
    const rows = trendRows([
      day(2, 50, { pool_tokens: 3000, sent_tokens: 1000 }),
      day(0, 0, { reports: 0 }),
      day(1, 0, { reports: 3 }),
    ])
    expect(rows.map((r) => r.iso.slice(8, 10))).toEqual(['01', '02', '03'])
    expect(rows[0]).toMatchObject({ recorded: false, sent: 0, saved: 0, notRecorded: 3000 })
    expect(rows[1]).toMatchObject({ recorded: false, notRecorded: 3000, reports: 3 })
    expect(rows[2]).toMatchObject({ recorded: true, sent: 1000, saved: 2000, notRecorded: 0 })
  })
})

describe('slices (C5)', () => {
  const win = { start: new Date('2026-09-02T12:00:00Z'), end: new Date('2026-10-02T12:00:00Z') }

  it('round-trips a bar interval for each bucket size', () => {
    for (const size of ['hour', 'day', 'week'] as const) {
      const s = sliceOf('2026-09-29T00:00:00.000Z', size)
      expect(parseSlice(s, win)?.size).toBe(size)
    }
    expect(sliceLabel(parseSlice(sliceOf('2026-09-29T00:00:00Z', 'day'), win)!)).toBe('Sep 29')
    expect(sliceLabel(parseSlice(sliceOf('2026-09-29T14:00:00Z', 'hour'), win)!)).toBe(
      'Sep 29, 14:00 UTC',
    )
    expect(sliceLabel(parseSlice(sliceOf('2026-09-23T00:00:00Z', 'week'), win)!)).toBe(
      'Sep 23 – Sep 29',
    )
  })

  it('ignores a slice outside the window, of another length, or malformed', () => {
    expect(parseSlice(sliceOf('2026-08-01T00:00:00Z', 'day'), win)).toBeNull()
    expect(parseSlice('2026-09-29T00:00:00Z/2026-09-29T05:00:00Z', win)).toBeNull()
    expect(parseSlice('yesterday/today', win)).toBeNull()
    expect(parseSlice('a/b/c', win)).toBeNull()
    expect(parseSlice(undefined, win)).toBeNull()
  })
})

describe('weeklyPoints (R6B)', () => {
  it('counts weeks back from the newest day, sums each, and starts each week 6 days before its newest day', () => {
    const days = Array.from({ length: 30 }, (_, i) => day(i, 50))
    const weeks = weeklyPoints(days)
    expect(weeks).toHaveLength(5)
    expect(weeks.at(-1)!.bucket_start).toBe(days[23]!.bucket_start)
    expect(weeks.at(-1)!.pool_tokens).toBe(7000)
    // The first week is the short one (2 days), still a 7-day slice.
    expect(weeks[0]!.pool_tokens).toBe(2000)
    expect(weeks.reduce((a, w) => a + w.reports, 0)).toBe(days.reduce((a, d) => a + d.reports, 0))
  })

  it('a week is recorded by its own coverage (C2)', () => {
    const days = [
      ...Array.from({ length: 4 }, (_, i) => day(i, 50, { reports: 0 })),
      ...Array.from({ length: 3 }, (_, i) => day(4 + i, 50)),
    ]
    expect(trendRows(weeklyPoints(days))[0]).toMatchObject({ recorded: false })
  })
})

describe('bucket cap (review: Codex)', () => {
  it('keeps at most TokenOps’ cap of buckets, newest first kept', () => {
    const many = Array.from({ length: 1000 }, (_, i) => day(i, 50))
    const rows = trendRows(many)
    expect(rows).toHaveLength(800)
    expect(rows.at(-1)!.iso).toBe(many.at(-1)!.bucket_start)
  })
})
