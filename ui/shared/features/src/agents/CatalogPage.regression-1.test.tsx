// Regression: ISSUE-001 — the Overview's health filter listed agents without saying why they were listed
// Found by /qa on 2026-09-29
// Report: .gstack/qa-reports/qa-report-localhost-2026-09-29.md
import { screen, waitFor, within } from '@testing-library/react'
import { describe, expect, it } from 'vitest'
import { setupPinnedSeed } from '@nasiko/testing/pinnedSeed'
import { renderApp } from '@nasiko/testing/renderApp'

setupPinnedSeed()

describe('Agents catalog ?health= (ISSUE-001)', () => {
  it("shows each filtered agent's health reason", async () => {
    renderApp('/agents?health=action')
    await screen.findByRole('button', { name: /Remove the Health: Needs action filter/ })
    const main = screen.getByRole('main')
    // The top bar's breadcrumb is a list too.
    const rows = () =>
      within(main)
        .getAllByRole('listitem')
        .filter((li) => !li.closest('[data-slot="breadcrumb"]'))
    await waitFor(() => expect(rows().length).toBeGreaterThan(0))
    const items = rows()
    for (const li of items)
      expect(within(li).getByTestId('health-reason').textContent).toMatch(/^Why: \S/)
  })

  it('shows no reasons without the filter', async () => {
    renderApp('/agents')
    const main = await screen.findByRole('main')
    await waitFor(() => expect(within(main).getAllByRole('listitem').length).toBeGreaterThan(0))
    expect(within(main).queryAllByTestId('health-reason')).toHaveLength(0)
  })
})
