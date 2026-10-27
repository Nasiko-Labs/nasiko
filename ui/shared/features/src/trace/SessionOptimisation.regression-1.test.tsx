// Regression: ISSUE-003 — the session view's optimisation line was a <div> inside the trace header's description <p>
// Found by /qa on 2026-10-05
// Report: .gstack/qa-reports/qa-report-localhost-2026-10-05.md
import { screen } from '@testing-library/react'
import { describe, expect, it } from 'vitest'
import { SHOWCASE_SESSION } from '@nasiko/mocks/observability'
import { setupPinnedSeed } from '@nasiko/testing/pinnedSeed'
import { renderApp } from '@nasiko/testing/renderApp'

setupPinnedSeed()

describe('Session optimization line (ISSUE-003)', () => {
  it('renders without block elements inside a paragraph', async () => {
    renderApp(`/sessions/${SHOWCASE_SESSION}`)
    const label = await screen.findByText('Context optimization', {}, { timeout: 12_000 })
    const p = label.closest('p')
    // When it sits in the header paragraph, nothing between the paragraph and the label may be a div.
    if (p) {
      let el: HTMLElement | null = label
      while (el && el !== p) {
        expect(el.tagName).not.toBe('DIV')
        el = el.parentElement
      }
    }
    expect(p?.querySelector('div') ?? null).toBeNull()
  })
})
