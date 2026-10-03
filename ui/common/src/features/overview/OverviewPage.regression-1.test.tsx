// Regression: ISSUE-003 — Overview links to Sessions opened a 30-day window (the default), not the card's 24 h
// Found by /qa on 2026-09-29
// Report: .gstack/qa-reports/qa-report-localhost-2026-09-29.md
import { screen, waitFor, within } from '@testing-library/react'
import { describe, expect, it } from 'vitest'
import { setupPinnedSeed } from '@/test/pinnedSeed'
import { renderApp } from '@/test/renderApp'

setupPinnedSeed()

describe('Overview → Sessions links (ISSUE-003)', () => {
  it('opens Sessions on the 7-day window from the card title, its rows and its empty-state link', async () => {
    renderApp('/')
    const card = await screen.findByTestId('overview-sessions')
    await waitFor(() =>
      expect(within(card).getAllByTestId('recent-session').length).toBeGreaterThan(0),
    )
    const hrefs = within(card)
      .getAllByRole('link')
      .map((a) => a.getAttribute('href') ?? '')
    expect(hrefs.length).toBeGreaterThan(1)
    for (const h of hrefs) expect(h).toMatch(/[?&]preset=7d/)
  })
})
