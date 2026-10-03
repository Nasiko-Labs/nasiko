import { afterEach, describe, expect, it } from 'vitest'
import { pinClock } from './anchor'

const RealDate = Date

afterEach(() => {
  globalThis.Date = RealDate
})

describe('pinClock', () => {
  it('shifts now, new Date() and Date() without new; explicit dates and statics are untouched', () => {
    const anchor = new RealDate('2030-01-01T15:00:00.000Z')
    pinClock(anchor)
    const slack = 5_000
    expect(Math.abs(Date.now() - anchor.getTime())).toBeLessThan(slack)
    expect(Math.abs(new Date().getTime() - anchor.getTime())).toBeLessThan(slack)
    expect(new Date() instanceof Date).toBe(true)
    // Called without `new`, Date returns a string (a bare class would throw here).
    const asString = (Date as unknown as () => string)()
    expect(typeof asString).toBe('string')
    expect(Math.abs(RealDate.parse(asString) - anchor.getTime())).toBeLessThan(slack)

    expect(new Date('2020-05-06T07:08:09.000Z').toISOString()).toBe('2020-05-06T07:08:09.000Z')
    expect(new Date(0).getTime()).toBe(0)
    expect(new Date(2026, 0, 2).getFullYear()).toBe(2026)
    expect(Date.UTC(2026, 0, 1)).toBe(RealDate.UTC(2026, 0, 1))
    expect(Date.parse('2026-01-01T00:00:00Z')).toBe(RealDate.parse('2026-01-01T00:00:00Z'))
  })
})
