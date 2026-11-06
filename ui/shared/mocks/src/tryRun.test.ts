/**
 * The `try-run` workspace through MSW: no activity until the try-a-run card's runs, each then a trace in the spend
 * reads, and the optimized run's difference reported as savings.
 */
import { afterEach, beforeEach, describe, expect, it } from 'vitest'
import { writeDone } from '@nasiko/features/overview/tryRun'
import { now, seed, setupPinnedSeed } from '@nasiko/testing/pinnedSeed'
import { configureMocks } from './handlers'
import { TRY_RUN_FIGURES } from './tryRun'

setupPinnedSeed()
beforeEach(() => configureMocks({ seed, now, loggedIn: true, variant: 'try-run' }))
afterEach(() => configureMocks({ seed, now, loggedIn: true, variant: null }))

const get = async <T>(path: string): Promise<T> => {
  const r = await fetch(new URL(path, globalThis.location.origin))
  expect(r.status, path).toBe(200)
  const body = (await r.json()) as { data?: T }
  return (body.data ?? body) as T
}
const sub = async () => (await get<{ sub: string }>('/api/me')).sub
const spend = async () => {
  const ts = await get<{ points: { spend_usd: number; operations: number }[] }>(
    '/api/observability/finops/spend-timeseries?range=30d',
  )
  return {
    spend: Math.round(ts.points.reduce((n, p) => n + p.spend_usd, 0) * 100) / 100,
    runs: ts.points.reduce((n, p) => n + p.operations, 0),
  }
}
const saved = async () =>
  (
    await get<{ total: { saved_tokens: number; saved_cost_usd: number } }>(
      '/api/observability/finops/savings?scope=agent',
    )
  ).total

describe('try-run workspace', () => {
  it('answers the reads Home makes, with no chats or requests yet', async () => {
    expect(await get<unknown[]>('/api/hitl/pending')).toEqual([])
  })

  it('has no activity before a run', async () => {
    expect(await spend()).toEqual({ spend: 0, runs: 0 })
    expect((await saved()).saved_tokens).toBe(0)
  })

  it('records the baseline run, then the optimized one with its savings', async () => {
    const id = await sub()
    writeDone(id, 'baseline')
    expect(await spend()).toEqual({ spend: TRY_RUN_FIGURES.baseline.cost, runs: 1 })
    expect((await saved()).saved_tokens).toBe(0)

    writeDone(id, 'optimized')
    const { baseline: b, optimized: o } = TRY_RUN_FIGURES
    expect(await spend()).toEqual({ spend: b.cost + o.cost, runs: 2 })
    expect(await saved()).toMatchObject({
      saved_tokens: b.input + b.output - o.input - o.output,
      saved_cost_usd: Math.round((b.cost - o.cost) * 100) / 100,
    })
  })
})
