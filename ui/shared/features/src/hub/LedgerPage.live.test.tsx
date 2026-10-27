/**
 * The ledger in live mode (plans/feat-agent-centric.md UC2, eng "Preview policy"): previews are absent, never drawn from
 * an unexpected live field. The recorded chat carries proposed per-turn records here, as a server answering more than
 * today's contract would; the ledger still shows only recorded tool calls and usage, with "not reported" for context
 * and memory, and never reads the proposed session budgets.
 */
import { screen, within } from '@testing-library/react'
import { describe, expect, it, vi } from 'vitest'
import { setupPinnedSeed } from '@nasiko/testing/pinnedSeed'
import { renderApp } from '@nasiko/testing/renderApp'
import { recordRequests } from '@nasiko/testing/setup'
import { copy } from './copy'

vi.mock('@nasiko/app-core/env', async (orig) => {
  const mod = await orig<typeof import('@nasiko/app-core/env')>()
  return { ...mod, env: { ...mod.env, mode: 'live', partialMocks: [] }, isPreview: () => false }
})

setupPinnedSeed()

const C4 = '5eedc000-0000-4000-8000-00000000c004'
const c = copy.ledger

describe('the ledger, live', () => {
  it('shows recorded calls and usage, no preview columns, badges or budget read', async () => {
    const rec = recordRequests()
    renderApp(`/sessions/${C4}?view=ledger`)
    const t = await screen.findByRole('table', { name: c.label })
    expect(within(t).getByText('read_file')).toBeInTheDocument()
    expect(within(t).queryByRole('columnheader', { name: c.controls })).toBeNull()
    expect(within(t).getAllByText(copy.notReported).length).toBeGreaterThan(0)
    expect(screen.queryByText(copy.rail.preview)).toBeNull()
    expect(screen.queryByRole('heading', { name: c.budgets.title })).toBeNull()
    rec.stop()
    // Never the proposed AH-8 read. Budgets 1.0 reads (the session's burn, B5) go out only where the availability probe
    // (plans/feat-budget-burn.md F11) says the server has them.
    expect(rec.urls.filter((u) => u.pathname.startsWith('/api/budgets/session/'))).toEqual([])
    const allowed = (u: URL) =>
      u.pathname === '/api/budgets/capabilities' ||
      (u.pathname === '/api/budgets' && u.searchParams.get('limit') === '1') ||
      (u.pathname === '/api/budgets/burn' && u.searchParams.get('scope') === 'session')
    expect(rec.urls.filter((u) => u.pathname.startsWith('/api/budgets') && !allowed(u))).toEqual([])
  })
})
