/**
 * Regression: ISSUE-006 — an agent with no sessions in its sample read "the latest 0 sessions"
 * Found by /qa on 2026-10-11 (PR1c)
 * Report: .gstack/qa-reports/qa-report-localhost-2026-10-11.md
 *
 * The hub's session sample starts at the agent's creation (or 7 days back), so a harness created today scanned nothing,
 * and both Needs you and Recent sessions said "latest 0 sessions". An empty sample means this agent had none in 7 days.
 * (PR1b dated the mock's harnesses from their first session, so the page case now empties the sample directly.)
 */
import { screen, within } from '@testing-library/react'
import { afterEach, describe, expect, it } from 'vitest'
import { configureMocks } from '@nasiko/mocks/handlers'
import { now, seed, setupPinnedSeed } from '@nasiko/testing/pinnedSeed'
import { renderApp } from '@nasiko/testing/renderApp'
import { copy } from './copy'

setupPinnedSeed()
afterEach(() =>
  configureMocks({ seed, now, loggedIn: true, variant: null, persona: null, chatVariants: [] }),
)

const panel = async (name: string) =>
  (await screen.findByRole('heading', { name })).closest('section') as HTMLElement

describe('an empty session sample', () => {
  it('says none in the last 7 days, never "the latest 0 sessions"', () => {
    expect(copy.needs.partSessions(0, 0, 0)).toBe("this agent's sessions (none in the last 7 days)")
    expect(copy.runs.sessionsNone(0)).toBe('None in the last 7 days.')
    expect(copy.runs.sessionsNone(1)).toBe('None in the latest 1 session checked.')
    expect(copy.needs.partSessions(0, 0, 25)).toContain('the latest 25 sessions you can see')
  })

  it('an agent whose sample is empty reads that way on its hub', async () => {
    // Every session read is empty (the sample a harness registered today would get).
    configureMocks({ variant: 'empty' })
    renderApp(`/agents/${seed.agents[0]!.id}`)
    const runs = await panel(copy.runs.sessions)
    const needs = await panel(copy.needs.title)
    expect(await within(runs).findByText(copy.runs.sessionsNone(0))).toBeInTheDocument()
    expect(await within(needs).findByText(/^Checked /)).toHaveTextContent(
      copy.needs.partSessions(0, 0, 0),
    )
    for (const p of [runs, needs]) expect(p).not.toHaveTextContent(/latest 0 sessions/)
  })
})
