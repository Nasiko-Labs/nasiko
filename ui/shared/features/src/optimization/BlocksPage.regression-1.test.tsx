// Regression: ISSUE-001 — two requests from one chat read as identical Biggest senders rows
// Found by /qa on 2026-10-03
// Report: .gstack/qa-reports/qa-report-localhost-2026-10-03.md
import { screen, waitFor, within } from '@testing-library/react'
import { afterEach, describe, expect, it } from 'vitest'
import { configureMocks } from '@nasiko/mocks/handlers'
import { now, seed, setupPinnedSeed } from '@nasiko/testing/pinnedSeed'
import { renderApp } from '@nasiko/testing/renderApp'
import { copy } from './copy'

setupPinnedSeed()
afterEach(() => configureMocks({ seed, now, loggedIn: true, variant: null, superuser: null }))

describe('Biggest senders rows', () => {
  it('each row ends with its request time, so rows from one chat differ', async () => {
    renderApp('/optimization')
    const h = await screen.findByRole(
      'heading',
      { level: 2, name: copy.senders.title },
      { timeout: 12_000 },
    )
    const block = h.closest('section') as HTMLElement
    await waitFor(() => expect(within(block).getAllByRole('listitem')).toHaveLength(5), {
      timeout: 12_000,
    })
    const rows = within(block)
      .getAllByRole('listitem')
      .map((li) => li.textContent ?? '')
    for (const r of rows) expect(r).toMatch(/· \w{3} \d{1,2} \d{2}:\d{2} UTC$/)
    expect(new Set(rows).size).toBe(rows.length)
  })
})
