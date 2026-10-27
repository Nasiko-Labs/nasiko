/**
 * Flows pure logic (plans/feat-flows.md; eng review A3, A4, A5, O3, O4, O5, O6): the kind rule, calls from steps and
 * spans, the merge, lanes, the shown status and the trace re-read schedule.
 */
import { describe, expect, it } from 'vitest'
import { flattenSpans, type FlatSpan } from '@nasiko/features/observability/spans'
import type { SpanNode } from '@nasiko/features/observability/types'
import { admit, endedAt, isGone, spanRereadMs, traceRereadMs, traceTiming } from './api'
import { callsFromSpans, callsFromSteps, type Call } from './calls'
import { criticalPath } from './critical'
import { copy, own, reasonFor, stepStatusLabel } from './copy'
import { flowKind } from './kind'
import { moveFrom } from './keys'
import { flowNarrative } from '@nasiko/features/narrative/flows'
import { fmtDuration, fmtWait, hasWholeSeconds } from './precision'
import { buildAxis, curve, fanOuts, position, ticks, BREAK_PX } from './timeline'
import { buildLanes } from './lanes'
import { buckets, filterRows, MAX_BUCKETS, percentiles } from './list'
import { mergeCalls } from './merge'
import { flowState, isWorking, lastActivityMs, pollMs } from './status'
import {
  FINISHING_MAX_MS,
  LANE_CAP,
  PAUSED_POLL_MS,
  RUNNING_POLL_MS,
  STUCK_AFTER_MS,
  STUCK_POLL_MS,
  SUBROW_CAP,
  TRACE_FINAL_READ_MS,
  TRACE_REREAD_MS,
} from './tuning'
import { ApiError } from '@nasiko/data-access/client'
import { flowSession, type Flow, type FlowStep } from './types'

const T = Date.parse('2026-10-06T09:00:00Z')
const iso = (ms: number) => new Date(T + ms).toISOString()

const call = (over: Partial<Call> & Pick<Call, 'key' | 'startMs'>): Call => ({
  agentId: 'a',
  agentName: 'alpha',
  endMs: over.startMs + 500,
  status: 'completed',
  source: 'recorded',
  stepId: null,
  spanId: null,
  parentKey: null,
  input: null,
  output: null,
  error: null,
  tokens: null,
  exactEnd: true,
  maybeSame: false,
  waitStartMs: null,
  ...over,
})
const step = (key: string, startMs: number, over: Partial<Call> = {}) =>
  call({ key: `step:${key}`, startMs, stepId: key, ...over })
const span = (hex: string, startMs: number, over: Partial<Call> = {}) =>
  call({ key: `span:${hex}`, startMs, spanId: hex, source: 'trace', ...over })

const flow = (over: Partial<Flow> = {}): Flow => ({
  flow_id: 'f1',
  root_agent_name: 'orchestrator',
  status: 'completed',
  created_at: iso(0),
  completed_at: iso(4_000),
  duration_ms: 4_000,
  ...over,
})

describe('flowKind (A3)', () => {
  it('reads the Orchestrator root, the MAF mode and everything else as direct', () => {
    expect(flowKind({ root_agent_name: 'orchestrator' })).toBe('orchestrated')
    expect(flowKind({ root_agent_name: 'writer', metadata: { mode: 'free_flowing' } })).toBe(
      'workflow',
    )
    expect(flowKind({ root_agent_name: 'writer', metadata: { context_id: 's1' } })).toBe('direct')
    expect(flowKind({ root_agent_name: null, metadata: 'not an object' })).toBe('direct')
  })
})

describe('callsFromSteps', () => {
  const base: FlowStep = {
    id: 's1',
    step_order: 1,
    depth: 1,
    agent_id: 'a',
    agent_name: 'alpha',
    status: 'completed',
    created_at: iso(100),
    completed_at: iso(1_900),
    latency_ms: 1_000,
  }
  it('uses completed_at (exact) over the truncated latency', () => {
    const [c] = callsFromSteps([base])
    expect(c).toMatchObject({ startMs: T + 100, endMs: T + 1_900, exactEnd: true, key: 'step:s1' })
  })
  it('falls back to whole-second latency, and an open step has no end', () => {
    const [closed, open] = callsFromSteps([
      { ...base, completed_at: null },
      { ...base, id: 's2', status: 'running', completed_at: null },
    ])
    expect(closed).toMatchObject({ endMs: T + 1_100, exactEnd: false })
    expect(open?.endMs).toBeNull()
  })
  it('links a proposed parent_step_id (FL-3)', () => {
    expect(callsFromSteps([{ ...base, parent_step_id: 'p' }])[0]?.parentKey).toBe('step:p')
  })
})

const node = (
  hex: string,
  parent: string | null,
  start: number,
  end: number | null,
  name = 'a2a.proxy',
): SpanNode => ({
  id: `b64-${hex}`,
  span_id: hex,
  name,
  span_kind: 'server',
  status_code: 'OK',
  start_time: iso(start),
  end_time: end === null ? null : iso(end),
  parent_id: parent ? `b64-${parent}` : null,
  latency_ms: end === null ? null : end - start,
  token_count_total: 0,
  input_tokens: 0,
  output_tokens: 0,
  cache_read_tokens: 0,
  cache_creation_tokens: 0,
  span_annotation_summaries: [],
  children: [],
})
const flat = (...nodes: SpanNode[]): FlatSpan[] =>
  flattenSpans({ spans: [], span_lookup: Object.fromEntries(nodes.map((n) => [n.id, n])) })

describe('callsFromSpans (A1)', () => {
  const spans = flat(
    node('d', null, 0, 5_000, 'a2a.dispatch'),
    node('r', 'd', 100, 4_000),
    node('x', 'r', 300, 1_200),
    node('y', 'r', 320, null),
    node('z', 'r', 400, 900),
  )
  const targets = new Map([
    ['b64-r', 'research'],
    ['b64-x', 'search'],
    ['b64-y', 'search'],
  ])
  const calls = callsFromSpans(spans, targets, (id) => `${id}-name`)

  it('keeps proxy spans with a known target, parented by the nearest proxy above', () => {
    expect(calls.map((c) => c.key)).toEqual(['span:r', 'span:x', 'span:y'])
    expect(calls[0]?.parentKey).toBeNull()
    expect(calls[1]).toMatchObject({
      parentKey: 'span:r',
      agentName: 'search-name',
      source: 'trace',
    })
  })
  it('reads an open span as running', () => {
    expect(calls[2]).toMatchObject({ endMs: null, status: 'running' })
  })
})

describe('mergeCalls (A5, O3)', () => {
  it('pairs a step and a span of the same agent within a second: recorded status, span timing', () => {
    const out = mergeCalls(
      [step('s', 100, { status: 'failed', output: 'boom', endMs: 2_000, exactEnd: false })],
      [span('p', 140, { endMs: 1_950 })],
    )
    expect(out).toHaveLength(1)
    expect(out[0]).toMatchObject({
      key: 'step:s',
      status: 'failed',
      output: 'boom',
      startMs: 140,
      endMs: 1_950,
      spanId: 'p',
      source: 'both',
      exactEnd: true,
    })
  })
  it('never matches another agent, or a start more than a second away', () => {
    const out = mergeCalls([step('s', 100)], [span('p', 120, { agentId: 'b' }), span('q', 1_200)])
    expect(out.map((c) => c.source)).toEqual(['recorded', 'trace', 'trace'])
  })
  it('O3a: two same-agent calls never pair by start order (start order can’t prove identity)', () => {
    // Steps A/B at 0/600, spans at 50/650: equal counts and a clear order, still not proof of which is which.
    const out = mergeCalls([step('s1', 0), step('s2', 600)], [span('p1', 50), span('p2', 650)])
    expect(out).toHaveLength(4)
    expect(out.every((c) => c.maybeSame && c.source !== 'both')).toBe(true)
  })
  it('keeps an ambiguous group apart and marks every call in it (unequal counts)', () => {
    const out = mergeCalls([step('s1', 0)], [span('p1', 50), span('p2', 400)])
    expect(out).toHaveLength(3)
    expect(out.every((c) => c.maybeSame)).toBe(true)
  })
  it('keeps tied starts apart', () => {
    const out = mergeCalls([step('s1', 0), step('s2', 0)], [span('p1', 10), span('p2', 20)])
    expect(out).toHaveLength(4)
    expect(out.every((c) => c.maybeSame)).toBe(true)
  })
  it("re-points a trace call's parent at the step it merged into", () => {
    const out = mergeCalls(
      [step('s', 100)],
      [span('p', 120), span('child', 300, { agentId: 'c', parentKey: 'span:p' })],
    )
    expect(out.find((c) => c.key === 'span:child')?.parentKey).toBe('step:s')
  })
  it('leaves a step without an agent id unmerged', () => {
    const out = mergeCalls([step('s', 100, { agentId: null })], [span('p', 120)])
    expect(out).toHaveLength(2)
  })
})

describe('buildLanes (F27, O6)', () => {
  it('orders lanes by first call and indents callees under their caller', () => {
    const m = buildLanes([
      step('o1', 0, { agentId: 'research', agentName: 'research' }),
      span('w1', 100, { agentId: 'search', agentName: 'search', parentKey: 'step:o1' }),
      step('o2', 2_000, { agentId: 'writer', agentName: 'writer' }),
    ])
    expect(m.lanes.map((l) => [l.agentName, l.indent])).toEqual([
      ['research', 0],
      ['search', 1],
      ['writer', 0],
    ])
  })
  it('splits overlapping calls of one agent into sub-rows, then counts the rest', () => {
    const calls = [0, 10, 20, 30, 40].map((t, i) =>
      span(`p${i}`, t, { agentId: 'search', agentName: 'search', endMs: 1_000 }),
    )
    const [lane] = buildLanes(calls).lanes
    expect(lane?.rows).toHaveLength(SUBROW_CAP)
    expect(lane?.overflow).toHaveLength(5 - SUBROW_CAP)
  })
  it('places an agent under its first caller and labels a call from another parent', () => {
    const m = buildLanes([
      step('a', 0, { agentId: 'A', agentName: 'A', endMs: 5_000 }),
      step('b', 10, { agentId: 'B', agentName: 'B', endMs: 5_000 }),
      span('c1', 100, { agentId: 'C', agentName: 'C', parentKey: 'step:a' }),
      span('c2', 200, { agentId: 'C', agentName: 'C', parentKey: 'step:b' }),
    ])
    expect(m.lanes.find((l) => l.agentId === 'C')?.parentId).toBe('agent:A')
    expect(m.fromCaller.get('span:c2')).toBe('B')
    expect(m.fromCaller.has('span:c1')).toBe(false)
  })
  it('never cycles on a call back to an ancestor (A → B → A)', () => {
    const m = buildLanes([
      step('a1', 0, { agentId: 'A', agentName: 'A', endMs: 9_000 }),
      span('b1', 100, { agentId: 'B', agentName: 'B', parentKey: 'step:a1', endMs: 8_000 }),
      span('a2', 200, { agentId: 'A', agentName: 'A', parentKey: 'span:b1' }),
    ])
    expect(m.lanes.map((l) => [l.agentName, l.indent])).toEqual([
      ['A', 0],
      ['B', 1],
    ])
    expect(m.fromCaller.get('span:a2')).toBe('B')
  })
  it('caps lanes, colors the first five agents, and opens big flows on the Table', () => {
    const calls = Array.from({ length: LANE_CAP + 2 }, (_, i) =>
      span(`p${i}`, i * 10, { agentId: `ag${i}`, agentName: `ag${i}` }),
    )
    const m = buildLanes(calls)
    expect(m.lanes).toHaveLength(LANE_CAP)
    expect(m.hidden).toHaveLength(2)
    expect(m.lanes.map((l) => l.colour).slice(4, 6)).toEqual([4, null])
    expect(m.tableFirst).toBe(false)
    expect(buildLanes(Array.from({ length: 201 }, (_, i) => span(`q${i}`, i))).tableFirst).toBe(
      true,
    )
  })
})

describe('flowState (A4, O5)', () => {
  const now = T + 10 * 60_000
  it('passes running, paused and failed through', () => {
    expect(flowState(flow({ status: 'running' }), [], null, now).status).toBe('running')
    expect(flowState(flow({ status: 'paused' }), [], null, now).status).toBe('paused')
    expect(flowState(flow({ status: 'failed' }), [], null, now)).toMatchObject({
      status: 'failed',
      durationMs: 4_000,
    })
  })
  it('A4: a completed flow with a running or waiting step is still going', () => {
    expect(flowState(flow(), [{ status: 'running' }], null, now)).toMatchObject({
      status: 'running',
      markedEarly: true,
    })
    expect(flowState(flow(), [{ status: 'awaiting_human' }], null, now).status).toBe('paused')
  })
  it('O5: trace spans after completed_at read Finishing for at most FINISHING_MAX_MS', () => {
    const t = { lastEndMs: T + 6_000, open: false }
    expect(flowState(flow(), [], t, T + 5_000)).toMatchObject({
      status: 'finishing',
      markedEarly: true,
      durationMs: null,
    })
    expect(flowState(flow(), [], t, T + 4_000 + FINISHING_MAX_MS + 1)).toMatchObject({
      status: 'completed',
      markedEarly: true,
      durationMs: 6_000,
    })
  })
  it('trusts completed when nothing contradicts it', () => {
    expect(
      flowState(flow(), [{ status: 'completed' }], { lastEndMs: T + 3_900, open: false }, now),
    ).toEqual({ status: 'completed', markedEarly: false, durationMs: 4_000 })
  })
  it('polls by status (F13) and re-reads traces only while working', () => {
    expect(pollMs('running')).toBe(RUNNING_POLL_MS)
    expect(pollMs('finishing')).toBe(RUNNING_POLL_MS)
    expect(pollMs('paused')).toBe(PAUSED_POLL_MS)
    expect(pollMs('completed')).toBe(false)
    expect(isWorking('paused')).toBe(false)
  })
})

describe('trace reads (O4, O5)', () => {
  it('re-reads every 12 s while working, once after the end, then stops', () => {
    expect(traceRereadMs(true, null, 0, 0)).toBe(TRACE_REREAD_MS)
    const end = T
    expect(traceRereadMs(false, end, end - 1_000, end + 2_000)).toBe(TRACE_FINAL_READ_MS - 2_000)
    expect(traceRereadMs(false, end, end + TRACE_FINAL_READ_MS, end + 20_000)).toBe(false)
    expect(traceRereadMs(false, null, 0, 0)).toBe(false)
  })
  it('a 503 (no trace store) ends the re-reads even while the flow works; a 404 keeps them (review fix)', () => {
    expect(traceRereadMs(true, null, 0, 0, 503)).toBe(false)
    expect(traceRereadMs(false, T, T - 1_000, T + 2_000, 503)).toBe(false)
    expect(traceRereadMs(true, null, 0, 0, 404)).toBe(TRACE_REREAD_MS)
  })
  it('finds the latest span end and open spans', () => {
    const a = node('a', null, 0, 3_000)
    const b = node('b', 'a', 100, null)
    expect(traceTiming({ span_lookup: { [a.id]: a } } as never)).toEqual({
      lastEndMs: T + 3_000,
      open: false,
    })
    expect(traceTiming({ span_lookup: { [a.id]: a, [b.id]: b } } as never)?.open).toBe(true)
    expect(traceTiming(undefined)).toBeNull()
  })
})

describe('fmtDuration (F17)', () => {
  it('is exact for spans and whole seconds for latency-only steps', () => {
    expect(fmtDuration(812)).toBe('812 ms')
    expect(fmtDuration(6_420)).toBe('6.4 s')
    expect(fmtDuration(400, false)).toBe('<1 s')
    expect(fmtDuration(1_900, false)).toBe('1 s')
    expect(fmtDuration(125_000)).toBe('2 min 5 s')
    expect(fmtDuration(null)).toBe('—')
  })
  it('flags whole-second timing only for closed, inexact calls', () => {
    expect(hasWholeSeconds([step('a', 0, { exactEnd: false })])).toBe(true)
    expect(hasWholeSeconds([step('a', 0, { exactEnd: false, endMs: null })])).toBe(false)
  })
})

describe('criticalPath (F22)', () => {
  const tree = [
    step('o1', 0, { endMs: 4_000 }),
    span('a', 100, { parentKey: 'step:o1', endMs: 1_000 }),
    span('b', 120, { parentKey: 'step:o1', endMs: 3_800 }),
    step('o2', 4_100, { endMs: 5_000 }),
  ]
  it('follows the last-ending call down its last-ending children', () => {
    const p = criticalPath([...tree, span('c', 4_200, { parentKey: 'step:o2', endMs: 4_900 })])
    // o2 waited for o1, whose slowest child b decided when o1 could end; a ended early and isn't on it.
    expect(p.keys && [...p.keys].sort()).toEqual(['span:b', 'span:c', 'step:o1', 'step:o2'])
  })
  it('is unavailable without exact timing, without parent links, or when it marks everything', () => {
    expect(criticalPath([...tree, step('x', 0, { exactEnd: false })]).reason).toBe('timing')
    expect(criticalPath([step('x', 0), step('y', 600)]).reason).toBe('parents')
    expect(
      criticalPath([step('p', 0, { endMs: 2_000 }), span('k', 10, { parentKey: 'step:p' })]).reason,
    ).toBe('everything')
    expect(criticalPath(tree).keys).not.toBeNull()
  })
})

describe('timeline (F13, F23, F28)', () => {
  it('keeps 20% headroom while running and none once finished', () => {
    const calls = [step('a', 0, { endMs: 1_000 })]
    expect(buildAxis(calls, 0, null, 1_000).endMs).toBe(1_200)
    expect(buildAxis(calls, 0, 1_000, 9_999).endMs).toBe(1_000)
    // Paused: open but not working, so no headroom.
    expect(buildAxis(calls, 0, null, 1_000, false).endMs).toBe(1_000)
  })
  it('puts the headroom on the drawn work, never on a collapsed wait', () => {
    const calls = [
      step('a', 0, { endMs: 1_000 }),
      step('w', 1_000, { status: 'resumed', endMs: 61_000 }),
      step('b', 61_000, { endMs: 62_000, status: 'running' }),
    ]
    expect(buildAxis(calls, 0, null, 62_000).endMs).toBe(62_000 + 2_000 * 0.2)
  })
  it('collapses a wait longer than 25% of the flow into a fixed break', () => {
    const calls = [
      step('a', 0, { endMs: 1_000 }),
      step('w', 1_000, { status: 'awaiting_human', endMs: null }),
    ]
    const axis = buildAxis(calls, 0, null, 60_000)
    expect(axis.gap).toEqual({ fromMs: 1_000, toMs: 60_000 })
    expect(position(axis, 60_000).px).toBe(BREAK_PX)
    expect(ticks(axis).every((t) => t.ms <= 1_000 || t.ms >= 60_000)).toBe(true)
    expect(buildAxis([step('a', 0, { endMs: 1_000 })], 0, 1_000, 0).gap).toBeNull()
  })
  it('brackets overlapping calls with the same parent, not sequential ones', () => {
    const f = fanOuts(
      [
        span('x', 100, { parentKey: 'p', endMs: 900 }),
        span('y', 200, { parentKey: 'p', endMs: 500 }),
        span('z', 1_000, { parentKey: 'p', endMs: 1_200 }),
        span('q', 150, { parentKey: null, endMs: 800 }),
      ],
      0,
    )
    expect(f).toEqual([
      { parentKey: 'p', fromMs: 100, toMs: 900, keys: ['span:x', 'span:y'], peak: 2 },
    ])
  })
})

describe('flowNarrative (F5)', () => {
  const base = {
    status: 'completed' as const,
    durationMs: 10_000,
    elapsedMs: 10_000,
    now: 20_000,
    exact: true,
    fanOut: null,
    reason: () => 'the agent didn’t answer in time',
  }
  const text = (s: ReturnType<typeof flowNarrative>) =>
    s.map((x) => x.map((c) => c.text).join('')).join(' ')
  it('names the call that took most of a finished flow', () => {
    const calls = [
      step('a', 0, { endMs: 2_000, agentName: 'research' }),
      step('b', 2_000, { endMs: 8_000, agentName: 'writer' }),
    ]
    expect(text(flowNarrative({ ...base, calls }))).toBe('Took 10.0 s; writer took 60% of it.')
  })
  it('names the slowest call doing its own work, not the call that encloses a fan-out', () => {
    // Regression: ISSUE-001 — a direct call's root (100% of the flow) was named as the slow call.
    // Found by /qa on 2026-10-06
    // Report: .gstack/qa-reports/qa-report-localhost-2026-10-06.md
    const calls = [
      span('root', 0, { endMs: 2_600, agentName: 'support-bot' }),
      span('a', 300, { endMs: 1_000, agentName: 'reviewer', parentKey: 'span:root' }),
      span('b', 320, { endMs: 2_500, agentName: 'sql-analyst', parentKey: 'span:root' }),
    ]
    expect(text(flowNarrative({ ...base, durationMs: 2_600, calls }))).toBe(
      'Took 2.6 s; sql-analyst took 84% of it.',
    )
  })
  it('says where it failed, in plain words, or that no step recorded it', () => {
    const calls = [step('a', 0, { status: 'failed', agentName: 'fact-check' })]
    expect(text(flowNarrative({ ...base, status: 'failed', calls }))).toBe(
      'Failed at fact-check: the agent didn’t answer in time.',
    )
    expect(
      text(flowNarrative({ ...base, status: 'failed', calls: [step('a', 0, { agentName: 'x' })] })),
    ).toBe('Failed after x; no step recorded the error.')
  })
  it('adds a fan-out when there is room', () => {
    const calls = [step('a', 0, { endMs: 9_000 })]
    expect(
      text(flowNarrative({ ...base, calls, fanOut: { parent: 'research', count: 3, keys: [] } })),
    ).toBe('Took 10.0 s. research ran 3 calls at once.')
  })
})

describe('the list (F8, F11, F16)', () => {
  const f = (over: Partial<Flow>): Flow =>
    flow({ created_at: iso(0), completed_at: iso(1_000), ...over })
  it('keeps the window and the kind', () => {
    const rows = [
      f({ flow_id: 'in' }),
      f({ flow_id: 'old', created_at: iso(-10_000) }),
      f({ flow_id: 'wf', root_agent_name: 'w', metadata: { mode: 'free_flowing' } }),
    ]
    expect(filterRows(rows, T - 1, T + 5_000, 'all').map((r) => r.flow_id)).toEqual(['in', 'wf'])
    expect(filterRows(rows, T - 1, T + 5_000, 'workflow').map((r) => r.flow_id)).toEqual(['wf'])
  })
  it('counts every bucket by the server’s status, UTC days', () => {
    const day = 86_400_000
    const start = Date.parse('2026-10-04T00:00:00Z')
    const rows = [
      f({ created_at: '2026-10-04T10:00:00Z', status: 'failed' }),
      f({ created_at: '2026-10-06T08:00:00Z', status: 'completed' }),
      f({ created_at: '2026-10-06T09:00:00Z', status: 'running' }),
    ]
    const b = buckets(rows, start, start + 3 * day, false)
    expect(b.map((x) => [x.label, x.total, x.failed, x.running])).toEqual([
      ['Oct 4', 1, 1, 0],
      ['Oct 5', 0, 0, 0],
      ['Oct 6', 2, 0, 1],
    ])
  })
  it('takes nearest-rank percentiles over finished flows, p99 only from 100', () => {
    const rows = Array.from({ length: 10 }, (_, i) => f({ completed_at: iso((i + 1) * 1_000) }))
    expect(percentiles([...rows, f({ status: 'running', completed_at: null })])).toEqual({
      p50: 5_000,
      p90: 9_000,
      p99: null,
      count: 10,
    })
    const many = Array.from({ length: 100 }, (_, i) => f({ completed_at: iso(i + 1) }))
    expect(percentiles(many)?.p99).toBe(99)
    expect(percentiles([])).toBeNull()
  })
})

describe('moveFrom (F25)', () => {
  const lanes = [
    [step('a', 0), step('d', 3_000)],
    [span('b', 1_000), span('c', 1_100)],
    [span('e', 2_900)],
  ]
  it('moves in start order across lanes, stopping at the ends', () => {
    expect(moveFrom(lanes, null, 'first')).toBe('step:a')
    expect(moveFrom(lanes, 'step:a', 'next')).toBe('span:b')
    expect(moveFrom(lanes, 'span:e', 'next')).toBe('step:d')
    expect(moveFrom(lanes, 'step:d', 'next')).toBe('step:d')
    expect(moveFrom(lanes, 'step:a', 'prev')).toBe('step:a')
    expect(moveFrom(lanes, 'span:b', 'last')).toBe('step:d')
  })
  it('moves to the nearest call in the lane above or below', () => {
    expect(moveFrom(lanes, 'step:d', 'down')).toBe('span:c')
    expect(moveFrom(lanes, 'span:c', 'down')).toBe('span:e')
    expect(moveFrom(lanes, 'span:e', 'up')).toBe('span:c')
    expect(moveFrom(lanes, 'step:a', 'up')).toBe('step:a')
  })
})

describe('review fixes (2026-10-06)', () => {
  it('reads an unknown server status as Unknown, never as Failed', () => {
    expect(flowState(flow({ status: 'cancelled' }), [], null, T).status).toBe('unknown')
    expect(flowState(flow({ status: 'failed' }), [], null, T).status).toBe('failed')
  })

  it('jumps a days-long wait in one step instead of walking it', () => {
    const day = 86_400_000
    const calls = [
      step('a', 0, { endMs: 2_000 }),
      step('w', 2_000, { status: 'awaiting_human', endMs: null }),
    ]
    const axis = buildAxis(calls, 0, null, 30 * day, false)
    const started = performance.now()
    const t = ticks(axis)
    expect(performance.now() - started).toBeLessThan(50)
    expect(t.length).toBeGreaterThan(0)
    expect(t.every((x) => x.ms <= 2_000 || x.ms >= 30 * day)).toBe(true)
  })

  it('counts a chain of overlaps by its peak, not its length', () => {
    const [f] = fanOuts(
      [
        span('a', 0, { parentKey: 'p', endMs: 100 }),
        span('b', 50, { parentKey: 'p', endMs: 150 }),
        span('c', 125, { parentKey: 'p', endMs: 200 }),
      ],
      0,
    )
    expect(f).toMatchObject({ keys: ['span:a', 'span:b', 'span:c'], peak: 2 })
  })

  it('bounds the chart for any window and keeps every label unique', () => {
    const start = Date.parse('2000-01-01T00:00:00Z')
    const end = Date.parse('2026-10-06T00:00:00Z')
    const long = buckets([], start, end, false)
    expect(long.length).toBeLessThanOrEqual(MAX_BUCKETS + 2)
    expect(new Set(long.map((b) => b.label)).size).toBe(long.length)
    const hours = buckets(
      [],
      Date.parse('2026-10-05T12:34:00Z'),
      Date.parse('2026-10-06T12:35:00Z'),
      true,
    )
    expect(new Set(hours.map((b) => b.label)).size).toBe(hours.length)
  })

  it('resolves a routed step’s agent by its unique name, and keeps a waiting step open (FL-9)', () => {
    const base: FlowStep = {
      id: 's1',
      step_order: 1,
      depth: 1,
      agent_id: null,
      agent_name: 'writer',
      status: 'awaiting_human',
      created_at: iso(0),
      completed_at: iso(400),
    }
    const [c] = callsFromSteps([base], (name) => (name === 'writer' ? 'w-id' : null))
    expect(c).toMatchObject({ agentId: 'w-id', endMs: null })
    expect(callsFromSteps([{ ...base, agent_name: 'ghost' }], () => null)[0]?.agentId).toBeNull()
  })

  it('words a failure from the error alone, on whole words', () => {
    expect(reasonFor({ error: 'Failed to generate a response' })).toBe(
      'the agent reported an error',
    )
    expect(reasonFor({ error: 'HTTP 429 Too Many Requests' })).toBe(
      'the agent hit a rate or usage limit',
    )
    expect(reasonFor({ error: 'upstream timeout after 3 s' })).toBe(
      'the agent didn’t answer in time',
    )
    expect(reasonFor({ error: null })).toBe('the agent reported an error')
  })

  it('never reads a workflow flow’s context id as a chat session', () => {
    expect(
      flowSession(flow({ metadata: { mode: 'free_flowing', context_id: 'exec-1' } })),
    ).toBeNull()
    expect(flowSession(flow({ metadata: { context_id: 'chat-1' } }))).toBe('chat-1')
    expect(
      flowSession({ ...flow({ metadata: { mode: 'free_flowing' } }), session_id: 's9' } as Flow),
    ).toBe('s9')
  })

  it('reads a human wait in whole minutes', () => {
    expect(fmtWait(40_000)).toBe('<1 min')
    expect(fmtWait(25 * 60_000 + 59_000)).toBe('25 min')
    expect(fmtWait(125 * 60_000)).toBe('2 h 5 min')
  })

  it('schedules an overdue final trace read a moment out, never at 0', () => {
    expect(traceRereadMs(false, 0, 0, 60_000)).toBeGreaterThan(0)
  })

  it('keeps admitted spans across re-reads; a late, earlier span never evicts one or spends a read past the cap', () => {
    const sp = (id: string, start: number) =>
      ({ node: { id, start_time: iso(start) } }) as unknown as FlatSpan
    const first = admit([], [sp('a', 10), sp('b', 20)], 2)
    expect(first).toEqual(['a', 'b'])
    const again = admit(first, [sp('z', 0), sp('a', 10), sp('b', 20)], 2)
    expect(again).toBe(first)
    expect(admit(['a'], [sp('z', 0), sp('a', 10)], 2)).toEqual(['a', 'z'])
  })
})

describe('adversarial review fixes (2026-10-07)', () => {
  const MIN = 60_000
  const ranLate = { lastEndMs: T + 4_000 + 5_000, open: false }

  it('a client clock behind the server can’t stretch Finishing: it runs from when the page first read the end', () => {
    // completed_at is T + 4 s on the server's clock; this client is an hour behind and first read it at its T − 1 h.
    const seen = T - 60 * MIN
    expect(flowState(flow(), [], ranLate, seen + 10_000, seen).status).toBe('finishing')
    expect(flowState(flow(), [], ranLate, seen + FINISHING_MAX_MS + 1, seen).status).toBe(
      'completed',
    )
    // Without the first-read time the same clock would have held Finishing for an hour.
    expect(flowState(flow(), [], ranLate, seen + FINISHING_MAX_MS + 1).status).toBe('finishing')
    // …and the final trace read is due 10 s after that first read, not an hour later.
    const end = endedAt(iso(4_000), seen)
    expect(end).toBe(seen)
    expect(traceRereadMs(false, end, seen, seen + 1_000)).toBe(TRACE_FINAL_READ_MS - 1_000)
    expect(traceRereadMs(false, end, seen + TRACE_FINAL_READ_MS, seen + 20_000)).toBe(false)
    expect(endedAt(null, seen)).toBeNull()
    expect(endedAt(iso(4_000), null)).toBe(T + 4_000)
  })

  it('a gone (404) or no-longer-yours (403) flow stops polling; other errors don’t', () => {
    expect(isGone(new ApiError(404, null, '/api/flows/f1', 'x'))).toBe(true)
    expect(isGone(new ApiError(403, null, '/api/flows/f1', 'x'))).toBe(true)
    expect(isGone(new ApiError(500, null, '/api/flows/f1', 'x'))).toBe(false)
    expect(isGone(new Error('x'))).toBe(false)
  })

  it('an unknown status is still re-read; a moving flow with nothing new for 30 min is re-read slowly', () => {
    expect(pollMs('unknown')).toBe(PAUSED_POLL_MS)
    expect(pollMs('running', STUCK_AFTER_MS - 1)).toBe(RUNNING_POLL_MS)
    expect(pollMs('running', STUCK_AFTER_MS)).toBe(STUCK_POLL_MS)
    expect(pollMs('paused', 2 * STUCK_AFTER_MS)).toBe(STUCK_POLL_MS)
    expect(pollMs('completed', 2 * STUCK_AFTER_MS)).toBe(false)
    expect(pollMs('failed')).toBe(false)
    expect(
      lastActivityMs({ created_at: iso(0) }, [
        { created_at: iso(1_000), completed_at: iso(9_000) },
        { created_at: iso(5_000), completed_at: null },
      ]),
    ).toBe(T + 9_000)
    expect(lastActivityMs({ created_at: 'not a date' }, [])).toBeNull()
  })

  it('a paused step works until its pause, then waits: the wait is counted from the pause', () => {
    const [w] = callsFromSteps([
      {
        id: 'w',
        step_order: 1,
        depth: 1,
        agent_id: 'a',
        agent_name: 'approver',
        status: 'awaiting_human',
        created_at: iso(0),
        // The dispatcher stamps completed_at at the pause.
        completed_at: iso(5 * MIN),
        latency_ms: null,
      },
    ])
    expect(w).toMatchObject({ startMs: T, endMs: null, waitStartMs: T + 5 * MIN })
    // Five minutes of work, then one minute waiting: "1 min", not "6 min".
    const [sentence] = flowNarrative({
      status: 'paused',
      durationMs: null,
      elapsedMs: 6 * MIN,
      now: T + 6 * MIN,
      calls: [w!],
      exact: true,
      fanOut: null,
      reason: reasonFor,
    })
    expect(sentence?.[0]?.text).toMatch(/^Waiting on you for 1 min:/)
    // The axis folds the wait, not the work.
    const axis = buildAxis([w!], T, null, T + 60 * MIN, false)
    expect(axis.gap).toEqual({ fromMs: T + 5 * MIN, toMs: T + 60 * MIN })
    // A pause stamped before the start is not a pause.
    const [odd] = callsFromSteps([
      {
        id: 'x',
        step_order: 1,
        depth: 1,
        agent_name: 'approver',
        status: 'awaiting_human',
        created_at: iso(1_000),
        completed_at: iso(0),
      },
    ])
    expect(odd?.waitStartMs).toBeNull()
  })

  it('merging a waiting step with its closed proxy span keeps the wait open', () => {
    const waiting = step('w', 0, {
      status: 'awaiting_human',
      endMs: null,
      exactEnd: false,
      agentId: 'a',
      waitStartMs: 400,
    })
    const transport = span('p', 50, { agentId: 'a', endMs: 600 })
    const [m] = mergeCalls([waiting], [transport])
    expect(m).toMatchObject({
      key: 'step:w',
      source: 'both',
      endMs: null,
      status: 'awaiting_human',
      waitStartMs: 400,
    })
  })

  it('a failed span-detail read is tried again with the trace while the flow works, then waits for Retry', () => {
    expect(spanRereadMs(true, 'error')).toBe(TRACE_REREAD_MS)
    expect(spanRereadMs(true, 'success')).toBe(false)
    expect(spanRereadMs(false, 'error')).toBe(false)
  })

  it('server status words never read Object.prototype', () => {
    expect(stepStatusLabel('constructor')).toBe('constructor')
    expect(stepStatusLabel('toString')).toBe('toString')
    expect(stepStatusLabel('failed')).toBe(copy.stepStatus.failed)
    expect(own(copy.list.statuses, 'constructor')).toBeUndefined()
    expect(own(copy.list.statuses, 'paused')).toBe('Paused')
  })
})

describe('swimlane connectors (F30)', () => {
  const xAt = (t: number) => (t - T) / 10
  const at = (key: string, startMs: number, endMs: number | null) =>
    ({ key, startMs, endMs }) as unknown as Call
  it('a hand-off is an S-curve from the caller’s end to the call’s start', () => {
    const d = curve(
      {
        from: { y: 10, call: at('a', T, T + 1_000) },
        to: { y: 50, call: at('b', T + 2_000, T + 3_000) },
        onPath: true,
      },
      xAt,
      T,
    )
    expect(d.startsWith('M 100.0 10 C')).toBe(true)
    expect(d.endsWith('200.0 50')).toBe(true)
  })
  it('a call made while its caller runs drops from the caller’s edge, along a shared trunk when given', () => {
    const e = {
      from: { y: 10, call: at('a', T, T + 5_000) },
      to: { y: 50, call: at('b', T + 1_000, T + 2_000) },
      onPath: false,
    }
    expect(curve(e, xAt, T)).toBe('M 92.0 20 L 92.0 44.0 Q 92.0 50, 100.0 50')
    expect(curve(e, xAt, T, 20, 40)).toBe('M 40.0 20 L 40.0 44.0 Q 40.0 50, 100.0 50')
  })
})
