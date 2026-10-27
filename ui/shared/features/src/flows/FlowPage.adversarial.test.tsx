/**
 * Adversarial review fixes (2026-10-07) on the flow page: a flow that goes away after the page loaded stops being
 * re-read and says so; a call that fails after the first render opens its reason (F15); a paused call draws its work
 * up to the pause and the wait after it.
 */
import { render, screen, waitFor, within } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { http, HttpResponse } from 'msw'
import { afterEach, describe, expect, it } from 'vitest'
import { configureMocks, flowsMockState } from '@nasiko/mocks/handlers'
import { FIXED, now, seed, setupPinnedSeed } from '@nasiko/testing/pinnedSeed'
import { renderApp } from '@nasiko/testing/renderApp'
import { recordRequests, server } from '@nasiko/testing/setup'
import type { Call } from './calls'
import { Steps } from './components/Steps'
import { copy } from './copy'
import { RUNNING_POLL_MS } from './tuning'

setupPinnedSeed()
afterEach(() => configureMocks({ seed, now, loggedIn: true, variant: null }))

const FLOW = '5eedf00000000000000000000000adv1'
const at = FIXED.toISOString()
const running = {
  flow: {
    flow_id: FLOW,
    root_agent_name: 'writer',
    title: 'A flow that goes away',
    status: 'running',
    created_at: at,
    completed_at: null,
  },
  steps: [
    {
      id: 'st1',
      step_order: 1,
      depth: 1,
      agent_name: 'writer',
      status: 'running',
      created_at: at,
      completed_at: null,
    },
  ],
}

describe('flow page: adversarial review fixes', () => {
  it('a flow that 404s after it loaded shows "not found" and stops re-reading', async () => {
    let gone = false
    server.use(
      http.get(`/api/flows/${FLOW}`, () =>
        gone ? new HttpResponse(null, { status: 404 }) : HttpResponse.json(running),
      ),
      http.get(`/api/observability/trace/${FLOW}`, () =>
        HttpResponse.text('not found', { status: 404 }),
      ),
    )
    renderApp(`/flows/${FLOW}`)
    await screen.findByRole(
      'heading',
      { level: 1, name: 'A flow that goes away' },
      { timeout: 8000 },
    )
    gone = true
    expect(
      await screen.findByText(copy.page.notFound, {}, { timeout: RUNNING_POLL_MS + 6000 }),
    ).toBeInTheDocument()
    const rec = recordRequests()
    await new Promise((r) => setTimeout(r, RUNNING_POLL_MS + 1000))
    const reads = rec.urls.filter((u) => u.pathname === `/api/flows/${FLOW}`).length
    rec.stop()
    expect(reads).toBe(0)
  })

  it('a call that fails after the first render opens with its reason; a collapsed row stays collapsed', async () => {
    const call = (status: string): Call => ({
      key: 'step:a',
      agentId: null,
      agentName: 'alpha',
      startMs: 0,
      endMs: status === 'running' ? null : 1_000,
      status,
      source: 'recorded',
      stepId: 'a',
      spanId: null,
      parentKey: null,
      input: 'Check the policy',
      output: null,
      error: status === 'failed' ? 'upstream timeout' : null,
      tokens: null,
      exactEnd: true,
      maybeSame: false,
      waitStartMs: null,
    })
    const props = {
      fanOuts: [],
      flowStartMs: 0,
      now: 2_000,
      traceLink: null,
      selected: null,
      onSelect: () => {},
    }
    const { rerender } = render(<Steps {...props} calls={[call('running')]} />)
    expect(screen.queryByText(copy.steps.reason)).toBeNull()
    rerender(<Steps {...props} calls={[call('failed')]} />)
    expect(screen.getByText('the agent didn’t answer in time', { selector: 'dd' })).toBeVisible()
    // Collapsed by hand, it stays collapsed on the next poll.
    await userEvent.click(screen.getByRole('button', { name: /alpha/ }))
    rerender(<Steps {...props} calls={[{ ...call('failed'), output: 'x' }]} />)
    expect(screen.queryByText('the agent didn’t answer in time', { selector: 'dd' })).toBeNull()
  })

  it('a paused call draws its work up to the pause, then the wait', async () => {
    const f = flowsMockState().flows.find((x) => x.title?.startsWith('Refund order'))!
    renderApp(`/flows/${f.id}`)
    const panel = (
      await screen.findByRole('heading', { name: copy.panel.title }, { timeout: 8000 })
    ).closest('section')!
    expect(within(panel).getByText(copy.panel.waitingOnYou)).toBeInTheDocument()
    expect(panel.querySelector('[data-part="work"]')).not.toBeNull()
  })

  it('one pending-requests poll serves every "Answer the request" on the page', async () => {
    const f = flowsMockState().flows.find((x) => x.title?.startsWith('Refund order'))!
    const { queryClient } = renderApp(`/flows/${f.id}`)
    await screen.findByRole('heading', { name: copy.panel.title }, { timeout: 8000 })
    // Open the waiting call's row: its own button joins the header's.
    await userEvent.click(screen.getByRole('button', { name: /^2\. /, expanded: false }))
    await waitFor(() =>
      expect(screen.getAllByRole('link', { name: copy.answer.waiting }).length).toBe(2),
    )
    const polls = queryClient
      .getQueryCache()
      .findAll({ queryKey: ['chat', 'pending'] })
      .map((q) => q.getObserversCount())
    expect(polls).toEqual([1])
  })

  it('a flow with nothing waiting reads no pending requests', async () => {
    const rec = recordRequests()
    renderApp(
      `/flows/${flowsMockState().flows.find((x) => x.title?.startsWith('Compare 2026'))!.id}`,
    )
    await screen.findByRole('heading', { name: copy.panel.title }, { timeout: 8000 })
    expect(rec.urls.some((u) => u.pathname === '/api/hitl/pending')).toBe(false)
    rec.stop()
  })
})
