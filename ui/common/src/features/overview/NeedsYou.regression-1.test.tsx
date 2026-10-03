// Regression: ISSUE-002 — a sixth Needs-you item was clipped below the fold with nothing saying there was more
// Found by /qa on 2026-09-29
// Report: .gstack/qa-reports/qa-report-localhost-2026-09-29.md
import { render, screen } from '@testing-library/react'
import {
  createMemoryHistory,
  createRootRoute,
  createRouter,
  RouterProvider,
} from '@tanstack/react-router'
import { describe, expect, it } from 'vitest'
import type { NeedsYou as NeedsYouData } from './api'
import { NeedsYou } from './components/NeedsYou'
import { copy } from './copy'
import { mergeNeeds } from './needs'

const agents = (n: number) =>
  Array.from({ length: n }, (_, k) => ({
    id: `a${k}`,
    name: `Agent ${k}`,
    reason: 'crashed',
    budget: false,
  }))
const data = (n: number): NeedsYouData => ({
  needs: mergeNeeds({
    requests: { state: 'ok', value: { chats: [], outside: 0 } },
    agents: { state: 'ok', value: agents(n) },
    budgets: { state: 'absent' },
    sessions: { state: 'ok', value: { failed: 0, checked: 25, agents: [] } },
  }),
  lastChecked: 0,
  retry: { requests: () => {}, agents: () => {}, budgets: () => {}, sessions: () => {} },
  sessions: {} as NeedsYouData['sessions'],
})

function renderCard(d: NeedsYouData) {
  const router = createRouter({
    routeTree: createRootRoute({ component: () => <NeedsYou data={d} now={0} userId="u1" /> }),
    history: createMemoryHistory({ initialEntries: ['/'] }),
  })
  return render(<RouterProvider router={router} />)
}

describe('Needs you past five rows (ISSUE-002)', () => {
  it('counts the rows below the fold', async () => {
    renderCard(data(7))
    expect(await screen.findByTestId('needs-more')).toHaveTextContent(copy.needs.more(2))
  })

  it('says nothing extra at five rows or fewer', async () => {
    renderCard(data(5))
    await screen.findAllByRole('listitem')
    expect(screen.queryByTestId('needs-more')).toBeNull()
  })
})
