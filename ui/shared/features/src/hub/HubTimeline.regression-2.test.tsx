// Regression: ISSUE-004 — a coding harness's hub asked for the per-agent savings series, which a harness never has
// (the request failed with a 400 in the console)
// Found by /qa on 2026-10-11
// Report: (session scratchpad) qa/qa-report-localhost-2026-10-11.md
import { screen } from '@testing-library/react'
import { describe, expect, it } from 'vitest'
import { setupPinnedSeed } from '@nasiko/testing/pinnedSeed'
import { agentNamed } from '@nasiko/testing/agents'
import { renderApp } from '@nasiko/testing/renderApp'
import { recordRequests } from '@nasiko/testing/setup'
import { copy } from './copy'

setupPinnedSeed()

describe('a coding harness hub', () => {
  it('reads no savings series', async () => {
    // The seed admin's own harness: its finops are visible, so the hub gets past the gate.
    const harness = await agentNamed('admin-claude-code')
    const rec = recordRequests()
    renderApp(`/agents/${harness.id}`)
    await screen.findByText(copy.lanes.peakP95)
    rec.stop()
    expect(rec.urls.filter((u) => u.pathname.endsWith('/finops/context-savings'))).toHaveLength(0)
  })
})
