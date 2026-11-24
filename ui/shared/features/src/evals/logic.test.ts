import { describe, expect, it } from 'vitest'
import { comparison, defaultPicks } from './logic'
import type { EvalDataset, EvalResult } from './types'

const r = (
  task_id: string,
  model_id: string,
  score: number | null,
  cost_usd: number | null = 0.01,
  latency_ms: number | null = 1000,
  status: EvalResult['status'] = 'done',
): EvalResult => ({
  task_id,
  model_id,
  attempt: 1,
  status,
  score,
  resolved: score === 100,
  cost_usd,
  latency_ms,
  failure: null,
  builder: false,
})

describe('comparison', () => {
  it('marks the best model per task, ties included, and never a missing cell', () => {
    const { rows } = comparison(
      ['t1', 't2'],
      ['a', 'b'],
      [r('t1', 'a', 90), r('t1', 'b', 70), r('t2', 'a', 80), r('t2', 'b', 80)],
    )
    const best = (i: number) => rows[i]!.cells.map((c) => (c?.kind === 'graded' ? c.best : null))
    expect(best(0)).toEqual([true, false])
    expect(best(1)).toEqual([true, true])
    const gap = comparison(['t1'], ['a', 'b'], [r('t1', 'b', 10)])
    expect(gap.rows[0]!.cells[0]).toBeNull()
    expect(gap.rows[0]!.cells[1]).toMatchObject({ kind: 'graded', best: true })
  })

  it('shows attempts still going and those that never got a grade', () => {
    const { rows, footer } = comparison(
      ['t1'],
      ['a', 'b', 'c'],
      [
        r('t1', 'a', null, null, null, 'running'),
        r('t1', 'b', null, null, null, 'failed'),
        r('t1', 'c', 40),
        { ...r('t1', 'c', 90), attempt: 2 },
      ],
    )
    expect(rows[0]!.cells.map((c) => c?.kind)).toEqual(['waiting', 'failed', 'graded'])
    expect(rows[0]!.cells[2]).toMatchObject({ score: 40, best: true })
    expect(footer).toEqual([null, null, { score: 40, cost: 0.01, latency: 1000 }])
  })

  it('sums each model: mean score, known costs, median latency', () => {
    const { footer } = comparison(
      ['t1', 't2', 't3'],
      ['a', 'b'],
      [
        r('t1', 'a', 90, 0.1, 100),
        r('t2', 'a', 60, 0.2, 300),
        r('t3', 'a', 30, 0.3, 200),
        r('t1', 'b', 50, null, null),
      ],
    )
    expect(footer[0]).toEqual({ score: 60, cost: expect.closeTo(0.6), latency: 200 })
    expect(footer[1]).toEqual({ score: 50, cost: null, latency: null })
  })

  it('has no figures for a model without results', () => {
    expect(comparison(['t1'], ['a'], []).footer[0]).toBeNull()
  })
})

describe('defaultPicks', () => {
  it('ticks the verified tasks and the models the sessions used', () => {
    const task = (id: string, status: string) => ({
      id,
      session_id: 's',
      repo: 'o/r',
      title: id,
      status,
      stage: 'done',
      reject_reason: null,
      original: { model: 'm', cost_usd: null, latency_ms: null },
    })
    const dataset = {
      id: 'd',
      status: 'ready',
      progress: { done: 2, total: 2 },
      outcomes: {},
      error: null,
      tasks: [task('t1', 'verified'), task('t2', 'rejected')],
      models: [
        { id: 'm1', name: 'M1', provider: 'p', model: 'm1', used: true },
        { id: 'm2', name: 'M2', provider: 'p', model: 'm2', used: false },
      ],
    } as EvalDataset
    expect(defaultPicks(dataset)).toEqual({ tasks: ['t1'], models: ['m1'] })
  })
})
