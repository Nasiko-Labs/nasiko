import { describe, expect, it } from 'vitest'
import { comparison } from './logic'
import type { EvalResult } from './types'

const r = (
  task_id: string,
  model_id: string,
  score: number,
  cost_usd = 0.01,
  latency_ms = 1000,
): EvalResult => ({
  task_id,
  model_id,
  score,
  cost_usd,
  latency_ms,
})

describe('comparison', () => {
  it('marks the best model per task, ties included, and never a missing cell', () => {
    const { rows } = comparison(
      ['t1', 't2'],
      ['a', 'b'],
      [r('t1', 'a', 90), r('t1', 'b', 70), r('t2', 'a', 80), r('t2', 'b', 80)],
    )
    expect(rows[0]!.cells.map((c) => c?.best)).toEqual([true, false])
    expect(rows[1]!.cells.map((c) => c?.best)).toEqual([true, true])
    const gap = comparison(['t1'], ['a', 'b'], [r('t1', 'b', 10)])
    expect(gap.rows[0]!.cells[0]).toBeNull()
    expect(gap.rows[0]!.cells[1]?.best).toBe(true)
  })

  it('sums each model: mean score, total cost, median latency', () => {
    const { footer } = comparison(
      ['t1', 't2', 't3'],
      ['a', 'b'],
      [
        r('t1', 'a', 90, 0.1, 100),
        r('t2', 'a', 60, 0.2, 300),
        r('t3', 'a', 30, 0.3, 200),
        r('t1', 'b', 50, 1, 1000),
      ],
    )
    expect(footer[0]).toEqual({ score: 60, cost: expect.closeTo(0.6), latency: 200 })
    expect(footer[1]).toEqual({ score: 50, cost: 1, latency: 1000 })
  })

  it('has no figures for a model without results', () => {
    expect(comparison(['t1'], ['a'], []).footer[0]).toBeNull()
  })
})
