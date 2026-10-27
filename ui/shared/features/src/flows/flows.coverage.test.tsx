/**
 * Ship-audit gap tests for Flows (plans/feat-flows.md): the app's one expiry path from both pages, the list's later
 * page failing (A2: rows kept, a warning with Retry) and its LIST_CAP note, a trace 404 while the flow still works
 * (its spans haven't landed: "checking", never "expired"), and "Answer the request" when exactly one pending request
 * matches the flow's chat and agent (F20, O2).
 */
import { screen, waitFor, within } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { http, HttpResponse } from 'msw'
import { afterEach, describe, expect, it } from 'vitest'
import { apiFetch } from '@nasiko/data-access/client'
import type { FlowsState } from '@nasiko/mocks/flows'
import { configureMocks, flowsMockState } from '@nasiko/mocks/handlers'
import { now, seed, setupPinnedSeed } from '@nasiko/testing/pinnedSeed'
import { renderApp } from '@nasiko/testing/renderApp'
import { recordRequests, server } from '@nasiko/testing/setup'
import { copy } from './copy'
import { LIST_CAP, LIST_PAGE } from './tuning'
import type { FlowDetail } from './types'

setupPinnedSeed()
afterEach(() => configureMocks({ seed, now, loggedIn: true, variant: null }))

const T = { timeout: 8000 }

const idOf = (pred: (f: FlowsState['flows'][number]) => boolean) => {
  const f = flowsMockState().flows.find(pred)
  if (!f) throw new Error('flow not in the mock')
  return f.id
}

/** A page of synthetic flows, newest first, an hour apart from `now()`, numbered from `offset`. */
const page = (offset: number, n: number) => ({
  data: Array.from({ length: n }, (_, i) => ({
    flow_id: `5eedf0c0-0000-4000-8000-${String(offset + i).padStart(12, '0')}`,
    status: 'completed',
    root_agent_name: 'orchestrator',
    title: `Synthetic flow ${offset + i}`,
    metadata: { context_id: null },
    created_at: new Date(now() - (offset + i + 1) * 60_000).toISOString(),
    completed_at: new Date(now() - (offset + i + 1) * 60_000 + 2_000).toISOString(),
  })),
  total: n,
})

const unauthorized = () =>
  HttpResponse.json(
    { data: null, status_code: 401, message: 'missing or invalid token' },
    { status: 401 },
  )

describe('Flows: session expiry', () => {
  it('a 401 on the list signs out through the app’s one expiry path', async () => {
    server.use(http.get('/api/flows', unauthorized))
    const { router } = renderApp('/flows')
    await waitFor(() => expect(router.state.location.pathname).toBe('/login'), T)
    expect(router.state.location.search).toMatchObject({ expired: true })
  })

  it('a 401 on a flow signs out too, never "not found"', async () => {
    server.use(http.get('/api/flows/:id', unauthorized))
    const { router } = renderApp(`/flows/${idOf((f) => !!f.title?.startsWith('Compare 2026'))}`)
    await waitFor(() => expect(router.state.location.pathname).toBe('/login'), T)
    expect(router.state.location.search).toMatchObject({ expired: true })
    expect(screen.queryByText(copy.page.notFound)).toBeNull()
  })
})

describe('Flows list: reading back (A2)', () => {
  it('a later page failing keeps the rows read and offers Retry', async () => {
    let fail = true
    server.use(
      http.get('/api/flows', ({ request }) => {
        const offset = Number(new URL(request.url).searchParams.get('offset') ?? 0)
        if (offset === 0) return HttpResponse.json(page(0, LIST_PAGE))
        return fail
          ? new HttpResponse('internal error', { status: 500 })
          : HttpResponse.json(page(offset, 3))
      }),
    )
    renderApp('/flows')
    await screen.findByText(copy.list.readFailed, {}, T)
    expect(
      screen.getByRole('table', { name: copy.list.tableLabel }).querySelectorAll('tbody tr').length,
    ).toBeGreaterThan(0)
    fail = false
    await userEvent.click(screen.getByRole('button', { name: copy.list.retry }))
    await waitFor(() => expect(screen.queryByText(copy.list.readFailed)).toBeNull(), T)
  })

  it('stops at LIST_CAP flows and says how to see older ones', async () => {
    const rec = recordRequests()
    server.use(
      http.get('/api/flows', ({ request }) => {
        const offset = Number(new URL(request.url).searchParams.get('offset') ?? 0)
        return HttpResponse.json(page(offset, LIST_PAGE))
      }),
    )
    renderApp('/flows')
    expect(await screen.findByText(copy.list.capped(LIST_CAP), {}, T)).toBeInTheDocument()
    const reads = rec.urls.filter((u) => u.pathname === '/api/flows').length
    rec.stop()
    expect(reads).toBe(LIST_CAP / LIST_PAGE)
  })

  it('a past window older than the latest LIST_CAP flows says it can’t be reached, not "no flows match"', async () => {
    server.use(
      http.get('/api/flows', ({ request }) => {
        const offset = Number(new URL(request.url).searchParams.get('offset') ?? 0)
        return HttpResponse.json(page(offset, LIST_PAGE))
      }),
    )
    // Last month: every synthetic flow is from the last nine hours.
    renderApp('/flows?preset=last-month')
    expect(await screen.findByText(copy.list.outOfReach(LIST_CAP), {}, T)).toBeInTheDocument()
    expect(screen.queryByRole('button', { name: copy.list.clear })).toBeNull()
    expect(screen.queryByRole('heading', { name: copy.list.requests })).toBeNull()
  })
})

describe('Flow page: trace edges', () => {
  it('a trace 404 while the flow still works is "checking", not "expired" (spans land late)', async () => {
    server.use(
      http.get(
        '/api/observability/trace/:id',
        () => new HttpResponse('trace not found', { status: 404 }),
      ),
    )
    const rec = recordRequests()
    renderApp(`/flows/${idOf((f) => f.title === 'Summarise this week’s support tickets')}`)
    const panel = (await screen.findByRole('heading', { name: copy.panel.title }, T)).closest(
      'section',
    )!
    await waitFor(
      () =>
        expect(rec.urls.some((u) => u.pathname.startsWith('/api/observability/trace/'))).toBe(true),
      T,
    )
    rec.stop()
    // Let the 404 settle into the panel.
    await new Promise((r) => setTimeout(r, 500))
    expect(within(panel).queryByText(copy.panel.expired, { exact: false })).toBeNull()
  })
})

describe('Answer the request (F20, O2)', () => {
  it('exactly one matching pending request opens that chat at the request', async () => {
    const id = idOf((f) => !!f.title?.startsWith('Refund order'))
    const body = await apiFetch<FlowDetail>(`/api/flows/${id}`)
    // The waiting step names its agent by id (as a proxied step does; routed steps resolve it by name, FL-9).
    const AGENT = '5eed0000-0000-4000-8000-00000000a9e1'
    const steps = body.steps.map((s) =>
      s.status === 'awaiting_human' ? { ...s, agent_id: AGENT } : s,
    )
    expect(steps.some((s) => s.agent_id === AGENT)).toBe(true)
    const SESSION = '5eedc000-0000-4000-8000-00000000a115'
    server.use(
      http.get(`/api/flows/${id}`, () =>
        HttpResponse.json({ ...body, steps, flow: { ...body.flow, session_id: SESSION } }),
      ),
      http.get('/api/hitl/pending', () =>
        HttpResponse.json({
          data: [
            {
              id: 'req-5eed-1',
              status: 'pending',
              created_at: new Date(now()).toISOString(),
              question: { message: 'Approve the refund?' },
              execution: {
                context_id: SESSION,
                chat_session_id: null,
                agent_id: AGENT,
              },
            },
          ],
        }),
      ),
    )
    renderApp(`/flows/${id}`)
    const links = await screen.findAllByRole('link', { name: copy.answer.request }, T)
    for (const l of links) expect(l).toHaveAttribute('href', `/chat/${SESSION}`)
    expect(screen.queryByRole('link', { name: copy.answer.waiting })).toBeNull()
  })
})
