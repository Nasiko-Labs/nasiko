import { describe, expect, it } from 'vitest'
import {
  bySize,
  computeLanes,
  dayBounds,
  dedupeSessions,
  entryCost,
  foldDayScan,
  noTraceData,
  percentile,
  sessionDay,
  sortSessions,
  type Status,
} from './sessions'
import {
  classifySpan,
  defaultSpan,
  encodeSpanId,
  findRetryLoops,
  flattenAttributes,
  flattenSpans,
  groupKeyOf,
  groupKeysOf,
  isTraceFailing,
  tokenSplit,
  traceDurationMs,
  treeRows,
  unrecoveredErrors,
} from './spans'
import type { SessionSummary, SpanNode, TraceDetail, TraceEntry } from './types'

const row = (id: string, o: Partial<SessionSummary> = {}): SessionSummary => ({
  id,
  session_id: id,
  agent_id: 'seed-a',
  num_traces: 1,
  start_time: '2026-03-11T10:00:00.000Z',
  end_time: null,
  duration_ms: 1000,
  first_input: null,
  last_output: null,
  token_usage: { total: 10 },
  trace_latency_ms_p50: null,
  trace_latency_ms_p99: null,
  cost_summary: { total: { cost: 1 } },
  session_annotations: [],
  session_annotation_summaries: [],
  ...o,
})

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

const entry = (cost: unknown, tokens: number) =>
  ({
    root_span: { trace: { cost_summary: cost }, cumulative_token_count_total: tokens },
  }) as unknown as Pick<TraceEntry, 'root_span'>

describe('percentile (nearest-rank)', () => {
  it('takes rank ceil(q·n) of the sorted sample and ignores nulls', () => {
    expect(
      percentile([1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18, 19, 20]),
    ).toBe(19)
    expect(percentile([null, 5, undefined, 3])).toBe(5)
    expect(percentile([null, null])).toBeNull()
  })
})

describe('lanes', () => {
  const many = (n: number, f: (i: number) => Partial<SessionSummary>) =>
    Array.from({ length: n }, (_, i) => row(`s${i}`, f(i)))

  it('costly is strictly above p95: ties at the threshold stay out', () => {
    const rows = many(20, (i) => ({ cost_summary: { total: { cost: i < 19 ? 1 : 1 } } }))
    expect(computeLanes(rows, new Map()).costly.size).toBe(0)
    const rows2 = many(20, (i) => ({ cost_summary: { total: { cost: i === 19 ? 50 : i } } }))
    expect([...computeLanes(rows2, new Map()).costly]).toEqual(['s19'])
  })

  it('null cost or duration is excluded; an all-null page has no lanes', () => {
    const rows = many(20, () => ({ cost_summary: { total: { cost: null } }, duration_ms: null }))
    const l = computeLanes(rows, new Map())
    expect(l.costly.size + l.slow.size).toBe(0)
    expect(l.costP95).toBeNull()
  })

  it('slow uses the fleet p95 for an agent with fewer than 5 sessions', () => {
    const rows = [
      ...many(20, (i) => ({ agent_id: 'seed-busy', duration_ms: 100 + i })),
      row('rare', { agent_id: 'seed-rare', duration_ms: 500 }),
    ]
    expect(computeLanes(rows, new Map()).slow.has('rare')).toBe(true)
  })

  it('unknown-agent rows ("") never get a per-agent p95', () => {
    const rows = [
      ...many(6, (i) => ({ agent_id: '', duration_ms: 10 * i })),
      ...many(20, (i) => ({ agent_id: 'seed-b', duration_ms: 1000 + i })),
    ].map((r, i) => ({ ...r, session_id: `x${i}` }))
    // Fleet p95 is ~1018, so the unknown rows (≤ 50 ms) are not slow.
    expect([...computeLanes(rows, new Map()).slow].some((id) => Number(id.slice(1)) < 6)).toBe(
      false,
    )
  })

  it('ranked only with 20+ rows; failing and checked come from the status map', () => {
    expect(
      computeLanes(
        many(19, () => ({})),
        new Map(),
      ).ranked,
    ).toBe(false)
    const status = new Map<string, Status>([
      ['s0', 'failed'],
      ['s1', 'ok'],
      ['s2', 'checking'],
      ['s3', 'unknown'],
    ])
    const l = computeLanes(
      many(20, () => ({})),
      status,
    )
    expect(l.ranked).toBe(true)
    expect([...l.failing]).toEqual(['s0'])
    expect(l.checked).toBe(2)
  })
})

describe('sessions', () => {
  it('sorts by cost with nulls last, and by time newest first', () => {
    const rows = [
      row('a', { cost_summary: { total: { cost: null } } }),
      row('b', { cost_summary: { total: { cost: 2 } } }),
      row('c', { cost_summary: { total: { cost: 5 } } }),
    ]
    expect(sortSessions(rows, 'cost').map((r) => r.session_id)).toEqual(['c', 'b', 'a'])
    const t = [
      row('old', { start_time: at(0) }),
      row('new', { start_time: at(1000) }),
      row('none', { start_time: null }),
    ]
    expect(sortSessions(t, 'time').map((r) => r.session_id)).toEqual(['new', 'old', 'none'])
  })

  it('day is the UTC day of start_time; null start is no day', () => {
    expect(sessionDay(row('a', { start_time: '2026-03-11T23:59:59.000Z' }))).toBe('2026-03-11')
    expect(sessionDay(row('a', { start_time: null }))).toBeNull()
  })

  it('a row created before midnight but starting after it lands on the next day', () => {
    // The server filters on created_at; the client buckets on start_time (tuning.ts DAY_KEY_NOTE).
    const late = row('late', { start_time: '2026-03-12T00:01:00.000Z' })
    const scan = foldDayScan('2026-03-11', [{ sessions: [late], hasNextPage: false }], 3)
    expect(scan.rows).toHaveLength(0)
    expect(
      foldDayScan('2026-03-12', [{ sessions: [late], hasNextPage: false }], 3).rows,
    ).toHaveLength(1)
  })

  it('noTraceData only when rows exist and all lack num_traces', () => {
    expect(noTraceData([])).toBe(false)
    expect(noTraceData([row('a', { num_traces: null })])).toBe(true)
    expect(noTraceData([row('a', { num_traces: null }), row('b')])).toBe(false)
  })
})

describe('day scan', () => {
  const day = '2026-03-11'
  const newer = (i: number) =>
    row(`n${i}`, { start_time: `2026-03-15T10:00:${String(i % 60).padStart(2, '0')}.000Z` })
  const onDay = (i: number) => row(`d${i}`, { start_time: `2026-03-11T1${i % 10}:00:00.000Z` })

  it('skips rows newer than the day, dedupes, and completes when the server runs out', () => {
    const pages = [
      { sessions: [newer(1), newer(2), onDay(1)], hasNextPage: true },
      { sessions: [onDay(1), onDay(2)], hasNextPage: false },
    ]
    const s = foldDayScan(day, pages, 3)
    expect(s.rows.map((r) => r.session_id)).toEqual(['d1', 'd2'])
    expect(s.complete).toBe(true)
    expect(s.capped).toBe(false)
    expect(s.scanned).toBe(4)
  })

  it('is capped at the page limit; a cap with none of the day is "missed"', () => {
    const pages = Array.from({ length: 3 }, (_, p) => ({
      sessions: [newer(p * 2), newer(p * 2 + 1)],
      hasNextPage: true,
    }))
    const s = foldDayScan(day, pages, 3)
    expect(s.capped).toBe(true)
    expect(s.missedDay).toBe(true)
  })

  it('completes early once a row older than the day shows up', () => {
    const older = row('o', { start_time: '2026-03-10T09:00:00.000Z' })
    expect(foldDayScan(day, [{ sessions: [onDay(1), older], hasNextPage: true }], 3).complete).toBe(
      true,
    )
  })
})

describe('spans', () => {
  it('classifySpan: llm by model/provider, agent by a2a name, tool by name/operation, planner for roots', () => {
    expect(classifySpan(node('1', { model: 'gpt-4o' }))).toBe('llm')
    expect(classifySpan(node('1', { provider: 'openai' }))).toBe('llm')
    expect(classifySpan(node('1', { name: 'a2a.proxy', parent_id: 'p' }))).toBe('agent')
    expect(classifySpan(node('1', { name: 'tool.get_diff', parent_id: 'p' }))).toBe('tool')
    expect(
      classifySpan(node('1', { name: 'fetch', operation: 'execute_tool', parent_id: 'p' })),
    ).toBe('tool')
    expect(classifySpan(node('1', { name: 'planner' }))).toBe('planner')
    expect(
      classifySpan(node('1', { name: 'work', parent_id: 'p', span_kind: 'internal' }), true),
    ).toBe('planner')
    expect(classifySpan(node('1', { name: 'misc', parent_id: 'p', span_kind: 'server' }))).toBe(
      'other',
    )
  })

  it('flattens the tree depth-first in start order and appends lookup-only nodes as roots', () => {
    const root = node('aaaaaaaaaaaaaaaa', { name: 'planner' })
    const late = node('bbbbbbbbbbbbbbbb', { parent_id: root.id, start_time: at(500) })
    const early = node('cccccccccccccccc', { parent_id: root.id, start_time: at(100) })
    const t = trace([root, late, early])
    const orphan = node('dddddddddddddddd', { name: 'orphan' })
    t.span_lookup[orphan.id] = orphan
    const flat = flattenSpans(t)
    expect(flat.map((s) => s.node.span_id[0])).toEqual(['a', 'c', 'b', 'd'])
    expect(flat.map((s) => s.depth)).toEqual([0, 1, 1, 0])
    expect(flat[1].startMs).toBe(100)
  })

  it('ids with "/", "+" and "=" survive the tree and a URL round trip', () => {
    // base64("Span:" + 16 hex) is always URL-safe (21 bytes, hex alphabet), but the tree must
    // not depend on that: link by arbitrary id strings, and URL-encode ids anyway.
    expect(encodeSpanId('0123456789abcdef')).toMatch(/^[A-Za-z0-9]+$/)
    const odd = 'U3Bhbj+/x=='
    const root = { ...node('aaaaaaaaaaaaaaaa', { name: 'planner' }), id: odd }
    const child = node('eeeeeeeeeeeeeeee', { parent_id: odd, start_time: at(10) })
    const t = trace([root, child])
    expect(flattenSpans(t).map((s) => s.depth)).toEqual([0, 1])
    expect(decodeURIComponent(encodeURIComponent(odd))).toBe(odd)
  })

  it('failing = any span in error, not just the root', () => {
    const root = node('aaaaaaaaaaaaaaaa', { name: 'planner' })
    const bad = node('bbbbbbbbbbbbbbbb', { parent_id: root.id, status_code: 'ERROR' })
    expect(isTraceFailing(trace([root, bad]))).toBe(true)
    expect(isTraceFailing(trace([root]))).toBe(false)
  })

  it('token split excludes coding_agent.turn (it repeats the trace total)', () => {
    const turn = node('aaaaaaaaaaaaaaaa', {
      name: 'coding_agent.turn',
      input_tokens: 300,
      output_tokens: 30,
    })
    const l1 = node('bbbbbbbbbbbbbbbb', {
      parent_id: turn.id,
      model: 'm',
      input_tokens: 100,
      output_tokens: 10,
    })
    const l2 = node('cccccccccccccccc', {
      parent_id: turn.id,
      model: 'm',
      input_tokens: 200,
      output_tokens: 20,
      start_time: at(10),
    })
    const split = tokenSplit(flattenSpans(trace([turn, l1, l2])))
    expect(split.total).toBe(330)
    expect(split.byClass.llm).toBe(330)
  })

  it('finds a retry loop with its re-plans, and picks the latest failing span by default', () => {
    const root = node('aaaaaaaaaaaaaaaa', { name: 'planner' })
    const tries = [0, 1, 2].map((i) =>
      node(`b${i}bbbbbbbbbbbbbb`, {
        name: 'tool.get_diff',
        parent_id: root.id,
        status_code: 'ERROR',
        start_time: at(i * 1000),
        latency_ms: 980,
      }),
    )
    const fix = node('cccccccccccccccc', {
      name: 'llm.repair_args',
      parent_id: tries[0].id,
      model: 'm',
      start_time: at(300),
      input_tokens: 50,
    })
    const other = node('dddddddddddddddd', {
      name: 'llm.other',
      parent_id: root.id,
      model: 'm',
      start_time: at(9000),
      input_tokens: 900,
    })
    const flat = flattenSpans(trace([root, ...tries, fix, other]))
    const [loop] = findRetryLoops(flat)
    expect(loop.attempts).toHaveLength(3)
    expect(loop.failures).toBe(3)
    expect(loop.maxGapMs).toBe(20)
    expect(loop.between.map((s) => s.node.name)).toEqual(['llm.repair_args'])
    expect(defaultSpan(flat)?.node.span_id).toBe(tries[2].span_id)
  })

  it('default span falls back to most tokens, then slowest', () => {
    const root = node('aaaaaaaaaaaaaaaa', { name: 'planner', latency_ms: 5000 })
    const big = node('bbbbbbbbbbbbbbbb', { parent_id: root.id, model: 'm', input_tokens: 900 })
    expect(defaultSpan(flattenSpans(trace([root, big])))?.node.span_id).toBe(big.span_id)
    expect(defaultSpan(flattenSpans(trace([root])))?.node.span_id).toBe(root.span_id)
  })

  it('groups same-name siblings as ×N and expands them into numbered attempts', () => {
    const root = node('aaaaaaaaaaaaaaaa', { name: 'planner' })
    const tries = [0, 1].map((i) =>
      node(`b${i}bbbbbbbbbbbbbb`, {
        name: 'tool.get_diff',
        parent_id: root.id,
        status_code: i ? 'ERROR' : 'OK',
        start_time: at(i * 100),
      }),
    )
    const flat = flattenSpans(trace([root, ...tries]))
    const key = groupKeyOf(flat, tries[1].id)!
    const closed = treeRows(flat, new Set(), new Set())
    expect(closed.map((r) => r.kind)).toEqual(['span', 'group'])
    const g = closed[1]
    expect(g.kind === 'group' && g.members.length === 2 && g.failures === 1).toBe(true)
    const open = treeRows(flat, new Set([key]), new Set())
    expect(
      open
        .filter((r) => r.kind === 'span' && r.attempt)
        .map((r) => r.kind === 'span' && r.attempt?.index),
    ).toEqual([1, 2])
    // Collapsing the root hides everything below it.
    expect(treeRows(flat, new Set(), new Set([root.id]))).toHaveLength(1)
  })
})

describe('flattenAttributes', () => {
  it("flattens the server's nested attributes to dotted keys (service.rs unflatten_attrs)", () => {
    expect(
      flattenAttributes({
        gen_ai: { operation: { name: 'chat' }, usage: { input_tokens: 5 } },
        agent: { id: 'a-1' },
        session: { id: 's' },
      }),
    ).toEqual({
      'gen_ai.operation.name': 'chat',
      'gen_ai.usage.input_tokens': 5,
      'agent.id': 'a-1',
      'session.id': 's',
    })
  })
  it('reads already-flat attributes unchanged, and keeps arrays, nulls and empty objects as leaves', () => {
    expect(flattenAttributes({ 'agent.id': 'a-1', tags: ['x'], none: null, empty: {} })).toEqual({
      'agent.id': 'a-1',
      tags: ['x'],
      none: null,
      empty: {},
    })
  })
  it('treats a missing or non-object value as no attributes', () => {
    expect(flattenAttributes(undefined)).toEqual({})
    expect(flattenAttributes('x')).toEqual({})
  })
})

describe('span derivations: degenerate input', () => {
  it('empty traces, junk timestamps, missing latency, negative latency and self-cycles never throw or go negative', () => {
    const empty = flattenSpans({ spans: [], span_lookup: {} })
    expect(empty).toEqual([])
    expect(traceDurationMs(empty)).toBe(0)
    expect(defaultSpan(empty)).toBeUndefined()
    expect(isTraceFailing({ spans: [], span_lookup: {} })).toBe(false)

    // Every timestamp unparseable: the origin falls back to 0 and each span starts at 0.
    const junk = flattenSpans(
      trace([
        node('aaaaaaaaaaaaaaaa', { start_time: 'not a date', end_time: null, latency_ms: null }),
      ]),
    )
    expect(junk[0]).toMatchObject({ startMs: 0, durationMs: 0 })

    // No latency_ms: duration comes from end − start. A negative latency clamps to 0.
    const root = node('aaaaaaaaaaaaaaaa', { name: 'planner', latency_ms: null, end_time: at(2500) })
    const neg = node('bbbbbbbbbbbbbbbb', {
      parent_id: root.id,
      latency_ms: -40,
      start_time: at(100),
    })
    const flat = flattenSpans(trace([root, neg]))
    expect(flat.map((s) => s.durationMs)).toEqual([2500, 0])
    expect(traceDurationMs(flat)).toBe(2500)

    // A node listed as its own child is visited once.
    const loop = node('cccccccccccccccc', { name: 'planner' })
    loop.children = [loop]
    expect(flattenSpans({ spans: [loop], span_lookup: {} })).toHaveLength(1)

    // Unknown ids and collapsing a leaf are no-ops.
    expect(groupKeyOf(flat, 'nope')).toBeUndefined()
    expect(treeRows(flat, new Set(), new Set([neg.id]))).toHaveLength(2)
  })

  it('a retry that later succeeded is recovered; a failure only on the last attempt is not a retry loop', () => {
    const root = node('aaaaaaaaaaaaaaaa', { name: 'planner' })
    const fail = node('b0bbbbbbbbbbbbbb', {
      name: 'tool.fetch',
      parent_id: root.id,
      status_code: 'ERROR',
      start_time: at(0),
      latency_ms: 100,
    })
    const ok = node('b1bbbbbbbbbbbbbb', {
      name: 'tool.fetch',
      parent_id: root.id,
      status_code: 'OK',
      start_time: at(1000),
      latency_ms: 100,
    })
    const recovered = flattenSpans(trace([root, fail, ok]))
    expect(unrecoveredErrors(recovered)).toEqual([])
    expect(isTraceFailing(trace([root, fail, ok]))).toBe(false)
    const [loop] = findRetryLoops(recovered)
    expect(loop).toMatchObject({ name: 'tool.fetch', failures: 1, maxGapMs: 900 })
    // With no unrecovered error, the default span still points at the (recovered) error.
    expect(defaultSpan(recovered)?.node.span_id).toBe(fail.span_id)

    // Success first, then an error: nothing recovered it, and it is not a retry loop.
    const late = node('b2bbbbbbbbbbbbbb', {
      name: 'tool.fetch',
      parent_id: root.id,
      status_code: 'ERROR',
      start_time: at(2000),
    })
    const first = node('b3bbbbbbbbbbbbbb', {
      name: 'tool.fetch',
      parent_id: root.id,
      status_code: 'OK',
      start_time: at(0),
    })
    const failing = flattenSpans(trace([root, first, late]))
    expect(unrecoveredErrors(failing).map((s) => s.node.span_id)).toEqual([late.span_id])
    expect(findRetryLoops(failing)).toEqual([])
    // Same-name siblings that all succeeded are not a loop either.
    expect(findRetryLoops(flattenSpans(trace([root, first, ok])))).toEqual([])
  })
})

describe('session derivations: junk from the wire', () => {
  it('percentile, entryCost, bySize, dedupe and the day scan tolerate missing and non-numeric values', () => {
    expect(percentile([Number.NaN, Number.POSITIVE_INFINITY, 3, null])).toBe(3)
    expect(percentile([7], 0)).toBe(7)
    expect(percentile([])).toBeNull()

    expect(entryCost(entry({ total: { cost: 2.5 } }, 0))).toBe(2.5)
    expect(entryCost(entry({ total: { cost: '2.5' } }, 0))).toBeNull()
    expect(entryCost(entry({ total: { cost: Number.NaN } }, 0))).toBeNull()
    expect(entryCost(entry(null, 0))).toBeNull()

    // Cost first; equal or missing cost falls back to tokens.
    const cheapSmall = entry({ total: { cost: 1 } }, 10)
    const cheapBig = entry({ total: { cost: 1 } }, 50)
    const dear = entry({ total: { cost: 3 } }, 0)
    expect(bySize([cheapSmall, cheapBig])).toEqual([cheapBig, cheapSmall])
    expect(bySize([cheapSmall, dear])).toEqual([dear, cheapSmall])
    const unpricedSmall = entry(null, 5)
    const unpricedBig = entry(null, 9)
    expect(bySize([unpricedSmall, unpricedBig])).toEqual([unpricedBig, unpricedSmall])

    const a1 = row('a', { duration_ms: 1 })
    expect(dedupeSessions([a1, row('b'), row('a', { duration_ms: 2 })])).toEqual([a1, row('b')])

    expect(sessionDay(row('x', { start_time: 'garbage' }))).toBeNull()
    expect(dayBounds('2026-03-11').end.toISOString()).toBe('2026-03-12T00:00:00.000Z')

    // No pages yet: nothing to scan, and nothing capped.
    expect(foldDayScan('2026-03-11', [], 3)).toMatchObject({
      rows: [],
      scanned: 0,
      pages: 0,
      complete: true,
      capped: false,
      missedDay: false,
    })
    // Rows with an unparseable start never land on the day.
    expect(
      foldDayScan(
        '2026-03-11',
        [{ sessions: [row('j', { start_time: 'garbage' })], hasNextPage: false }],
        3,
      ).rows,
    ).toEqual([])
  })
})

// Regressions from the /ship adversarial review (2026-09-26): overlapping same-name spans are not recoveries,
// expanded groups keep subtrees, and deep links open ancestor groups.
describe('recovery needs a retry that starts after the failure ended', () => {
  const root = node('aa', { name: 'planner', latency_ms: 5000, end_time: at(5000) })
  it('an overlapping parallel call with the same name does not hide the failure', () => {
    const failed = node('b1', {
      name: 'tool.fetch',
      parent_id: root.id,
      status_code: 'ERROR',
      start_time: at(0),
      latency_ms: 2000,
    })
    const parallel = node('b2', {
      name: 'tool.fetch',
      parent_id: root.id,
      status_code: 'OK',
      start_time: at(500),
      latency_ms: 2000,
    })
    expect(isTraceFailing(trace([root, failed, parallel]))).toBe(true)
  })
  it('a retry after the failed attempt ended still counts as recovered', () => {
    const failed = node('b1', {
      name: 'tool.fetch',
      parent_id: root.id,
      status_code: 'ERROR',
      start_time: at(0),
      latency_ms: 1000,
    })
    const retry = node('b2', {
      name: 'tool.fetch',
      parent_id: root.id,
      status_code: 'OK',
      start_time: at(1000),
      latency_ms: 1000,
    })
    expect(isTraceFailing(trace([root, failed, retry]))).toBe(false)
  })

  it('a retry starting 1 ms before the rounded end of the failure still recovers it', () => {
    const r = node('aa', { name: 'planner', latency_ms: 5000 })
    const failed = node('b1', {
      name: 'tool.fetch',
      parent_id: r.id,
      status_code: 'ERROR',
      start_time: at(0),
      latency_ms: 1001,
    })
    const retry = node('b2', {
      name: 'tool.fetch',
      parent_id: r.id,
      status_code: 'OK',
      start_time: at(1000),
      latency_ms: 500,
    })
    expect(isTraceFailing(trace([r, failed, retry]))).toBe(false)
  })
})

describe('expanded groups keep each member subtree', () => {
  const root = node('aa', { name: 'planner', latency_ms: 5000 })
  const a1 = node('c1', { name: 'tool.run', parent_id: root.id, start_time: at(0) })
  const a2 = node('c2', { name: 'tool.run', parent_id: root.id, start_time: at(2000) })
  const child = node('d1', {
    name: 'llm.repair_args',
    parent_id: a2.id,
    start_time: at(2100),
    model: 'gpt-4o',
  })
  const spans = flattenSpans(trace([root, a1, a2, child]))
  const key = `${root.id}|tool.run`

  it('the child LLM span of an attempt appears when its group is expanded', () => {
    const rows = treeRows(spans, new Set([key]), new Set())
    const names = rows.map((r) => (r.kind === 'span' ? r.span.node.name : `group:${r.name}`))
    expect(names).toEqual(['planner', 'group:tool.run', 'tool.run', 'tool.run', 'llm.repair_args'])
  })
  it('a deep link to that child opens the ancestor group', () => {
    expect(groupKeysOf(spans, child.id)).toEqual([key])
  })

  it('a parent cycle in the data does not hang the ancestor walk', () => {
    const selfParent = node('f1', { name: 'loop', parent_id: encodeSpanId('f1') })
    expect(groupKeysOf(flattenSpans(trace([selfParent])), selfParent.id)).toEqual([])
  })
})

describe('a span collapsed inside an expanded group', () => {
  const root = node('aa', { name: 'planner', latency_ms: 5000 })
  const a1 = node('c1', { name: 'tool.run', parent_id: root.id, start_time: at(0) })
  const a2 = node('c2', { name: 'tool.run', parent_id: root.id, start_time: at(2000) })
  const mid = node('d1', { name: 'llm.repair_args', parent_id: a2.id, start_time: at(2100) })
  const leaf = node('e1', { name: 'http.call', parent_id: mid.id, start_time: at(2200) })
  const spans = flattenSpans(trace([root, a1, a2, mid, leaf]))
  const key = `${root.id}|tool.run`

  it('a span collapsed inside an expanded group hides its own children', () => {
    const names = (collapsed: Set<string>) =>
      treeRows(spans, new Set([key]), collapsed).map((r) =>
        r.kind === 'span' ? r.span.node.name : `group:${r.name}`,
      )
    expect(names(new Set())).toContain('http.call')
    expect(names(new Set([mid.id]))).not.toContain('http.call')
  })
})
