/**
 * The seed admin's harness turns (seed-harness.ts `harnessTurns`), shared by scripts/seed-trace-usage.ts and the finops
 * mock (aggregate.ts `withHarnessTurns`): one row per turn, tokens and cost conserved per session, none on the skip day.
 */
import { describe, expect, it } from 'vitest'
import { withHarnessTurns } from './aggregate'
import { generateSeed } from './seed'
import {
  adminHarnessAgents,
  generateHarnessSeed,
  harnessTurns,
  providerOfModel,
  turnTokens,
} from './seed-harness'

const anchor = new Date('2026-03-20T15:00:00Z')
const hs = generateHarnessSeed({ anchor })

describe('harnessTurns', () => {
  const agents = adminHarnessAgents(hs)
  const turns = harnessTurns(hs, agents)
  const sessions = hs.sessions.filter((s) => agents.some((a) => a.id === s.agent_id))

  it("writes one row per turn with the session's tokens and cost conserved; unpriced turns cost nothing", () => {
    expect(sessions.length).toBeGreaterThan(0)
    expect(turns).toHaveLength(sessions.reduce((n, s) => n + s.turns, 0))
    for (const s of sessions) {
      const mine = turns.filter((t) => t.session_id === s.session_id)
      expect(mine.reduce((n, t) => n + t.input_tokens + t.output_tokens, 0)).toBe(s.tokens)
      const priced = s.turns - s.unpriced_turns
      expect(mine.reduce((n, t) => n + t.cost_usd, 0)).toBeCloseTo(priced ? s.cost_usd : 0, 9)
      expect(mine.slice(0, s.unpriced_turns).every((t) => t.cost_usd === 0)).toBe(true)
      expect(
        mine.every(
          (t, i) =>
            t.trace_id === `${s.session_id}-${String(i).padStart(2, '0')}` &&
            t.ts === Date.parse(s.started_at) + i * 60_000 &&
            t.provider === providerOfModel(s.model),
        ),
      ).toBe(true)
    }
    expect(sessions.some((s) => s.unpriced_turns > 0)).toBe(true)
    // Tokens split evenly with the remainder on the last turn; providers by model family.
    expect([0, 1, 2].map((t) => turnTokens({ tokens: 10, turns: 3 }, t))).toEqual([3, 3, 4])
    expect([
      providerOfModel('claude-sonnet-4'),
      providerOfModel('gpt-5'),
      providerOfModel('gemini-2.5-pro'),
    ]).toEqual(['anthropic', 'openai', 'other'])
  })

  it('leaves out every turn on the skip day (only those); withHarnessTurns adds the admin agents and time-ordered turns', () => {
    const day = turns[0]!.started_at.slice(0, 10)
    const skipped = harnessTurns(hs, agents, day)
    expect(skipped.some((t) => t.started_at.startsWith(day))).toBe(false)
    expect(skipped).toHaveLength(turns.filter((t) => !t.started_at.startsWith(day)).length)

    const seed = generateSeed({ anchor })
    const merged = withHarnessTurns(seed, hs)
    expect(merged.agents.slice(seed.agents.length)).toEqual(
      agents.map((a) => ({
        id: a.id,
        name: a.name,
        display_name: a.display_name,
        deleted: false,
        version: '1.0.0',
      })),
    )
    const added = merged.traces.filter((t) => !seed.traces.includes(t))
    expect(added).toHaveLength(harnessTurns(hs, agents, seed.spikeDate).length)
    expect(added.some((t) => t.started_at.startsWith(seed.spikeDate))).toBe(false)
    expect(merged.traces.every((t, i, a) => i === 0 || a[i - 1]!.ts <= t.ts)).toBe(true)
  })
})
