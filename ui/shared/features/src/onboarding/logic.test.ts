import { describe, expect, it } from 'vitest'
import { NAV_ITEMS } from '@nasiko/app-core/shell/nav'
import { DRAFT_PREFIX } from '@nasiko/app-core/draftKeys'
import {
  firstOpenStep,
  guideDue,
  guideProviders,
  MANDATORY,
  MAX_SEEDED,
  minutesLeft,
  opensFor,
  skipKey,
  STEPS,
  ticks,
  toDeploy,
  togglePick,
} from './logic'
import { PERSONAS } from './types'

describe('onboarding logic', () => {
  it('always deploys the meeting notes processor: the picks on Prebuilt, it alone from any other tab', () => {
    expect(MANDATORY).toBe('meeting-notes-processor')
    const none = new Set<string>()
    expect(toDeploy('seeded', [MANDATORY, 'summarizer'], none).map((a) => a.reference)).toEqual([
      'registry.nasiko.dev/nasiko/meeting-notes-processor:1.0.0',
      'registry.nasiko.dev/nasiko/summarizer:1.0.0',
    ])
    for (const tab of ['registry', 'github', 'upload'] as const)
      expect(toDeploy(tab, [MANDATORY, 'summarizer'], none).map((a) => a.name)).toEqual([MANDATORY])
  })

  it('never deploys an agent that already exists', () => {
    const existing = new Set([MANDATORY])
    expect(toDeploy('registry', [MANDATORY], existing)).toEqual([])
    expect(toDeploy('seeded', [MANDATORY, 'translator'], existing).map((a) => a.name)).toEqual([
      'translator',
    ])
  })

  it('never drops the mandatory pick, and a pick at the cap swaps out the latest one', () => {
    expect(togglePick(['M'], 'M', 'M')).toEqual(['M'])
    expect(togglePick(['M'], 'A', 'M')).toEqual(['M', 'A'])
    expect(togglePick(['M', 'A'], 'A', 'M')).toEqual(['M'])
    expect(MAX_SEEDED).toBe(2)
    expect(togglePick(['M', 'A'], 'B', 'M')).toEqual(['M', 'B'])
  })

  it('has the three steps in order, no role, the first agent and the model key as one', () => {
    expect(STEPS).toEqual(['welcome', 'start', 'harness'])
  })

  it('resumes at the coding harnesses once everything is done', () => {
    expect(firstOpenStep({ model: true, agent: true })).toBe('harness')
    expect(firstOpenStep({ model: true, agent: false })).toBe('start')
  })

  it('counts down two minutes', () => {
    expect(STEPS.map((_, i) => minutesLeft(i + 1))).toEqual([2, 1, 0])
  })

  it('opens a nav page for every persona', () => {
    const paths: string[] = NAV_ITEMS.map((i) => i.to)
    for (const p of PERSONAS) expect(paths).toContain(opensFor(p))
    expect(opensFor('data_analyst')).toBe('/tokenops')
    expect(opensFor('leadership')).toBe('/')
    // Connecting a local coding harness comes later, never as a persona's first page.
    for (const p of PERSONAS) expect(opensFor(p)).not.toBe('/harnesses')
  })

  it('is due only for a known first-time user who has not skipped', () => {
    expect(guideDue({ is_first_time_user: true, persona: null }, false)).toBe(true)
    expect(guideDue({ is_first_time_user: true, persona: null }, true)).toBe(false)
    expect(guideDue({ is_first_time_user: false, persona: 'sre' }, false)).toBe(false)
    expect(guideDue(undefined, false)).toBe(false)
  })

  it('ticks and resumes at the first open step', () => {
    expect(ticks({ configs: 0, agents: 0 })).toEqual({ model: false, agent: false })
    expect(firstOpenStep(ticks({ configs: 0, agents: 1 }))).toBe('start')
    expect(firstOpenStep(ticks({ configs: 1, agents: 0 }))).toBe('start')
    expect(firstOpenStep(ticks({ configs: 1, agents: 1 }))).toBe('harness')
  })

  it('keeps the shown flag outside the drafts prefix, so sign-out keeps it', () => {
    expect(skipKey('u1').startsWith(DRAFT_PREFIX)).toBe(false)
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
