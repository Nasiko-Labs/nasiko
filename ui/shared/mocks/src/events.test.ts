// Regression: ISSUE-002 — the mock event log recorded changes before the agent existed (a harness created today read
// "Token optimization turned on on Sep 29")
// Found by /qa on 2026-10-11
// Report: (session scratchpad) qa/qa-report-localhost-2026-10-11.md
import { describe, expect, it } from 'vitest'
import { setupPinnedSeed } from '@nasiko/testing/pinnedSeed'

setupPinnedSeed()

interface Row {
  id: string
  created_at: string
}

async function allAgents(): Promise<Row[]> {
  const out: Row[] = []
  for (let offset = 0; ; offset += 100) {
    const page = (await (await fetch(`/api/agents?limit=100&offset=${offset}`)).json()) as Row[]
    out.push(...page)
    if (page.length < 100) return out
  }
}

describe('events mock', () => {
  it('records nothing before an agent existed', async () => {
    const agents = await allAgents()
    expect(agents.length).toBeGreaterThan(0)
    for (const a of agents) {
      const body = (await (
        await fetch(`/api/observability/agent/${a.id}/events?limit=500`)
      ).json()) as {
        data: { events: { at: string }[] }
      }
      for (const e of body.data.events)
        expect(Date.parse(e.at)).toBeGreaterThanOrEqual(Date.parse(a.created_at))
    }
  })
})
