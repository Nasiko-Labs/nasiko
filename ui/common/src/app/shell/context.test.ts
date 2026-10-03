/**
 * The URL contract (context.ts table): junk params fall back, `?compare=0` arrives as a
 * number, `open` normalises to a canonical CSV, and only shared keys cross pages.
 */
import { describe, expect, it } from 'vitest'
import { traceSearchSchema } from '@/features/sessions/search'
import { openSet, toggleOpen, tokenopsSearchSchema } from '@/features/tokenops/search'
import { compareOn, pickShared, presenterStep } from './context'

describe('URL contract', () => {
  it('junk falls back, open is canonical, and only shared keys are carried', () => {
    const t = traceSearchSchema.parse({
      preset: 'nope',
      compare: 0,
      day: '2026-02-30',
      lane: 'bogus',
      status: 'meh',
      trace: '   ',
      span: 'U3Bhbj=',
      mock: 'chaos',
      demo: 1,
    })
    // Sessions and the trace fall back to 7d (TokenOps keeps 30d).
    expect(t).toMatchObject({ preset: '7d', compare: false, demo: true })
    expect(tokenopsSearchSchema.parse({ preset: 'nope' }).preset).toBe('30d')
    for (const k of ['day', 'lane', 'status', 'trace', 'span', 'mock'] as const)
      expect(t[k]).toBeUndefined()
    expect(traceSearchSchema.parse({ span: 'ABCdef0123' }).span).toBe('ABCdef0123')

    expect(tokenopsSearchSchema.parse({ open: 'all' }).open).toBe(
      'spend,optimise,drivers,perf,month,metrics',
    )
    expect(tokenopsSearchSchema.parse({ open: 'metrics, junk,spend' }).open).toBe('spend,metrics')
    expect(tokenopsSearchSchema.parse({ open: 'junk' }).open).toBeUndefined()
    expect(tokenopsSearchSchema.parse({ compare: '1' }).compare).toBe(true)
    expect(openSet(undefined).size).toBe(0)
    expect(toggleOpen('spend', 'spend')).toBeUndefined()
    expect(toggleOpen('metrics', 'spend')).toBe('spend,metrics')

    expect(compareOn({})).toBe(true)
    expect(compareOn({ compare: false })).toBe(false)
    expect(
      pickShared({ preset: '7d', agent: undefined, day: '2026-03-11', open: 'spend', demo: true }),
    ).toEqual({ preset: '7d', demo: true })
    expect(presenterStep('/sessions/', { day: '2026-03-11' })).toBe(1)
    expect(presenterStep('/settings', {})).toBeNull()
  })
})
