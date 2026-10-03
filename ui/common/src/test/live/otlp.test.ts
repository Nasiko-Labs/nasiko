/** The OTLP writer (scripts/lib/otlp.ts) against the attributes nasiko-server at ea233d20 reads back from Tempo. */
import { describe, expect, it } from 'vitest'
import {
  otlpPayloads,
  otlpSpans,
  seedTraceSessions,
  type OtlpSpan,
} from '../../../../scripts/lib/otlp.ts'
import { generateSeed } from '@/mocks/seed'
import { generateSpans, observabilityData } from '@/mocks/spanBuilder'

const anchor = new Date('2026-09-26T15:00:00Z')
const seed = generateSeed({ anchor })
const data = observabilityData(seed)
const get = (s: OtlpSpan, key: string) => {
  const v = s.attributes.find((a) => a.key === key)?.value
  return v ? Object.values(v)[0] : undefined
}
const int = (s: OtlpSpan, key: string) => Number(get(s, key) ?? 0)

describe('otlpSpans', () => {
  const session = data.byId.get(data.traceById.get(data.showcaseTraceId)!.session.session_id)!
  const trace = data.traceById.get(data.showcaseTraceId)!.trace
  const spans = otlpSpans(trace, session, generateSpans(seed, trace.trace_id))

  it('keeps the trace id and hex span ids, with an empty parent on the root only', () => {
    expect(
      spans.every((s) => s.traceId === trace.trace_id && /^[0-9a-f]{16}$/.test(s.spanId)),
    ).toBe(true)
    expect(spans.filter((s) => s.parentSpanId === '').map((s) => s.name)).toEqual(['planner'])
  })
  it('puts session.id and the conversation on the root span only', () => {
    const root = spans.find((s) => s.parentSpanId === '')!
    expect(get(root, 'session.id')).toBe(session.session_id)
    expect(JSON.parse(String(get(root, 'gen_ai.input.messages')))[0].parts[0].content).toBe(
      session.firstInput,
    )
    expect(spans.filter((s) => get(s, 'session.id') !== undefined)).toHaveLength(1)
  })
  it("writes usage the extractor reads, summing to the trace's tokens", () => {
    const sum = (key: string) => spans.reduce((a, s) => a + int(s, key), 0)
    expect(sum('gen_ai.usage.input_tokens')).toBe(trace.input_tokens)
    expect(sum('gen_ai.usage.output_tokens')).toBe(trace.output_tokens)
    expect(sum('gen_ai.usage.cache_read_input_tokens')).toBe(trace.cache_read_tokens)
    const llm = spans.filter((s) => get(s, 'gen_ai.operation.name') === 'chat')
    expect(
      llm.every(
        (s) =>
          get(s, 'gen_ai.request.model') === trace.model &&
          get(s, 'gen_ai.provider.name') === trace.provider &&
          get(s, 'nasiko.usage.prompt_convention') === 'exclusive',
      ),
    ).toBe(true)
  })
  it('uses OTLP enums: kind 1-3 and status 0-2, with the error message on failed spans', () => {
    expect(new Set(spans.map((s) => s.kind))).toEqual(new Set([1, 2, 3]))
    const failed = spans.filter((s) => s.status.code === 2)
    expect(failed.length).toBeGreaterThan(0)
    expect(
      failed.every((s) => s.status.message && get(s, 'error.message') === s.status.message),
    ).toBe(true)
  })
  it('writes nanosecond times as strings', () => {
    expect(spans[0]!.startTimeUnixNano).toMatch(/^\d{19}$/)
  })
})

describe('coding_agent.turn', () => {
  it('leaves usage off the root: the server sums the whole trace onto it', () => {
    const { trace, session } = data.traceById.get(data.codingTraceId!)!
    const spans = otlpSpans(trace, session, generateSpans(seed, trace.trace_id))
    const root = spans.find((s) => s.parentSpanId === '')!
    expect(root.name).toBe('coding_agent.turn')
    expect(get(root, 'gen_ai.usage.input_tokens')).toBeUndefined()
    expect(spans.reduce((a, s) => a + int(s, 'gen_ai.usage.input_tokens'), 0)).toBe(
      trace.input_tokens,
    )
  })
})

describe('otlpPayloads', () => {
  const since = anchor.getTime() - 2 * 86_400_000
  const sessions = seedTraceSessions(seed, since)
  const payloads = otlpPayloads(seed, sessions, 500)
  const all = payloads.flatMap((p) => p.resourceSpans)

  it('covers every trace of the sessions in the window, once', () => {
    const ids = new Set(
      all.flatMap((r) => r.scopeSpans.flatMap((s) => s.spans.map((x) => x.traceId))),
    )
    expect(ids).toEqual(new Set(sessions.flatMap((s) => s.traces.map((t) => t.trace_id))))
    expect(sessions.every((s) => s.created >= since)).toBe(true)
  })
  it('names the agent in the resource service.name, one resource per agent per payload', () => {
    for (const p of payloads) {
      const names = p.resourceSpans.map(
        (r) => r.resource.attributes.find((a) => a.key === 'service.name')?.value,
      )
      expect(new Set(names.map((n) => JSON.stringify(n))).size).toBe(names.length)
    }
    expect(
      all.every((r) =>
        String(Object.values(r.resource.attributes[0]!.value)[0]).startsWith('seed-'),
      ),
    ).toBe(true)
  })
  it('keeps a trace within one payload and payloads under the span cap', () => {
    for (const p of payloads)
      expect(
        p.resourceSpans.reduce((a, r) => a + r.scopeSpans[0]!.spans.length, 0),
      ).toBeLessThanOrEqual(500)
    const where = new Map<string, number>()
    payloads.forEach((p, i) => {
      for (const r of p.resourceSpans) {
        for (const s of r.scopeSpans[0]!.spans) {
          expect(where.get(s.traceId) ?? i).toBe(i)
          where.set(s.traceId, i)
        }
      }
    })
  })
})
