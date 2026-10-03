// Regression: ISSUE-005 — with every session check failing (no trace store), Needs you still said "Nothing needs you"
// Found by /qa on 2026-09-29 (live, EE without Tempo)
// Report: .gstack/qa-reports/qa-report-localhost-2026-09-29.md
import { screen, waitFor, within } from '@testing-library/react'
import { http, HttpResponse } from 'msw'
import { describe, expect, it } from 'vitest'
import { setupPinnedSeed } from '@/test/pinnedSeed'
import { renderApp } from '@/test/renderApp'
import { server } from '@/test/setup'
import { copy } from './copy'

setupPinnedSeed()

describe('Needs you when no session can be checked (ISSUE-005)', () => {
  it("says it couldn't check sessions and never claims an all-clear", async () => {
    server.use(
      http.get('*/api/observability/session/:id', ({ params }) =>
        params.id === 'list'
          ? undefined
          : new HttpResponse('Tempo is not configured', { status: 503 }),
      ),
    )
    renderApp('/')
    const needs = await screen.findByTestId('overview-needs')
    expect(
      await within(needs).findByText(
        copy.couldntCheck(copy.needs.source.sessions),
        {},
        { timeout: 5000 },
      ),
    ).toBeInTheDocument()
    expect(within(needs).queryByTestId('needs-empty')).toBeNull()
    await waitFor(() =>
      expect(screen.getByTestId('overview-headline')).not.toHaveTextContent(copy.needs.nothing),
    )
  })
})
