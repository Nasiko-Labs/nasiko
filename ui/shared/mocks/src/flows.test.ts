/**
 * The flows mock (plans/feat-flows.md), asserted through the MSW handlers: the caller's own flows, newest first, with
 * flows.rs's paging; another user's flow is a 404; ids it doesn't own fall through to chat's routed-turn flows; its
 * flows' traces carry the proxy spans the detail page reads; and the list read-back (A2) stops at the window start.
 */
import { http, HttpResponse } from 'msw'
import { afterEach, describe, expect, it } from 'vitest'
import { readFlowsBack } from '@nasiko/features/flows/api'
import { flowKind } from '@nasiko/features/flows/kind'
import { flowDetailSchema, flowListSchema, type Flow } from '@nasiko/features/flows/types'
import { ApiError, apiFetch } from '@nasiko/data-access/client'
import { now, seed, setupPinnedSeed } from '@nasiko/testing/pinnedSeed'
import { LIST_CAP, LIST_PAGE } from '@nasiko/features/flows/tuning'
import { server } from '@nasiko/testing/setup'
import { configureMocks, flowsMockState } from './handlers'

setupPinnedSeed()
afterEach(() => configureMocks({ seed, now, loggedIn: true, variant: null }))

const list = (q = '') =>
  apiFetch<{ data: Flow[]; total: number }>(`/api/flows${q}`, { schema: flowListSchema })
const detail = (id: string) => apiFetch(`/api/flows/${id}`, { schema: flowDetailSchema })
const status = (p: Promise<unknown>) =>
  p.then(
    () => 200,
    (e: unknown) => (e instanceof ApiError ? e.status : -1),
  )

describe('flows mock', () => {
  it('lists the caller’s flows newest first, 50 by default, total = the page length', async () => {
    const page = await list()
    expect(page.data).toHaveLength(50)
    expect(page.total).toBe(50)
    const times = page.data.map((f) => f.created_at)
    expect([...times].sort().reverse()).toEqual(times)
    expect(page.data.some((f) => f.title === 'Someone else’s question')).toBe(false)
  })

  it('filters status exactly and q on title or root agent, and pages with offset', async () => {
    const paused = await list('?status=paused')
    expect(paused.data.length).toBeGreaterThan(0)
    expect(paused.data.every((f) => f.status === 'paused')).toBe(true)
    const q = await list('?q=PRICING')
    expect(q.data.map((f) => f.title)).toEqual([
      'Compare 2026 pricing for three vendors and draft a summary',
    ])
    const [a, b] = await Promise.all([list('?limit=10'), list('?limit=10&offset=10')])
    expect(new Set([...a.data, ...b.data].map((f) => f.flow_id)).size).toBe(20)
  })

  it('covers each kind', async () => {
    const all = (await list('?limit=500')).data
    expect(new Set(all.map(flowKind))).toEqual(new Set(['orchestrated', 'direct', 'workflow']))
  })

  it('answers {flow, steps} for the caller’s flow and a bare 404 for another user’s', async () => {
    const flows = flowsMockState().flows
    const showcase = flows.find((f) => f.title?.startsWith('Compare 2026'))
    const other = flows.find((f) => f.title === 'Someone else’s question')
    const d = await detail(showcase?.id ?? '')
    expect(d.steps.map((s) => s.step_order)).toEqual([1, 2])
    // FL-4: whole seconds, while the timestamps are exact.
    expect(d.steps[0]?.latency_ms).toBe(4_000)
    expect(await status(detail(other?.id ?? ''))).toBe(404)
  })

  it('passes ids it doesn’t own on to chat’s flows (a 404 there for unknown ones)', async () => {
    expect(await status(detail('00000000000000000000000000000000'))).toBe(404)
  })

  it('serves the showcase trace: three parallel calls under the first agent’s call', async () => {
    const showcase = flowsMockState().flows.find((f) => f.title?.startsWith('Compare 2026'))
    const id = showcase?.id ?? ''
    const t = await apiFetch<{
      data: { trace: { span_lookup: Record<string, { name: string; span_id: string }> } }
    }>(`/api/observability/trace/${id}`)
    const proxies = Object.values(t.data.trace.span_lookup).filter((s) => s.name === 'a2a.proxy')
    expect(proxies).toHaveLength(6)
    const first = proxies[0]?.span_id ?? ''
    const span = await apiFetch<{ data: { span: { attributes: { agent?: { id?: string } } } } }>(
      `/api/observability/span/${id}/${first}`,
    )
    expect(span.data.span.attributes.agent?.id).toMatch(/^5eed/)
  })

  it('answers 503 for a flow’s trace when Tempo is down (trace-503)', async () => {
    configureMocks({ variant: 'trace-503' })
    const id = flowsMockState().flows[0]?.id ?? ''
    expect(await status(apiFetch(`/api/observability/trace/${id}`))).toBe(503)
  })

  it('flows-empty lists nothing; flows-absent is a bare 404', async () => {
    configureMocks({ variant: 'flows-empty' })
    expect((await list()).data).toEqual([])
    configureMocks({ variant: 'flows-absent' })
    expect(await status(list())).toBe(404)
  })
})

describe('readFlowsBack (A2)', () => {
  const signal = new AbortController().signal
  it('reads pages until the window start', async () => {
    const since = now() - 3 * 86_400_000
    const r = await readFlowsBack({ sinceMs: since }, signal)
    expect(r.complete).toBe(true)
    expect(r.rows.length).toBeGreaterThan(0)
    expect(r.rows.every((f) => Date.parse(f.created_at) >= since)).toBe(true)
  })
  it('stops at the last short page when the window reaches past every flow', async () => {
    const r = await readFlowsBack({ sinceMs: 0 }, signal)
    expect(r).toMatchObject({ complete: true, capped: false, error: null })
    expect(r.rows.length).toBe((await list('?limit=500')).data.length)
  })
  it('throws when the first page fails', async () => {
    configureMocks({ variant: 'flows-absent' })
    await expect(readFlowsBack({ sinceMs: 0 }, signal)).rejects.toBeInstanceOf(ApiError)
  })

  const fullPage = (offset: number, at = now()) =>
    Array.from({ length: LIST_PAGE }, (_, i) => ({
      flow_id: `f${offset + i}`,
      status: 'completed',
      created_at: new Date(at - (offset + i) * 1_000).toISOString(),
    }))
  const offsetOf = (request: Request) =>
    Number(new URL(request.url).searchParams.get('offset') ?? 0)

  it('keeps the pages read when a later one fails, and says so', async () => {
    server.use(
      http.get('/api/flows', ({ request }) =>
        offsetOf(request) > 0
          ? new HttpResponse('internal error', { status: 500 })
          : HttpResponse.json({ data: fullPage(0), total: LIST_PAGE }),
      ),
    )
    const r = await readFlowsBack({ sinceMs: 0 }, signal)
    expect(r).toMatchObject({ complete: false, capped: false })
    expect(r.error).toBeInstanceOf(ApiError)
    expect(r.rows).toHaveLength(LIST_PAGE)
  })

  it('a 401 on a later page still throws, so the app signs out', async () => {
    server.use(
      http.get('/api/flows', ({ request }) =>
        offsetOf(request) > 0
          ? new HttpResponse('unauthorized', { status: 401 })
          : HttpResponse.json({ data: fullPage(0), total: LIST_PAGE }),
      ),
    )
    await expect(readFlowsBack({ sinceMs: 0 }, signal)).rejects.toMatchObject({ status: 401 })
  })

  it('counts a flow once when a new one shifts it onto the next page, and stops at the cap', async () => {
    server.use(
      // Page 2 starts one row early: the last row of page 1 comes back again.
      http.get('/api/flows', ({ request }) =>
        HttpResponse.json({ data: fullPage(Math.max(0, offsetOf(request) - 1)), total: LIST_PAGE }),
      ),
    )
    const r = await readFlowsBack({ sinceMs: 0 }, signal)
    expect(new Set(r.rows.map((f) => f.flow_id)).size).toBe(r.rows.length)
    expect(r.capped).toBe(true)
    expect(r.rows.length).toBeLessThanOrEqual(LIST_CAP)
  })
})
