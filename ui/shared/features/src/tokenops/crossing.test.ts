/** X5's crossing date (plans/feat-agent-centric.md X5; eng test plan "crossingDate on summarizeMonth band"). */
import { describe, expect, it } from 'vitest'
import { crossingDate } from './forecast'

const day = (date: string, spend: number) => ({
  date,
  spend_usd: spend,
  operations: 1,
  intensity: 0,
})
/** $10 a day from the 1st through `through` (day of March 2026). */
const steady = (through: number, perDay = 10) =>
  Array.from({ length: through }, (_, i) =>
    day(`2026-03-${String(i + 1).padStart(2, '0')}`, perDay),
  )

describe('crossingDate', () => {
  it('gives the first UTC date the running total reaches the limit, inside the month', () => {
    // 10 days at $10 = $100 by Mar 11 00:00; $50 more at $10/day = 5 days → Mar 16.
    expect(crossingDate(150, steady(10), new Date('2026-03-11T00:00:00Z'))).toEqual({
      kind: 'crosses',
      date: '2026-03-16',
    })
  })

  it('says nothing when the date falls after month end', () => {
    expect(crossingDate(1_000, steady(10), new Date('2026-03-11T00:00:00Z'))).toBeNull()
  })

  it('says nothing before three days of data', () => {
    expect(crossingDate(25, steady(2), new Date('2026-03-02T12:00:00Z'))).toBeNull()
  })

  it('says the limit is reached when month-to-date already meets it', () => {
    expect(crossingDate(100, steady(10), new Date('2026-03-11T00:00:00Z'))).toEqual({
      kind: 'reached',
    })
    expect(crossingDate(80, steady(10), new Date('2026-03-11T00:00:00Z'))).toEqual({
      kind: 'reached',
    })
  })

  it('paces on the last seven completed days, not the whole month', () => {
    // A quiet start then $20/day: Mar 1–3 at $0, Mar 4–10 at $20 = $140; $60 more at $20/day = 3 days → Mar 14.
    const days = [...steady(3, 0), ...steady(10, 20).slice(3)]
    expect(crossingDate(200, days, new Date('2026-03-11T00:00:00Z'))).toEqual({
      kind: 'crosses',
      date: '2026-03-14',
    })
  })

  it('says nothing with no limit or no spend', () => {
    expect(crossingDate(0, steady(10), new Date('2026-03-11T00:00:00Z'))).toBeNull()
    expect(crossingDate(100, [], new Date('2026-03-11T00:00:00Z'))).toBeNull()
  })
})

describe('crossingDate edges (review)', () => {
  it('a day missing inside the pace window counts as $0', () => {
    // Mar 1–10 at $20 except Mar 8–10 missing: $140 by Mar 11; pace = (20*4 + 0*3)/7 ≈ 11.43 → $60 more ≈ 5.25 days.
    const days = steady(7, 20)
    expect(crossingDate(200, days, new Date('2026-03-11T00:00:00Z'))).toEqual({
      kind: 'crosses',
      date: '2026-03-16',
    })
  })

  it('compares whole cents: 0.1 + 0.7 meets a $0.80 limit', () => {
    const days = [day('2026-03-01', 0.1), day('2026-03-02', 0.7)]
    expect(crossingDate(0.8, days, new Date('2026-03-02T12:00:00Z'))).toEqual({ kind: 'reached' })
  })

  it('a crossing on the month’s last day is shown; one at next month’s midnight is not', () => {
    // 30 days at $10 = $300 by Mar 31 00:00; $5 more at $10/day = Mar 31 12:00.
    expect(crossingDate(305, steady(30), new Date('2026-03-31T00:00:00Z'))).toEqual({
      kind: 'crosses',
      date: '2026-03-31',
    })
    // $10 more lands exactly at Apr 1 00:00: after month end.
    expect(crossingDate(310, steady(30), new Date('2026-03-31T00:00:00Z'))).toBeNull()
  })
})
