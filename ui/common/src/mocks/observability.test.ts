/**
 * Mock observability data: shapes and cross-endpoint invariants, asserted THROUGH the MSW
 * handlers (what the app sees), not the generator internals; 'richer mock traces' checks the span
 * generator's own invariants directly (unique step names, error ownership, gen_ai usage).
 */
import { afterEach, describe, expect, it } from 'vitest'
import {
  flattenSpans,
  isTraceFailing,
  ownTokens,
  TRACE_TOTAL_SPAN,
} from '@/features/observability/spans'
import type {
  SessionDetailResponse,
  SessionListResponse,
  SessionSummary,
  SpanDetailResponse,
  TraceDetailResponse,
} from '@/features/observability/types'
import { now, seed, setupPinnedSeed } from '@/test/pinnedSeed'
import { configureMocks } from './handlers'
import {
  encodeSpanId,
  generateSpans,
  observabilityData,
  SHOWCASE_SESSION,
  traceDetail,
} from './observability'
import { utcDate } from './seed'

setupPinnedSeed()
afterEach(() => configureMocks({ variant: null }))

const get = async <T>(path: string): Promise<{ status: number; body: T; text: string }> => {
  const res = await fetch(new URL(path, globalThis.location.origin))
  const text = await res.text()
  let body: unknown = text
  try {
    body = JSON.parse(text)
  } catch {
    /* plain text */
  }
  return { status: res.status, body: body as T, text }
}

async function scanDay(day: string): Promise<SessionSummary[]> {
  const rows: SessionSummary[] = []
  for (let offset = 0; ; offset += 100) {
    const { body } = await get<SessionListResponse>(
      `/api/observability/session/list?start_time=${day}T00:00:00Z&limit=100&offset=${offset}`,
    )
    rows.push(...body.data.sessions)
    if (!body.data.pagination.has_next_page) break
  }
  return rows.filter((r) => r.start_time?.slice(0, 10) === day)
}

async function finopsDayTotal(day: string): Promise<number> {
  const { body } = await get<{ data: { hours: { spend_usd: number }[] } }>(
    `/api/observability/finops/spend-calendar/day?date=${day}`,
  )
  return body.data.hours.reduce((s, h) => s + h.spend_usd, 0)
}

describe('session list', () => {
  it('orders by created_at DESC, clamps the limit and pages with has_next_page', async () => {
    const a = await get<SessionListResponse>('/api/observability/session/list?limit=500')
    expect(a.body.data.sessions).toHaveLength(100)
    const starts = a.body.data.sessions.map((s) => Date.parse(s.start_time!))
    expect([...starts].sort((x, y) => y - x)).toEqual(starts)
    const d = await get<SessionListResponse>('/api/observability/session/list')
    expect(d.body.data.sessions).toHaveLength(25) // default page
    expect(d.body.data.pagination.has_next_page).toBe(true)
  })

  it('carries the raw agent name, and "" for the deleted agent', async () => {
    const rows = await scanDay(utcDate(new Date(now() - 86_400_000)))
    const names = new Set(rows.map((r) => r.agent_id))
    expect([...names].every((n) => n === '' || n.startsWith('seed-'))).toBe(true)
    const deleted = seed.agents.find((a) => a.deleted)!
    const data = observabilityData(seed)
    expect(data.sessions.some((s) => s.agent.id === deleted.id)).toBe(true)
  })
})

describe('invariants', () => {
  it('spike day: the session sum equals the TokenOps day total', async () => {
    const rows = await scanDay(seed.spikeDate)
    const sum = rows.reduce((s, r) => s + (r.cost_summary.total.cost ?? 0), 0)
    expect(sum).toBeCloseTo(await finopsDayTotal(seed.spikeDate), 4)
  })

  it('a non-spike day with workflow traffic diverges (chat sessions only)', async () => {
    const data = observabilityData(seed)
    const day = utcDate(data.nonChat[data.nonChat.length - 1].started_at)
    const rows = await scanDay(day)
    const sum = rows.reduce((s, r) => s + (r.cost_summary.total.cost ?? 0), 0)
    expect(sum).toBeLessThan(await finopsDayTotal(day))
  })

  it('a session costs the sum of its trace totals, and a trace the sum of its span costs', async () => {
    const rows = (await scanDay(utcDate(new Date(now() - 2 * 86_400_000)))).slice(0, 4)
    for (const r of rows) {
      const { body } = await get<SessionDetailResponse>(
        `/api/observability/session/${r.session_id}`,
      )
      let traceSum = 0
      for (const t of body.data.session.traces) {
        const td = (await get<TraceDetailResponse>(`/api/observability/trace/${t.trace_id}`)).body
          .data.trace
        traceSum += td.cost_summary.total.cost
        let spanSum = 0
        for (const s of flattenSpans(td)) {
          if (s.node.name === TRACE_TOTAL_SPAN) continue
          spanSum += (
            await get<SpanDetailResponse>(`/api/observability/span/${t.trace_id}/${s.node.span_id}`)
          ).body.data.span.cost_summary.total.cost
        }
        expect(spanSum).toBeCloseTo(td.cost_summary.total.cost, 6)
      }
      expect(traceSum).toBeCloseTo(r.cost_summary.total.cost ?? 0, 5)
    }
  })

  it('no session crosses UTC midnight, and fewer than 300 sessions start after the spike day', () => {
    const data = observabilityData(seed)
    for (const s of data.sessions)
      expect(new Set(s.traces.map((t) => utcDate(t.started_at))).size).toBe(1)
    const since = Date.parse(`${seed.spikeDate}T00:00:00Z`)
    expect(data.sessions.filter((s) => s.created >= since).length).toBeLessThan(300)
  })
})

describe('traces and spans', () => {
  it('the spike is PR-review retry storms: PR #481 is the costliest spike-day session, every trace failing', async () => {
    const rows = await scanDay(seed.spikeDate)
    const top = [...rows].sort(
      (a, b) => (b.cost_summary.total.cost ?? 0) - (a.cost_summary.total.cost ?? 0),
    )[0]
    expect(top.session_id).toBe(SHOWCASE_SESSION)
    expect(top.first_input).toMatch(/^Review PR #481/)
    const { body } = await get<SessionDetailResponse>(
      `/api/observability/session/${SHOWCASE_SESSION}`,
    )
    expect(body.data.session.traces.length).toBeGreaterThan(1)
    for (const t of body.data.session.traces.slice(0, 5)) {
      const td = (await get<TraceDetailResponse>(`/api/observability/trace/${t.trace_id}`)).body
        .data.trace
      expect(isTraceFailing(td)).toBe(true)
      expect(
        flattenSpans(td).filter((s) => s.node.name === 'tool.get_diff').length,
      ).toBeGreaterThanOrEqual(3)
    }
    // The showcase trace carries the whole story: 6 attempts and a 3-call cascade.
    const td = (
      await get<TraceDetailResponse>(
        `/api/observability/trace/${observabilityData(seed).showcaseTraceId}`,
      )
    ).body.data.trace
    const flat = flattenSpans(td)
    expect(flat.filter((s) => s.node.name === 'tool.get_diff')).toHaveLength(6)
    expect(flat.filter((s) => s.node.name === 'a2a.proxy')).toHaveLength(3)
    // Roots don't propagate the child errors (OTel rarely does).
    expect(td.root_spans.edges.every((e) => e.span.status_code !== 'ERROR')).toBe(true)
  })

  it('a retry that recovered is not a failure', async () => {
    const data = observabilityData(seed)
    const recovered = data.sessions
      .filter((s) => !data.storm.has(s.traces[0].trace_id))
      .flatMap((s) => s.traces)
      .map((t) => traceDetail(seed, t.trace_id)!)
      .find((td) => {
        const flat = flattenSpans(td)
        return flat.some((s) => s.node.status_code === 'ERROR') && !isTraceFailing(td)
      })
    expect(recovered).toBeTruthy()
  })

  it('spans is a tree of roots; span_lookup is flat and keyed by base64("Span:"+hex)', async () => {
    const t = observabilityData(seed).sessions[3].traces[0]
    const td = (await get<TraceDetailResponse>(`/api/observability/trace/${t.trace_id}`)).body.data
      .trace
    expect(td.spans).toHaveLength(1)
    expect(td.spans[0].children.length).toBeGreaterThan(0)
    for (const [key, node] of Object.entries(td.span_lookup)) {
      expect(key).toBe(node.id)
      expect(node.id).toBe(encodeSpanId(node.span_id))
      expect(node.span_id).toMatch(/^[0-9a-f]{16}$/)
      expect(['internal', 'server', 'client']).toContain(node.span_kind)
    }
    expect(Object.keys(td.span_lookup)).toHaveLength(td.num_spans)
  })

  it('span detail matches the hex id; the base64 id is a 404 like the server', async () => {
    const t = observabilityData(seed).sessions[3].traces[0]
    const td = (await get<TraceDetailResponse>(`/api/observability/trace/${t.trace_id}`)).body.data
      .trace
    const node = td.spans[0]
    expect((await get(`/api/observability/span/${t.trace_id}/${node.span_id}`)).status).toBe(200)
    const bad = await get(`/api/observability/span/${t.trace_id}/${encodeURIComponent(node.id)}`)
    expect(bad.status).toBe(404)
    expect(bad.text).toMatch(/not found/)
  })

  it('coding_agent.turn repeats the trace total; own-token sums exclude it', async () => {
    const id = observabilityData(seed).codingTraceId!
    expect(id).toBeTruthy()
    const td = (await get<TraceDetailResponse>(`/api/observability/trace/${id}`)).body.data.trace
    const flat = flattenSpans(td)
    const turn = flat.find((s) => s.node.name === TRACE_TOTAL_SPAN)!
    const own = flat.reduce((s, x) => s + ownTokens(x.node), 0)
    expect(turn.node.input_tokens + turn.node.output_tokens).toBe(own)
  })

  it('unknown session and trace are plain-text 404s', async () => {
    const s = await get('/api/observability/session/nope')
    expect(s.status).toBe(404)
    expect(s.text).toBe("session 'nope' not found")
    expect((await get('/api/observability/trace/nope')).status).toBe(404)
  })
})

describe('variants', () => {
  it('tempo-down: rows come back DB-only with null num_traces', async () => {
    configureMocks({ variant: 'tempo-down' })
    const { body } = await get<SessionListResponse>('/api/observability/session/list')
    expect(
      body.data.sessions.every((s) => s.num_traces === null && s.cost_summary.total.cost === null),
    ).toBe(true)
    expect(body.data.successful_agents).toBe(0)
  })

  it('trace-503 / trace-500 / empty / scan-fail', async () => {
    configureMocks({ variant: 'trace-503' })
    expect((await get(`/api/observability/session/${SHOWCASE_SESSION}`)).status).toBe(503)
    configureMocks({ variant: 'trace-500' })
    expect((await get(`/api/observability/session/${SHOWCASE_SESSION}`)).text).toBe(
      'internal error',
    )
    configureMocks({ variant: 'empty' })
    expect(
      (await get<SessionListResponse>('/api/observability/session/list')).body.data.sessions,
    ).toHaveLength(0)
    configureMocks({ variant: 'scan-fail' })
    expect((await get('/api/observability/session/list?offset=100')).status).toBe(500)
  })

  it('logs: array, SSE stream with a close event, 404 for the deleted agent', async () => {
    const ok = await get<unknown[]>('/api/observability/agents/seed-code-reviewer/logs')
    expect(ok.body).toHaveLength(30)
    const stream = await get('/api/observability/agents/seed-code-reviewer/logs/stream')
    expect(stream.text).toMatch(/^data: /)
    expect(stream.text).toMatch(/event: close/)
    const deleted = seed.agents.find((a) => a.deleted)!
    expect((await get(`/api/observability/agents/${deleted.name}/logs`)).status).toBe(404)
  })
})

describe('richer mock traces', () => {
  const data = observabilityData(seed)
  const normalTraces = data.sessions
    .filter((s) => s.agent.name !== 'seed-code-reviewer')
    .flatMap((s) => s.traces)
    .slice(0, 400)

  it('tool steps have unique names under the root (never a false ×N retry group)', () => {
    for (const t of normalTraces) {
      const spans = generateSpans(seed, t.trace_id)
      const root = spans[0]
      const steps = spans
        .filter((s) => s.parentHex === root.hex && s.name.startsWith('step.'))
        .map((s) => s.name)
      expect(new Set(steps).size).toBe(steps.length)
    }
  })

  it('a failing tool owns the error; its HTTP child carries the same status code', () => {
    let checked = 0
    for (const t of normalTraces) {
      const spans = generateSpans(seed, t.trace_id)
      for (const tool of spans.filter((s) => s.status === 'ERROR' && s.name.startsWith('tool.'))) {
        const child = spans.find((s) => s.parentHex === tool.hex && s.name === 'http.request')
        if (!child) continue // a failed first attempt that was retried has no HTTP child
        expect(child.status).toBe('UNSET')
        expect(child.attrs['http.response.status_code']).toBe(tool.attrs['http.status_code'])
        expect(
          tool.statusMessage.includes('permission denied')
            ? 403
            : tool.statusMessage.includes('503')
              ? 503
              : 500,
        ).toBe(tool.attrs['http.status_code'])
        checked++
      }
    }
    expect(checked).toBeGreaterThan(0)
  })

  it('LLM spans carry gen_ai usage attributes that match their token counts', () => {
    const spans = generateSpans(seed, normalTraces[0].trace_id).filter(
      (s) => s.model && s.name.startsWith('llm.'),
    )
    expect(spans.length).toBeGreaterThan(0)
    for (const s of spans) {
      expect(s.attrs['gen_ai.usage.input_tokens']).toBe(s.input)
      expect(s.attrs['gen_ai.usage.output_tokens']).toBe(s.output)
    }
  })
})
