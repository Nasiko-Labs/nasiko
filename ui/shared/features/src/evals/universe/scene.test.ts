import { describe, expect, it } from 'vitest'
import { AXIS_SPAN, buildModelPalette, overlapOffset, pointColor, pointRadius, pointXY } from './scene'
import type { UniverseAttempt } from './types'

const attempt = (overrides: Partial<UniverseAttempt> = {}): UniverseAttempt => ({
  attempt_id: 'a1',
  run_id: 'r1',
  task_id: 't1',
  model_id: 'm1',
  model_name: 'Flint (Free)',
  provider: 'openrouter',
  model: 'recursive/flint-v1.0:free',
  status: 'done',
  metrics: null,
  latency_ms: 1000,
  cost_usd: 0,
  input_tokens: 0,
  output_tokens: 0,
  worst_price_source: 'Default',
  error: null,
  ...overrides,
})

describe('overlapOffset', () => {
  it('leaves a lone point exactly on its score, no offset', () => {
    expect(overlapOffset(0, 1)).toEqual([0, 0])
  })

  it('spreads several same-score points apart, none colliding', () => {
    const a = overlapOffset(0, 3)
    const b = overlapOffset(1, 3)
    const c = overlapOffset(2, 3)
    expect(a).not.toEqual(b)
    expect(b).not.toEqual(c)
    expect(a).not.toEqual(c)
  })
})

describe('pointXY', () => {
  it('is a fixed off-quadrant spot for ungraded, pending, or failed — never a fabricated (0, 0)', () => {
    expect(pointXY(attempt({ status: 'pending', metrics: null }))).toEqual([-0.5, -0.5])
    expect(pointXY(attempt({ status: 'done', metrics: null }))).toEqual([-0.5, -0.5])
    expect(pointXY(attempt({ status: 'failed', metrics: null }))).toEqual([-0.5, -0.5])
  })

  it('plots on the shared recall/precision scale once graded and done', () => {
    const metrics = {
      true_positives: 1,
      false_positives: 0,
      false_negatives: 0,
      precision: 0.5,
      recall: 1,
      severity_accuracy: 1,
    }
    expect(pointXY(attempt({ status: 'done', metrics }))).toEqual([AXIS_SPAN, AXIS_SPAN * 0.5])
  })
})

describe('buildModelPalette', () => {
  it('gives every distinct model its own color, up to the palette size', () => {
    const keys = [
      'openai/gpt-oss-120b',
      'qwen/qwen3.7-flash',
      'deepseek/deepseek-v4-flash',
      'tencent/hy4-flash',
      'recursive/flint-v1.0:free',
      'mistralai/mistral-small-24b-instruct-2501',
    ]
    const palette = buildModelPalette(keys)
    const colors = keys.map((k) => palette.get(k))
    expect(new Set(colors).size).toBe(keys.length)
  })

  it('is stable regardless of input order (sorts first)', () => {
    const a = buildModelPalette(['b', 'a', 'c'])
    const b = buildModelPalette(['c', 'b', 'a'])
    expect(a.get('a')).toBe(b.get('a'))
    expect(a.get('b')).toBe(b.get('b'))
    expect(a.get('c')).toBe(b.get('c'))
  })
})

describe('pointColor', () => {
  it('is gray for an ungraded or not-yet-done attempt, never a fabricated color', () => {
    const palette = buildModelPalette(['recursive/flint-v1.0:free'])
    expect(pointColor(attempt({ status: 'pending', metrics: null }), palette)).toBe('#9ca3af')
    expect(pointColor(attempt({ status: 'done', metrics: null }), palette)).toBe('#9ca3af')
  })

  it('is the model color once graded and done', () => {
    const metrics = {
      true_positives: 1,
      false_positives: 0,
      false_negatives: 0,
      precision: 1,
      recall: 1,
      severity_accuracy: 1,
    }
    const a = attempt({ status: 'done', metrics })
    const palette = buildModelPalette([a.model ?? a.model_name])
    expect(pointColor(a, palette)).toBe(palette.get(a.model ?? a.model_name))
  })
})

describe('pointRadius', () => {
  it('still renders a visible minimum size when tokens are 0 (the known usage-display bug)', () => {
    const r = pointRadius(attempt({ input_tokens: 0, output_tokens: 0 }))
    expect(r).toBeGreaterThan(0)
  })

  it('grows, but stays bounded, as token usage grows', () => {
    const small = pointRadius(attempt({ input_tokens: 10, output_tokens: 10 }))
    const large = pointRadius(attempt({ input_tokens: 50_000, output_tokens: 50_000 }))
    expect(large).toBeGreaterThan(small)
    expect(large).toBeLessThanOrEqual(0.4)
  })
})
