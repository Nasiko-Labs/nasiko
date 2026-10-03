import { describe, expect, it } from 'vitest'
import { NAV_ITEMS } from '@/app/shell/nav'
import { DRAFT_PREFIX } from '@/lib/draftKeys'
import {
  firstOpenStep,
  guideDue,
  guideProviders,
  minutesLeft,
  opensFor,
  skipKey,
  STEPS,
  ticks,
} from './logic'
import { PERSONAS } from './types'

describe('onboarding logic', () => {
  it('has the six steps in order', () => {
    expect(STEPS).toEqual(['welcome', 'role', 'model', 'agent', 'optimise', 'ready'])
  })

  it('skips the optimise step when resuming, because it completes nothing', () => {
    // It configures nothing by itself: stopping a returning user on a page they have already read
    // would be a step backwards, while a first run still walks through it linearly.
    expect(firstOpenStep({ role: true, model: true, agent: true })).toBe('ready')
    expect(firstOpenStep({ role: true, model: true, agent: false })).toBe('agent')
  })

  it('opens a nav page for every persona', () => {
    const paths: string[] = NAV_ITEMS.map((i) => i.to)
    for (const p of PERSONAS) expect(paths).toContain(opensFor(p))
    expect(opensFor('data_analyst')).toBe('/tokenops')
    expect(opensFor('leadership')).toBe('/')
  })

  it('is due only for a known first-time user who has not skipped', () => {
    expect(guideDue({ is_first_time_user: true, persona: null }, false)).toBe(true)
    expect(guideDue({ is_first_time_user: true, persona: null }, true)).toBe(false)
    expect(guideDue({ is_first_time_user: false, persona: 'sre' }, false)).toBe(false)
    expect(guideDue(undefined, false)).toBe(false)
  })

  it('ticks and resumes at the first open step', () => {
    expect(ticks({ persona: null, configs: 0, agents: 0 })).toEqual({
      role: false,
      model: false,
      agent: false,
    })
    expect(firstOpenStep(ticks({ persona: null, configs: 2, agents: 1 }))).toBe('role')
    expect(firstOpenStep(ticks({ persona: 'sre', configs: 0, agents: 1 }))).toBe('model')
    expect(firstOpenStep(ticks({ persona: 'sre', configs: 1, agents: 0 }))).toBe('agent')
    expect(firstOpenStep(ticks({ persona: 'sre', configs: 1, agents: 1 }))).toBe('ready')
  })

  it('estimates minutes left like the prototype', () => {
    expect([1, 2, 3, 4, 5, 6].map(minutesLeft)).toEqual([3, 3, 2, 2, 1, 0])
  })

  it('keeps the skip flag under the drafts prefix', () => {
    expect(skipKey('u1').startsWith(DRAFT_PREFIX)).toBe(true)
  })

  it('offers the built-in providers the catalog prices, Gemini through its google entries', () => {
    const m = (...names: string[]) => names.map((model) => ({ model }))
    expect(
      guideProviders([
        { provider: 'google', models: m('gemini-2.0-flash') },
        { provider: 'anthropic', models: m('claude-sonnet-4') },
        { provider: 'openai', models: m('gpt-4o', 'gpt-4o-mini') },
        { provider: 'mistral', models: m('mistral-large') },
      ]),
    ).toEqual([
      { provider: 'openai', models: ['gpt-4o', 'gpt-4o-mini'] },
      { provider: 'anthropic', models: ['claude-sonnet-4'] },
      { provider: 'gemini', models: ['gemini-2.0-flash'] },
    ])
    expect(guideProviders(undefined)).toEqual([])
  })
})
