import { describe, expect, it } from 'vitest'
import { decimateTimeSeries } from './decimate-time-series'

const series = (n: number, spikeAt: number) =>
  Array.from({ length: n }, (_, i) => ({ x: i, flat: 1, spiky: i === spikeAt ? 1000 : 1 }))

describe('decimateTimeSeries (per-series LTTB)', () => {
  it('returns short data untouched', () => {
    const d = series(10, 3)
    expect(decimateTimeSeries(d, 50, ['flat'])).toBe(d)
  })

  it('keeps first and last and stays near the budget for one series', () => {
    const out = decimateTimeSeries(series(5000, 1234), 100, ['spiky'])
    expect(out[0]!.x).toBe(0)
    expect(out.at(-1)!.x).toBe(4999)
    expect(out.length).toBeLessThanOrEqual(100)
  })

  it('opposite spikes in two series both survive (their mean is flat, which hid them)', () => {
    const d = Array.from({ length: 5000 }, (_, i) => ({
      x: i,
      up: i === 2345 ? 1000 : 1,
      down: i === 2345 ? -998 : 1,
    }))
    const out = decimateTimeSeries(d, 64, ['up', 'down'])
    expect(out.some((p) => p.up === 1000 && p.down === -998)).toBe(true)
    // union of two passes: never more than twice the budget, always sorted by x
    expect(out.length).toBeLessThanOrEqual(128)
    expect(out.map((p) => p.x)).toEqual([...out.map((p) => p.x)].sort((a, b) => a - b))
  })
})
