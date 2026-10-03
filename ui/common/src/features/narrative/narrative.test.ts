import { describe, expect, it } from 'vitest'
import { encodeSpanId, findRetryLoops, flattenSpans } from '@/features/observability/spans'
import type { SpanNode, TraceDetail } from '@/features/observability/types'
import { spanDetail, traceDetail, observabilityData } from '@/mocks/observability'
import { generateSeed } from '@/mocks/seed'
import { findSpike, spikeSentence, tokenopsNarrative } from './tokenops'
import { narrativeText, traceNarrative, wasteCostSpans, wasteFetchable } from './trace'

const seed = generateSeed({ anchor: new Date('2026-03-20T15:00:00Z') })
const data = observabilityData(seed)

function showcase() {
  const td = traceDetail(seed, data.showcaseTraceId)!
  return { td, spans: flattenSpans(td) }
}

describe('trace narrative', () => {
  it('showcase: cause, money with retry waste in dollars, status, plus the no-backoff takeaway', () => {
    const { td, spans } = showcase()
    const [loop] = findRetryLoops(spans)
    expect(wasteFetchable(loop)).toBe(true)
    const costs = new Map(
      wasteCostSpans(loop).map((s) => [
        s.node.id,
        spanDetail(seed, td.id, s.node.span_id)!.cost_summary.total.cost,
      ]),
    )
    const n = traceNarrative({
      totalCost: td.cost_summary.total.cost,
      spans,
      agentName: () => 'QA Tester',
      wasteCosts: costs,
    })
    const text = narrativeText(n)
    expect(text).toHaveLength(3)
    expect(text[0]).toBe(
      'The planner retried `get_diff` 5 times after errors and called QA Tester 3 times through the proxy.',
    )
    expect(text[1]).toMatch(/^This trace cost \$\d+\.\d\d; retries cost \$\d+\.\d\d\.$/)
    const waste = [...costs.values()].reduce((a, b) => a + b, 0)
    expect(waste).toBeGreaterThan(0)
    expect(text[2]).toBe('It failed when the last attempt timed out at 30.0 s.')
    expect(n.takeaway?.text).toBe('Retry policy: 6 attempts, no backoff.')
    expect(n.details.some((d) => /% of the tokens$/.test(d.text))).toBe(true)
    // Every clause points at real spans (the highlight target).
    const ids = new Set(spans.map((s) => s.node.id))
    for (const clause of n.sentences.flat())
      for (const id of clause.spanIds) expect(ids.has(id)).toBe(true)
  })

  it('without SpanDetail costs (a failed fetch, or too many retries) the waste is worded in tokens', () => {
    const { td, spans } = showcase()
    const text = narrativeText(traceNarrative({ totalCost: td.cost_summary.total.cost, spans }))
    expect(text[1]).toMatch(/^This trace cost \$\d+\.\d\d; retries drove \d+% of the tokens\.$/)
  })

  it('a healthy trace says so in one sentence', () => {
    const healthy = data.sessions
      .flatMap((s) => s.traces)
      .find((t) => {
        const td = traceDetail(seed, t.trace_id)!
        return (
          !flattenSpans(td).some((s) => s.node.status_code === 'ERROR') &&
          t.trace_id !== data.codingTraceId
        )
      })!
    const td = traceDetail(seed, healthy.trace_id)!
    const n = traceNarrative({ totalCost: td.cost_summary.total.cost, spans: flattenSpans(td) })
    expect(n.healthy).toBe(true)
    expect(narrativeText(n)[0]).toMatch(
      /^This trace cost \$[\d.,]+ and finished in [\d.]+ (s|ms) with no errors\.$/,
    )
    expect(n.takeaway).toBeNull()
  })

  it('never returns more than 3 sentences', () => {
    for (const s of data.sessions.slice(0, 40)) {
      const td = traceDetail(seed, s.traces[0].trace_id)!
      expect(
        traceNarrative({ totalCost: td.cost_summary.total.cost, spans: flattenSpans(td) }).sentences
          .length,
      ).toBeLessThanOrEqual(3)
    }
  })
})

// Hand-built traces for the branches the seed doesn't reach: a lone error, a recovered retry with backoff, partial costs.
const node = (hex: string, o: Partial<SpanNode> = {}): SpanNode => ({
  id: encodeSpanId(hex),
  span_id: hex,
  name: 'x',
  span_kind: 'internal',
  status_code: 'UNSET',
  start_time: '2026-03-11T10:00:00.000Z',
  end_time: '2026-03-11T10:00:01.000Z',
  parent_id: null,
  latency_ms: 1000,
  token_count_total: 0,
  input_tokens: 0,
  output_tokens: 0,
  cache_read_tokens: 0,
  cache_creation_tokens: 0,
  model: null,
  operation: null,
  provider: null,
  span_annotation_summaries: [],
  children: [],
  ...o,
})

const at = (ms: number) => new Date(Date.parse('2026-03-11T10:00:00.000Z') + ms).toISOString()

function trace(nodes: SpanNode[]): Pick<TraceDetail, 'spans' | 'span_lookup'> {
  const byId = new Map(nodes.map((n) => [n.id, { ...n, children: [] as SpanNode[] }]))
  const roots: SpanNode[] = []
  for (const n of byId.values()) {
    if (n.parent_id && byId.has(n.parent_id)) byId.get(n.parent_id)!.children.push(n)
    else roots.push(n)
  }
  return {
    spans: roots,
    span_lookup: Object.fromEntries(nodes.map((n) => [n.id, { ...n, children: [] }])),
  }
}

const root = node('aaaaaaaaaaaaaaaa', { name: 'planner', latency_ms: 500 })

describe('trace narrative: edges', () => {
  it('a lone error (no retry) names the span, moves the cascade to details, and a long error reads as a timeout', () => {
    const bad = node('bbbbbbbbbbbbbbbb', {
      name: 'tool.fetch',
      parent_id: root.id,
      status_code: 'ERROR',
      latency_ms: 500,
      start_time: at(10),
    })
    const call = node('cccccccccccccccc', {
      name: 'a2a.proxy',
      parent_id: root.id,
      start_time: at(20),
      latency_ms: 100,
    })
    const n = traceNarrative({
      totalCost: 1.5,
      spans: flattenSpans(trace([root, bad, call])),
      agentName: () => undefined,
    })
    expect(narrativeText(n)).toEqual([
      '`fetch` returned an error.',
      'This trace cost $1.50.',
      'It ended with an error in `fetch`.',
    ])
    expect(n.details.map((d) => d.text)).toContain(
      'It called another agent 1 time through the proxy',
    )
    expect(n.healthy).toBe(false)
    expect(n.takeaway).toBeNull()

    const slow = node('dddddddddddddddd', {
      name: 'tool.fetch',
      parent_id: root.id,
      status_code: 'ERROR',
      latency_ms: 29_000,
    })
    expect(
      narrativeText(traceNarrative({ totalCost: 0, spans: flattenSpans(trace([root, slow])) }))[2],
    ).toBe('It failed when the last attempt timed out at 29.0 s.')

    // Healthy with calls to two different agents.
    const c1 = node('e0eeeeeeeeeeeeee', {
      name: 'a2a.proxy',
      parent_id: root.id,
      start_time: at(10),
      latency_ms: 10,
    })
    const c2 = node('e1eeeeeeeeeeeeee', {
      name: 'a2a.dispatch',
      parent_id: root.id,
      start_time: at(20),
      latency_ms: 10,
    })
    const names = new Map([
      [c1.id, 'QA Tester'],
      [c2.id, 'Code Reviewer'],
    ])
    const healthy = traceNarrative({
      totalCost: 2,
      spans: flattenSpans(trace([root, c1, c2])),
      agentName: (s) => names.get(s.node.id),
    })
    expect(healthy.healthy).toBe(true)
    expect(narrativeText(healthy)[1]).toBe('It called 2 agents 2 times through the proxy.')
    expect(healthy.details).toEqual([])
  })

  it('a recovered retry with backoff: singular wording, "recovered", no takeaway; partial or zero costs fall back to tokens', () => {
    const t0 = node('b0bbbbbbbbbbbbbb', {
      name: 'tool.get_diff',
      parent_id: root.id,
      status_code: 'ERROR',
      start_time: at(0),
      latency_ms: 100,
    })
    const replan = node('cccccccccccccccc', {
      name: 'llm.repair',
      parent_id: t0.id,
      model: 'm',
      start_time: at(30),
      latency_ms: 50,
      input_tokens: 100,
    })
    const t1 = node('b1bbbbbbbbbbbbbb', {
      name: 'tool.get_diff',
      parent_id: root.id,
      status_code: 'OK',
      start_time: at(1000),
      latency_ms: 100,
    })
    const other = node('dddddddddddddddd', {
      name: 'llm.answer',
      parent_id: root.id,
      model: 'm',
      start_time: at(5000),
      latency_ms: 1000,
      input_tokens: 300,
    })
    const spans = flattenSpans(trace([root, t0, replan, t1, other]))

    const tokens = traceNarrative({ totalCost: 1, spans, wasteCosts: new Map() })
    const text = narrativeText(tokens)
    expect(text[0]).toBe('The planner retried `get_diff` 1 time after 1 error.')
    expect(text[1]).toBe('This trace cost $1.00; retries drove 25% of the tokens.')
    expect(text[2]).toBe('It recovered and finished in 6.0 s.')
    expect(tokens.takeaway).toBeNull()

    // Every token-bearing waste span priced: dollars. A zero-dollar total still reads in tokens.
    expect(
      narrativeText(
        traceNarrative({ totalCost: 1, spans, wasteCosts: new Map([[replan.id, 0.42]]) }),
      )[1],
    ).toBe('This trace cost $1.00; retries cost $0.42.')
    expect(
      narrativeText(
        traceNarrative({ totalCost: 1, spans, wasteCosts: new Map([[replan.id, 0]]) }),
      )[1],
    ).toBe('This trace cost $1.00; retries drove 25% of the tokens.')
  })
})

describe('tokenops narrative', () => {
  const days = (xs: number[]) =>
    xs.map((spend, i) => ({ date: `2026-03-${String(i + 1).padStart(2, '0')}`, spend }))

  it('spend + change, driver, spike: at most 3 sentences', () => {
    const n = tokenopsNarrative({
      windowLabel: 'Last 30 days',
      total: 141.68,
      previous: 127.6,
      unpriced: false,
      rows: [
        { name: 'Code Reviewer', sharePct: 37.4 },
        { name: 'Support Bot', sharePct: 20 },
      ],
      days: days([4, 5, 4, 18.12, 5, 4]),
    })
    expect(n.sentences).toEqual([
      'In the last 30 days you spent $141.68, 11% more than the period before.',
      'Code Reviewer drove 37% of it.',
      'Spend peaked on Mar 4 at $18.12 (4.0× a typical day).',
    ])
  })

  it('"at least" with unpriced calls; no change clause when Compare is off; "new" when previous is 0', () => {
    expect(
      tokenopsNarrative({
        windowLabel: 'This month',
        total: 10,
        unpriced: true,
        rows: [],
        days: [],
      }).sentences[0],
    ).toBe('This month you spent at least $10.00.')
    expect(
      tokenopsNarrative({
        windowLabel: 'Last 7 days',
        total: 10,
        previous: 0,
        unpriced: false,
        rows: [],
        days: [],
      }).sentences[0],
    ).toBe('In the last 7 days you spent $10.00, all of it new this period.')
  })

  it('an agent filter names the agent and drops the driver sentence', () => {
    const n = tokenopsNarrative({
      windowLabel: 'Last 30 days',
      total: 50,
      previous: 50,
      unpriced: false,
      agentLabel: 'Code Reviewer',
      rows: [
        { name: 'A', sharePct: 90 },
        { name: 'B', sharePct: 10 },
      ],
      days: [],
    })
    expect(n.sentences).toEqual([
      'In the last 30 days Code Reviewer spent $50.00, about the same as the period before.',
    ])
  })

  it('no driver below a 20% share; no spike at or below 2× the median', () => {
    const n = tokenopsNarrative({
      windowLabel: 'Last 30 days',
      total: 1,
      unpriced: false,
      rows: [
        { name: 'A', sharePct: 19 },
        { name: 'B', sharePct: 18 },
      ],
      days: days([4, 4, 8, 4]),
    })
    expect(n.sentences).toHaveLength(1)
    expect(findSpike(days([4, 4, 8.01, 4]))?.date).toBe('2026-03-03')
    expect(findSpike(days([0, 0, 0]))).toBeNull()
  })

  it('the seed spike is found in the pinned window', () => {
    const byDay = new Map<string, number>()
    for (const t of seed.traces) {
      const d = t.started_at.slice(0, 10)
      if (d >= '2026-02-19') byDay.set(d, (byDay.get(d) ?? 0) + t.cost_usd)
    }
    const spike = findSpike([...byDay.entries()].map(([date, spend]) => ({ date, spend })))
    expect(spike?.date).toBe(seed.spikeDate)
  })
})

// TokenOps wording edges the seed doesn't reach.
describe('tokenops narrative: edges', () => {
  const days = (xs: number[]) =>
    xs.map((spend, i) => ({ date: `2026-03-${String(i + 1).padStart(2, '0')}`, spend }))

  it('month labels, a drop, zero-to-zero, a single row, the 20% boundary and idle windows', () => {
    expect(
      tokenopsNarrative({
        windowLabel: 'March 2026',
        total: 10,
        previous: 20,
        unpriced: false,
        rows: [],
        days: [],
      }).sentences,
    ).toEqual(['In March 2026 you spent $10.00, 50% less than the period before.'])
    expect(
      tokenopsNarrative({
        windowLabel: 'March 2026',
        total: 0,
        previous: 0,
        unpriced: false,
        rows: [],
        days: [],
      }).sentences,
    ).toEqual(['In March 2026 you spent $0.00.'])
    // One row is not a "driver", even at 100%.
    expect(
      tokenopsNarrative({
        windowLabel: 'Last 7 days',
        total: 5,
        unpriced: false,
        rows: [{ name: 'Solo', sharePct: 100 }],
        days: [],
      }).sentences,
    ).toHaveLength(1)
    // Exactly 20% counts, and the share is rounded for display.
    expect(
      tokenopsNarrative({
        windowLabel: 'Last 7 days',
        total: 5,
        unpriced: false,
        rows: [
          { name: 'A', sharePct: 19.6 },
          { name: 'B', sharePct: 10 },
        ],
        days: [],
      }).sentences,
    ).toHaveLength(1)
    expect(
      tokenopsNarrative({
        windowLabel: 'Last 7 days',
        total: 5,
        unpriced: false,
        rows: [
          { name: 'A', sharePct: 20 },
          { name: 'B', sharePct: 10 },
        ],
        days: [],
      }).sentences[1],
    ).toBe('A drove 20% of it.')

    expect(findSpike(days([1, 9]))).toBeNull()
    // A mostly idle window has no typical day to compare with.
    expect(findSpike(days([0, 0, 5]))).toBeNull()
    expect(spikeSentence({ date: '2026-03-04', spend: 18.12, factor: 4.03 })).toBe(
      'Spend peaked on Mar 4 at $18.12 (4.0× a typical day)',
    )
    const n = tokenopsNarrative({
      windowLabel: 'Last 30 days',
      total: 30,
      unpriced: false,
      rows: [],
      days: days([1, 1, 9, 1]),
    })
    expect(n.spike?.date).toBe('2026-03-03')
    expect(n.sentences.at(-1)).toMatch(/^Spend peaked on Mar 3/)
  })
})
