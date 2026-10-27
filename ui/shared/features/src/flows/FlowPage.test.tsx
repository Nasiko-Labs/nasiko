/**
 * The flow detail page (plans/feat-flows.md §2, §2a; T2, T3) through the real router and the flows mock: the fan-out
 * showcase merged with its trace, the lone direct call, a failure in place, a paused flow, the FL-8 early completions
 * (A4, O5), the session-gated links (O1), Tempo down, not found and a failing read.
 */
import { screen, waitFor, within } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { http, HttpResponse } from 'msw'
import { afterEach, describe, expect, it } from 'vitest'
import type { FlowsState } from '@nasiko/mocks/flows'
import { configureMocks, flowsMockState } from '@nasiko/mocks/handlers'
import { now, seed, setupPinnedSeed } from '@nasiko/testing/pinnedSeed'
import { renderApp } from '@nasiko/testing/renderApp'
import { recordRequests, server } from '@nasiko/testing/setup'
import { copy } from './copy'

setupPinnedSeed()
afterEach(() => configureMocks({ seed, now, loggedIn: true, variant: null }))

const idOf = (pred: (f: FlowsState['flows'][number]) => boolean) => {
  const f = flowsMockState().flows.find(pred)
  if (!f) throw new Error('flow not in the mock')
  return f.id
}
const showcase = () => idOf((f) => !!f.title?.startsWith('Compare 2026'))
const timeline = async () =>
  (await screen.findByRole('heading', { name: copy.panel.title }, { timeout: 8000 })).closest(
    'section',
  )!

describe('Flow page', () => {
  it('draws the fan-out: lanes by first call, three searches in parallel, merged with the trace', async () => {
    renderApp(`/flows/${showcase()}`)
    expect(
      await screen.findByRole(
        'heading',
        { level: 1, name: /Compare 2026 pricing/ },
        { timeout: 8000 },
      ),
    ).toBeInTheDocument()
    const panel = await timeline()
    await waitFor(() => expect(within(panel).getByText(copy.panel.merged)).toBeInTheDocument(), {
      timeout: 8000,
    })
    expect(within(panel).getByText(copy.panel.parallel(3))).toBeInTheDocument()
    expect(
      within(panel).getByText(copy.panel.orchestrator, { selector: 'span' }),
    ).toBeInTheDocument()
    // Every drawn call is in one chart image with a summary label.
    expect(
      within(panel)
        .getByRole('group', { name: /^Timeline of/ })
        .getAttribute('aria-label'),
    ).toMatch(/^Timeline of 6 agent calls/)
    // Steps groups the parallel calls; the trace-only ones say where they came from.
    const steps = (await screen.findByRole('heading', { name: copy.steps.title })).closest(
      'section',
    )!
    expect(within(steps).getByText(copy.steps.parallel(3))).toBeInTheDocument()
    // Exact span timing: the critical path toggle is offered.
    expect(within(panel).getByRole('button', { name: copy.panel.critical })).toHaveAttribute(
      'aria-pressed',
      'true',
    )
    expect(screen.getByTestId('flow-narrative').textContent).toMatch(/^Took \d+\.\d s;/)
  })

  it('shows a lone direct call as a Call card, with Open trace and Open chat from its session (O1)', async () => {
    const id = idOf(
      (f) => f.title === null && f.steps.length === 0 && f.root.name !== 'orchestrator',
    )
    renderApp(`/flows/${id}`)
    const card = (
      await screen.findByRole('heading', { name: copy.callCard.title }, { timeout: 8000 })
    ).closest('section')!
    expect(
      within(card).getByText(/A direct call to .*; no other agents were recorded\./),
    ).toBeInTheDocument()
    expect(screen.queryByRole('heading', { name: copy.panel.title })).toBeNull()
    expect(screen.getByRole('link', { name: copy.actions.openChat })).toHaveAttribute(
      'href',
      '/chat/5eed-sess-pr-481',
    )
    await waitFor(() =>
      expect(
        screen.getByRole('link', { name: copy.actions.openTrace }).getAttribute('href'),
      ).toContain(`trace=${id}`),
    )
  })

  it('a flow without a session has neither link (O1)', async () => {
    renderApp(`/flows/${showcase()}`)
    await timeline()
    expect(screen.queryByRole('link', { name: copy.actions.openTrace })).toBeNull()
    expect(screen.queryByRole('link', { name: copy.actions.openChat })).toBeNull()
  })

  it('puts a failure in place: the narrative and the auto-opened step give the reason in plain words (F15)', async () => {
    renderApp(`/flows/${idOf((f) => f.title === 'Check the Q3 contract for renewal terms')}`)
    await timeline()
    expect(screen.getByTestId('flow-narrative').textContent).toMatch(
      /^Failed at .*: the agent didn’t answer in time\./,
    )
    // The failed step opened by itself, with the same plain reason.
    expect(
      screen.getByText('the agent didn’t answer in time', { selector: 'dd' }),
    ).toBeInTheDocument()
    expect(screen.queryByText(/upstream timeout/)).toBeNull()
  })

  it('a paused flow says who is waiting on you', async () => {
    renderApp(`/flows/${idOf((f) => !!f.title?.startsWith('Refund order'))}`)
    const panel = await timeline()
    expect(within(panel).getByText(copy.panel.waitingOnYou)).toBeInTheDocument()
    expect(screen.getByTestId('flow-narrative').textContent).toMatch(/^Waiting on you for /)
    expect(screen.getByText(copy.kpi.waiting)).toBeInTheDocument()
  })

  it('A4: a "completed" flow with a step still running reads Running and says why', async () => {
    renderApp(`/flows/${idOf((f) => f.title === 'Draft the launch announcement')}`)
    expect(
      await screen.findByText(copy.markedEarly.running, {}, { timeout: 8000 }),
    ).toBeInTheDocument()
    expect(
      screen.getByText(copy.status.running, { selector: '[data-slot="badge"]' }),
    ).toBeInTheDocument()
  })

  it('O5: trace calls that ended after completed_at give the real duration', async () => {
    const id = idOf(
      (f) => f.steps.length === 0 && f.root.name !== 'orchestrator' && f.spans.length === 3,
    )
    renderApp(`/flows/${id}`)
    expect(
      await screen.findByText(copy.markedEarly.completed, {}, { timeout: 8000 }),
    ).toBeInTheDocument()
    expect(screen.getByText('2.6 s')).toBeInTheDocument()
  })

  it('Tempo not configured: recorded steps only, no caption, no Open trace', async () => {
    configureMocks({ variant: 'trace-503' })
    renderApp(`/flows/${showcase()}`)
    const panel = await timeline()
    await waitFor(() => expect(within(panel).queryByText(copy.panel.checking)).toBeNull())
    expect(within(panel).queryByText(copy.panel.merged)).toBeNull()
    expect(within(panel).queryByText(copy.panel.parallel(3))).toBeNull()
  })

  it('a failing trace store reads Partial with Retry', async () => {
    configureMocks({ variant: 'trace-500' })
    renderApp(`/flows/${showcase()}`)
    const panel = await timeline()
    expect(
      await within(panel).findByText(copy.panel.partial, {}, { timeout: 8000 }),
    ).toBeInTheDocument()
    expect(within(panel).getByRole('button', { name: copy.panel.retry })).toBeInTheDocument()
  })

  it('another user’s flow is not found', async () => {
    renderApp(`/flows/${idOf((f) => f.title === 'Someone else’s question')}`)
    expect(await screen.findByText(copy.page.notFound, {}, { timeout: 8000 })).toBeInTheDocument()
    expect(screen.getByText(copy.page.notFoundHint)).toBeInTheDocument()
  })

  it('a failing read shows the error with Retry', async () => {
    server.use(
      http.get('/api/flows/:id', () => new HttpResponse('internal error', { status: 500 })),
    )
    renderApp(`/flows/${showcase()}`)
    expect(
      await screen.findByRole('button', { name: /retry/i }, { timeout: 8000 }),
    ).toBeInTheDocument()
  })

  it('keyboard: one tab stop, arrows in start order and across lanes, Enter selects, Esc clears (F25)', async () => {
    const { router } = renderApp(`/flows/${showcase()}`)
    const panel = await timeline()
    await waitFor(() => expect(within(panel).getByText(copy.panel.merged)).toBeInTheDocument(), {
      timeout: 8000,
    })
    const chart = within(panel).getByRole('group', { name: /^Timeline of/ })
    const bars = within(chart).getAllByRole('button')
    // One tab stop: only the first call is tabbable.
    expect(bars.filter((b) => b.tabIndex === 0)).toHaveLength(1)
    const first = bars.find((b) => b.tabIndex === 0)!
    expect(first.getAttribute('aria-label')).toMatch(/^seed-support-bot · Collect current pricing/)
    first.focus()
    await userEvent.keyboard('{ArrowRight}')
    expect(document.activeElement?.getAttribute('aria-label')).toMatch(/^seed-code-reviewer/)
    await userEvent.keyboard('{End}')
    expect(document.activeElement?.getAttribute('aria-label')).toMatch(
      /Draft a one-page comparison/,
    )
    await userEvent.keyboard('{Enter}')
    await waitFor(() =>
      expect(router.state.location.search).toMatchObject({ step: expect.stringMatching(/^step:/) }),
    )
    // The selected call's Steps row opened.
    expect(
      screen.getByText('Drafted a comparison table and a three-line recommendation.'),
    ).toBeInTheDocument()
    expect(document.activeElement).toHaveAttribute('aria-pressed', 'true')
    await userEvent.keyboard('{Escape}')
    await waitFor(() => expect(router.state.location.search).not.toHaveProperty('step'))
  })

  it('a ?step= link opens that call’s row; the Table view lists every call and selects too (F26)', async () => {
    const id = showcase()
    const { router } = renderApp(`/flows/${id}`)
    const panel = await timeline()
    await waitFor(() => expect(within(panel).getByText(copy.panel.merged)).toBeInTheDocument(), {
      timeout: 8000,
    })
    await userEvent.click(within(panel).getByRole('radio', { name: copy.view.table }))
    const table = within(panel).getByRole('table', { name: copy.table.label })
    const rows = within(table).getAllByRole('row').slice(1)
    expect(rows).toHaveLength(6)
    expect(within(table).getAllByText(copy.table.group(1))).toHaveLength(3)
    expect(within(table).getAllByText(copy.steps.fromTrace).length).toBeGreaterThanOrEqual(4)
    await userEvent.click(within(rows[1]!).getByRole('button'))
    await waitFor(() =>
      expect(router.state.location.search).toMatchObject({ step: expect.stringMatching(/^span:/) }),
    )
  })

  it('a trace that expired says so; recorded steps still draw', async () => {
    server.use(
      http.get(
        '/api/observability/trace/:id',
        () => new HttpResponse('trace not found', { status: 404 }),
      ),
    )
    renderApp(`/flows/${showcase()}`)
    const panel = await timeline()
    expect(
      await within(panel).findByText(
        `${copy.panel.recorded} · ${copy.panel.expired}`,
        {},
        { timeout: 8000 },
      ),
    ).toBeInTheDocument()
  })

  it('a failed span-detail read is Partial, and Retry reads it again', async () => {
    let fail = true
    server.use(
      http.get('/api/observability/span/:trace/:span', () =>
        fail ? new HttpResponse('internal error', { status: 500 }) : undefined,
      ),
    )
    renderApp(`/flows/${showcase()}`)
    const panel = await timeline()
    expect(
      await within(panel).findByText(copy.panel.partial, {}, { timeout: 8000 }),
    ).toBeInTheDocument()
    fail = false
    await userEvent.click(within(panel).getByRole('button', { name: copy.panel.retry }))
    expect(
      await within(panel).findByText(copy.panel.merged, {}, { timeout: 8000 }),
    ).toBeInTheDocument()
  })

  it('a lone call isn’t claimed while its trace failed: the panel and its Retry stay', async () => {
    configureMocks({ variant: 'trace-500' })
    const id = idOf(
      (f) => f.steps.length === 0 && f.root.name !== 'orchestrator' && f.spans.length === 3,
    )
    renderApp(`/flows/${id}`)
    const panel = await timeline()
    expect(
      await within(panel).findByText(copy.panel.partial, {}, { timeout: 8000 }),
    ).toBeInTheDocument()
    expect(screen.queryByRole('heading', { name: copy.callCard.title })).toBeNull()
  })

  it('a workflow flow has no Open trace or Open chat (its context id is an execution id)', async () => {
    renderApp(`/flows/${idOf((f) => f.title === 'Quarterly report: draft the revenue section')}`)
    expect(
      await screen.findByRole('heading', { name: copy.callCard.title }, { timeout: 8000 }),
    ).toBeInTheDocument()
    expect(screen.queryByRole('link', { name: copy.actions.openTrace })).toBeNull()
    expect(screen.queryByRole('link', { name: copy.actions.openChat })).toBeNull()
  })

  it('a finished flow whose trace answers 503 reads it once more at most, never every second', async () => {
    configureMocks({ variant: 'trace-503' })
    const rec = recordRequests()
    renderApp(`/flows/${showcase()}`)
    await timeline()
    await new Promise((r) => setTimeout(r, 2_500))
    const reads = rec.urls.filter((u) => u.pathname.startsWith('/api/observability/trace/')).length
    rec.stop()
    expect(reads).toBeLessThanOrEqual(2)
  })
})
