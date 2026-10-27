// Regression: ISSUE-003 — a custom window longer than the 800 drawn buckets said "No deploys in this window" while its
// builds sat past the drawn part
// Found by /qa on 2026-10-11
// Report: (session scratchpad) qa/qa-report-localhost-2026-10-11.md
import { screen } from '@testing-library/react'
import { describe, expect, it } from 'vitest'
import { seed, setupPinnedSeed } from '@nasiko/testing/pinnedSeed'
import { renderApp } from '@nasiko/testing/renderApp'
import { copy } from './copy'

setupPinnedSeed()

describe('a truncated window', () => {
  it('says it is truncated and never claims the window has no changes', async () => {
    renderApp(`/agents/${seed.agents[0]!.id}?preset=custom&from=2020-01-01&to=2026-03-19`)
    expect(await screen.findByText(copy.lanes.truncated(800))).toBeInTheDocument()
    expect(await screen.findByText(new RegExp(copy.rail.showingOss))).toBeInTheDocument()
    expect(screen.queryByText(new RegExp(copy.rail.noDeploys))).toBeNull()
  })
})
