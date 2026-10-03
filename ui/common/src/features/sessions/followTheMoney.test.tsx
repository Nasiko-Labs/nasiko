/**
 * The demo path, end to end against the seed-backed MSW handlers: TokenOps spike clause →
 * that day's sessions (cost order) → the top session's trace. The same numbers at every
 * step, and a bounded number of requests.
 */
import { screen, waitFor, within } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { describe, expect, it } from 'vitest'
import { fmtMoney } from '@/lib/format'
import { observabilityData, SHOWCASE_SESSION } from '@/mocks/observability'
import { seed, setupPinnedSeed } from '@/test/pinnedSeed'
import { renderApp } from '@/test/renderApp'
import { recordRequests } from '@/test/setup'

setupPinnedSeed()

const dayTotal = seed.traces
  .filter((t) => t.started_at.startsWith(seed.spikeDate))
  .reduce((a, t) => a + t.cost_usd, 0)

describe('follow the money', () => {
  it(
    'spike clause → day sessions → top session trace, with matching figures and a request budget',
    { timeout: 20_000 },
    async () => {
      const user = userEvent.setup()
      const rec = recordRequests()
      try {
        const { router } = renderApp('/tokenops')
        // 1. TokenOps: the summary names the spike day and links to its sessions.
        const link = await screen.findByRole('link', { name: /See sessions/ })
        expect(screen.getByTestId('summary-narrative')).toHaveTextContent(
          /^In the last 30 days you spent/,
        )
        expect(screen.getByText(/^Spend peaked on Mar 11 at/)).toHaveTextContent(fmtMoney(dayTotal))
        await user.click(link)
        await waitFor(() => expect(router.state.location.pathname).toBe('/sessions'))
        expect(router.state.location.search).toMatchObject({ day: seed.spikeDate, sort: 'cost' })

        // 2. Sessions (day mode): the header total equals the TokenOps day total.
        const header = await screen.findByRole('heading', {
          level: 1,
          name: new RegExp(`^Mar 11 · \\d+ sessions · ${fmtMoney(dayTotal).replace('$', '\\$')}$`),
        })
        expect(header).toBeInTheDocument()
        expect(screen.getByText('Viewing Mar 11')).toBeInTheDocument()
        const list = await screen.findByRole('list', { name: 'Sessions' })
        const rows = within(list).getAllByRole('link')
        expect(rows[0]).toHaveAccessibleName(
          /^Review PR #481 for race conditions · Code Reviewer · \$[\d.]+ · /,
        )
        const rowCost = rows[0].getAttribute('aria-label')!.match(/\$[\d.,]+/)![0]
        await user.click(rows[0])
        await waitFor(() =>
          expect(router.state.location.pathname).toBe(`/sessions/${SHOWCASE_SESSION}`),
        )

        // 3. Trace: the session line repeats the row's cost; the narrative explains the trace.
        expect(
          await screen.findByRole(
            'heading',
            {
              level: 1,
              name: new RegExp(
                `^Session · Code Reviewer · Mar 11 .* · \\${rowCost} · \\d+ traces · ✕ failed$`,
              ),
            },
            { timeout: 8000 },
          ),
        ).toBeInTheDocument()
        const story = await screen
          .findByRole('region', { name: 'What happened' })
          .catch(() => screen.getByLabelText('What happened'))
        await waitFor(() =>
          expect(story).toHaveTextContent(
            /The planner retried `get_diff` 5 times after errors and called QA Tester 3 times through the proxy\./,
          ),
        )
        await waitFor(() =>
          expect(story).toHaveTextContent(/This trace cost \$[\d.]+; retries cost \$[\d.]+\./),
        )
        expect(story).toHaveTextContent('It failed when the last attempt timed out at 30.0 s.')
        expect(screen.getByText('Retry policy: 6 attempts, no backoff.')).toBeInTheDocument()
        // The failing attempt is selected by default.
        const tree = screen.getByRole('tree', { name: 'Spans' })
        expect(within(tree).getByRole('treeitem', { selected: true })).toHaveAccessibleName(
          /^tool\.get_diff attempt 6 of 6, Tool, starts .*, lasts 30\.0 s, 0 tokens, error$/,
        )

        // Requests: bounded (status checks cap at 25 sessions × 3 traces).
        const obs = rec.urls.filter(
          (u) => u.pathname.startsWith('/api/observability/') && !u.pathname.includes('/finops/'),
        )
        expect(obs.length).toBeLessThan(140)
      } finally {
        rec.stop()
      }
    },
  )

  it('Back from the trace restores the day list without rescanning', async () => {
    const user = userEvent.setup()
    const { router } = renderApp(`/sessions?day=${seed.spikeDate}&sort=cost`)
    const list = await screen.findByRole('list', { name: 'Sessions' })
    await user.click(within(list).getAllByRole('link')[0])
    await screen.findByRole('tree', { name: 'Spans' })
    const rec = recordRequests()
    try {
      router.history.back()
      await screen.findByRole('list', { name: 'Sessions' })
      expect(rec.urls.filter((u) => u.pathname.endsWith('/session/list'))).toHaveLength(0)
    } finally {
      rec.stop()
    }
  })

  it('the showcase session is the costliest on the spike day in the seed', () => {
    const data = observabilityData(seed)
    const day = data.sessions.filter((s) => s.traces[0].started_at.startsWith(seed.spikeDate))
    const top = [...day].sort(
      (a, b) =>
        b.traces.reduce((x, t) => x + t.cost_usd, 0) - a.traces.reduce((x, t) => x + t.cost_usd, 0),
    )[0]
    expect(top.session_id).toBe(SHOWCASE_SESSION)
  })
})
