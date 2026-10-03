// R2 budgets, pure logic (plans/feat-llm-router.md §5, §5.1).
import { describe, expect, it } from 'vitest'
import {
  budgetForecast,
  budgetLabel,
  budgetResetsAt,
  limitError,
  openScopes,
  parseLimit,
  parseThreshold,
  sentenceName,
  sortBudgets,
  stateView,
  thresholdError,
  toAlertOnly,
  usedPercent,
  withStopMark,
} from './budgets'
import { copy } from './copy'
import type { Budget, BudgetStatus } from './types'

const budget = (p: Partial<Budget> = {}): Budget => ({
  id: 'b1',
  scope: 'agent',
  agent_id: 'a1',
  owner_id: 'u1',
  set_by: 'u1',
  period: 'month',
  limit_usd: 50,
  thresholds: [50, 80, 100],
  action: 'alert',
  created_at: '2026-03-01T00:00:00Z',
  updated_at: '2026-03-01T00:00:00Z',
  ...p,
})
const status = (daily: number[], p: Partial<BudgetStatus> = {}): BudgetStatus => ({
  budget_id: 'b1',
  used_usd: daily.reduce((s, v) => s + v, 0),
  unpriced_calls: 0,
  resets_at: '2026-04-01T00:00:00Z',
  state: 'ok',
  crossed: null,
  stopped: false,
  daily: daily.map((v, i) => ({ date: `2026-03-${String(i + 1).padStart(2, '0')}`, cost_usd: v })),
  ...p,
})

describe('budgetForecast', () => {
  it('is hidden before a few days, and with no spend yet, and says which', () => {
    expect(budgetForecast(status([5, 5]), 50, new Date('2026-03-02T12:00:00Z'))).toMatchObject({
      show: false,
      hidden: 'early',
      overLimit: false,
    })
    expect(
      budgetForecast(status(Array(10).fill(0)), 50, new Date('2026-03-11T00:00:00Z')),
    ).toMatchObject({ show: false, hidden: 'no-spend', overLimit: false })
  })
  it('is a range when the median and mean differ', () => {
    const f = budgetForecast(status([1, 1, 1, 1, 10]), 500, new Date('2026-03-06T00:00:00Z'))
    expect(f.low!).toBeLessThan(f.high!)
    expect(f.overLimit).toBe(false)
  })
  it('gives a band from the median and mean daily rate, and flags a high end over the limit', () => {
    const f = budgetForecast(status(Array(10).fill(2)), 50, new Date('2026-03-11T00:00:00Z'))
    expect(f.show).toBe(true)
    // $20 so far, 21 days of March left at $2/day.
    expect(f.low).toBeCloseTo(62)
    expect(f.high).toBeCloseTo(62)
    expect(f.overLimit).toBe(true)
    expect(
      budgetForecast(status(Array(10).fill(0.5)), 50, new Date('2026-03-11T00:00:00Z')).overLimit,
    ).toBe(false)
  })
})

describe('stateView and usedPercent', () => {
  it('labels each state, with the crossed mark on a warning', () => {
    expect(stateView(undefined)).toBeNull()
    expect(stateView(status([1]))).toEqual({ kind: 'ok', label: copy.budgetOk })
    expect(stateView(status([1], { state: 'warning', crossed: 80 }))).toEqual({
      kind: 'warning',
      label: copy.budgetWarning(80),
    })
    expect(stateView(status([1], { state: 'exceeded', crossed: 100 }))).toEqual({
      kind: 'exceeded',
      label: copy.budgetExceeded,
    })
  })
  it('caps the bar at full', () => {
    expect(usedPercent(25, 50)).toBe(50)
    expect(usedPercent(80, 50)).toBe(100)
    expect(usedPercent(5, 0)).toBe(0)
  })
})

describe('editor rules', () => {
  it('thresholds are whole percents 1-100, unique, at least one', () => {
    expect(thresholdError([50, 80, 100])).toBeNull()
    expect(thresholdError([])).toBe(copy.thresholdsRequired)
    expect(thresholdError([0])).toBe(copy.thresholdRange)
    expect(thresholdError([101])).toBe(copy.thresholdRange)
    expect(thresholdError([50.5])).toBe(copy.thresholdRange)
    expect(thresholdError([50, 50])).toBe(copy.thresholdDuplicate)
  })
  it('parses typed thresholds and limits', () => {
    expect(parseThreshold('90')).toBe(90)
    expect(parseThreshold(' 90% ')).toBe(90)
    expect(parseThreshold('0')).toBeNull()
    expect(parseThreshold('abc')).toBeNull()
    expect(limitError('')).toBe(copy.required)
    expect(limitError('0')).toBe(copy.limitPositive)
    expect(limitError('-5')).toBe(copy.limitPositive)
    expect(limitError('$12.50')).toBeNull()
    expect(parseLimit('$12.505')).toBe(12.51)
    // Checked after rounding: this would be sent as $0.
    expect(limitError('0.004')).toBe(copy.limitPositive)
    expect(limitError('1,250.50')).toBeNull()
    expect(parseLimit('1,250.50')).toBe(1250.5)
  })
  it('Switch to Alert only keeps the limit and marks (a full replace, guarded by updated_at)', () => {
    expect(
      toAlertOnly(
        budget({ limit_usd: 40, thresholds: [90, 100], action: 'stop', updated_at: 'T1' }),
      ),
    ).toEqual({ limit_usd: 40, thresholds: [90, 100], action: 'alert', expected_updated_at: 'T1' })
  })
  it('a stop budget always has the 100% mark', () => {
    expect(withStopMark([50, 80], 'stop')).toEqual([50, 80, 100])
    expect(withStopMark([50, 80], 'alert')).toEqual([50, 80])
    expect(withStopMark([100], 'stop')).toEqual([100])
  })
  it('the reset falls back to the first of next UTC month', () => {
    expect(budgetResetsAt(undefined, new Date('2026-12-31T23:00:00Z'))).toBe(
      '2027-01-01T00:00:00.000Z',
    )
    expect(budgetResetsAt([status([1], { resets_at: 'X' })], new Date())).toBe('X')
  })
})

describe('scopes and order', () => {
  const agents = [
    { id: 'a1', name: 'Alpha' },
    { id: 'a2', name: 'Beta' },
  ]
  it('offers the owner budget once and each agent without a budget', () => {
    expect(openScopes([], agents)).toEqual({ owner: true, agents })
    expect(openScopes([budget({ scope: 'owner', agent_id: null }), budget()], agents)).toEqual({
      owner: false,
      agents: [agents[1]],
    })
  })
  it('labels and sorts: you first, then agents by name; an unknown agent stays visible', () => {
    const name = (id: string) => agents.find((a) => a.id === id)?.name
    const list = sortBudgets(
      [
        budget({ id: 'x', agent_id: 'a2' }),
        budget({ id: 'o', scope: 'owner', agent_id: null }),
        budget({ id: 'y', agent_id: 'a1' }),
      ],
      name,
    )
    expect(list.map((b) => budgetLabel(b, name))).toEqual([copy.budgetOwner, 'Alpha', 'Beta'])
    expect(budgetLabel(budget({ agent_id: 'deadbeef-0000' }), name)).toBe(
      copy.budgetUnknownAgent('deadbeef-0000'),
    )
    // Sentences word the owner budget as "your budget".
    expect(sentenceName(budget({ scope: 'owner', agent_id: null }), name)).toBeNull()
    expect(copy.switchedToAlert(null)).toMatch(/^Your budget is on Alert only/)
    expect(copy.budgetSaved(null)).toBe('Your budget is saved.')
    expect(sentenceName(budget(), name)).toBe('Alpha')
  })
})
