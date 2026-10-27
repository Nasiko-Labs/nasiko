// Regression: ISSUE-005 — with every session check failing (no trace store), Needs you still said "Nothing needs you"
// Found by /qa on 2026-09-29 (live, EE without Tempo)
// Report: .gstack/qa-reports/qa-report-localhost-2026-09-29.md
import { screen, waitFor, within } from '@testing-library/react'
import { http, HttpResponse } from 'msw'
import { describe, expect, it } from 'vitest'
import { setupPinnedSeed } from '@nasiko/testing/pinnedSeed'
import { renderApp } from '@nasiko/testing/renderApp'
import { server } from '@nasiko/testing/setup'
import { copy } from './copy'

setupPinnedSeed()

// The original failure was a per-session status check (`session/{id}`) that
// 503'd on every row while Needs you reported an all-clear. The Overview no
// longer makes those requests — status comes from `session/list` — so the same
// invariant is now guarded on the read that actually carries it: the failed-
// session count. A source that could not answer must never read as "no failures".
describe('Needs you when no session can be checked (ISSUE-005)', () => {
  it("says it couldn't check sessions and never claims an all-clear", async () => {
    server.use(
      http.get('*/api/observability/session/list', ({ request }) =>
        new URL(request.url).searchParams.get('status') === 'failed'
          ? new HttpResponse('internal error', { status: 500 })
          : undefined,
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
