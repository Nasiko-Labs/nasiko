// Regression: ISSUE-002 — the paused flow's axis break showed seconds ("24 min 57 s waiting") that went stale between
// its 15 s ticks, beside a "24 min" tile.
// Found by /qa on 2026-10-06
// Report: .gstack/qa-reports/qa-report-localhost-2026-10-06.md
import { screen, within } from '@testing-library/react'
import { describe, expect, it } from 'vitest'
import { flowsMockState } from '@nasiko/mocks/handlers'
import { setupPinnedSeed } from '@nasiko/testing/pinnedSeed'
import { renderApp } from '@nasiko/testing/renderApp'
import { copy } from './copy'

setupPinnedSeed()

describe('a paused flow’s wait (ISSUE-002)', () => {
  it('reads the same whole minutes on the axis break as on its tile', async () => {
    const f = flowsMockState().flows.find((x) => x.title?.startsWith('Refund order'))
    renderApp(`/flows/${f?.id ?? ''}`)
    const panel = (
      await screen.findByRole('heading', { name: copy.panel.title }, { timeout: 8000 })
    ).closest('section')!
    const label = within(panel).getByText(/^⋯ .+ waiting ⋯$/)
    expect(label.textContent).toMatch(/^⋯ (<1 min|\d+ min|\d+ h( \d+ min)?) waiting ⋯$/)
    expect(label.textContent).not.toMatch(/\d s/)
  })
})
