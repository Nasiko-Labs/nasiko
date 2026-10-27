/**
 * The flows mock (plans/feat-flows.md), shaped as nasiko-cloud-rs `development` `6ab60326` `oss/server/src/flows.rs`:
 * - `GET /api/flows`: the caller's own (`user_id`), `status` exact, `q` ILIKE on root agent or title, newest first,
 *   `limit` (default 50) / `offset`, `Paginated` with `total` = the page length;
 * - `GET /api/flows/{id}` → `{flow, steps}` (steps by `step_order`), a bare 404 for unknown or someone else's;
 * - `GET /api/flows/{id}/steps` → the bare step array.
 * Ids this mock doesn't own fall through to chat's routed-turn flows (`chatStore.ts`). Its flows' traces are served
 * here too (same `/api/observability/trace|span` shapes as `observability.ts`), and other trace ids fall through.
 *
 * The seed covers each case the pages handle: an Orchestrator flow whose first agent fans out three calls at once
 * (only the trace knows them, F4), a direct call with no steps (F12), a failed step (F15), a paused flow (F13), a
 * running flow that advances with the clock, a workflow step, the FL-8 early completion with a step still running
 * (A4) and with trace calls ending after `completed_at` (O5), another user's flow, and filler for the list charts.
 * Timing mirrors the server: step and flow `latency_ms`/`duration_ms` are whole seconds (FL-4); timestamps are exact.
 * Ids: flows (trace ids) `5eed0014…`, steps `5eed0015-*`.
 */
import { http, HttpResponse, type HttpHandler } from 'msw'
import type { SpanDetail, SpanNode, TraceDetail } from '@nasiko/features/observability/types'
import type { Flow, FlowStep } from '@nasiko/features/flows/types'
import type { AgentsState, MockAgent } from './agents'
import { encodeSpanId } from './spanBuilder'

const SEC = 1_000
const MIN = 60 * SEC
const HOUR = 60 * MIN
const DAY = 24 * HOUR
const OTHER_USER = '5eed0000-0000-4000-8000-00000000a0ff'

export interface FlowsMockCtx {
  loggedIn: () => boolean
  me: () => { id: string }
  agents: () => AgentsState
  now: () => number
  hasVariant: (v: string) => boolean
  /**
   * A trace from the observability seed (a chat session's request), so "Open flow" from Sessions and the trace page
   * opens a flow: on a server, every request through the proxy has one with the same id. Its trace stays the
   * observability mock's.
   */
  seedTrace?: (id: string) => SeedFlowTrace | null
}

interface SeedFlowTrace {
  traceId: string
  agentId: string
  agentName: string
  startMs: number
  latencyMs: number
  sessionId: string
}

/** A seed trace as the direct call's flow the proxy would have written (`agent_proxy.rs`: no steps). */
function seedFlow(t: SeedFlowTrace, me: string): Flow {
  return {
    id: `5eed0016-0000-4000-8000-${t.traceId.slice(-12)}`,
    flow_id: t.traceId,
    user_id: me,
    root_agent_id: t.agentId,
    root_agent_name: t.agentName,
    title: null,
    status: 'completed',
    duration_ms: Math.round(t.latencyMs / SEC) * SEC,
    error_message: null,
    metadata: { context_id: t.sessionId },
    created_at: iso(t.startMs),
    completed_at: iso(t.startMs + t.latencyMs),
  }
}

/** A span of a flow's trace; `end` null while open (the mock never serves open spans: Tempo has finished ones only). */
interface MockSpan {
  hex: string
  parentHex: string | null
  name: 'a2a.dispatch' | 'a2a.proxy'
  /** ms from the flow's start */
  start: number
  end: number
  agentId: string | null
  error?: boolean
}

interface MockStepDef {
  agent: MockAgent
  start: number
  /** ms from the flow's start; null while running or waiting. */
  end: number | null
  status: string
  input: string
  output?: string
  error?: string
}

interface MockFlow {
  id: string
  userId: string
  /** epoch ms */
  start: number
  root: { id: string | null; name: string }
  title: string | null
  status: string
  /** ms from start; null while open. */
  end: number | null
  error?: string
  metadata: Record<string, unknown>
  steps: MockStepDef[]
  spans: MockSpan[]
}

export interface FlowsState {
  flows: MockFlow[]
}

/** A MAF execution id (the workflow flow's `metadata.context_id`). */
const executionId = (n: number) => `5eed0011-0000-4000-8000-${n.toString(16).padStart(12, '0')}`
/** A waiting step pauses this long after it starts. */
const PAUSE_AFTER_MS = 400
const flowId = (n: number) => `5eed0014${n.toString(16).padStart(24, '0')}`
const stepId = (flow: number, i: number) =>
  `5eed0015-0000-4000-8000-${(flow * 100 + i).toString(16).padStart(12, '0')}`
const spanHex = (flow: number, i: number) =>
  `5eed${flow.toString(16).padStart(4, '0')}${i.toString(16).padStart(8, '0')}`
const iso = (ms: number) => new Date(ms).toISOString()
/** flows.rs at 6ab60326: `EXTRACT(EPOCH …)::integer * 1000`, the numeric cast rounds to the nearest second (FL-4). */
const wholeSeconds = (ms: number) => Math.round(ms / SEC) * SEC
/** Deterministic 0..1. */
const vary = (a: number, b: number) => (((a * 2654435761) ^ (b * 40503)) >>> 0) / 2 ** 32

const ORCH = { id: null, name: 'orchestrator' }

export function buildFlowsState(ctx: Pick<FlowsMockCtx, 'agents' | 'me' | 'now'>): FlowsState {
  const now = ctx.now()
  const me = ctx.me().id
  const pool = ctx
    .agents()
    .agents.filter((a) => !a.deleted && !a.tags.includes('coding-agent'))
    .slice(0, 6)
  const ag = (i: number): MockAgent => {
    const a = pool[i % Math.max(1, pool.length)]
    if (!a) throw new Error('[flows mock] the agents mock has no agents')
    return a
  }
  const [research, writer, search, factCheck, docs, approver] = [0, 1, 2, 3, 4, 5].map(ag) as [
    MockAgent,
    MockAgent,
    MockAgent,
    MockAgent,
    MockAgent,
    MockAgent,
  ]
  const flows: MockFlow[] = []
  let n = 0

  /** The Orchestrator's flow: its dispatch span, one proxy span per recorded step, plus any extra trace-only spans. */
  const orchestrated = (
    f: Omit<MockFlow, 'id' | 'root' | 'spans' | 'userId' | 'metadata'> & {
      userId?: string
      extra?: (stepSpan: (i: number) => string, add: (s: Omit<MockSpan, 'hex'>) => string) => void
    },
  ) => {
    const id = ++n
    const spans: MockSpan[] = []
    let k = 0
    const add = (s: Omit<MockSpan, 'hex'>) => {
      const hex = spanHex(id, ++k)
      spans.push({ ...s, hex })
      return hex
    }
    const lastEnd = Math.max(...f.steps.map((s) => s.end ?? 0), f.end ?? 0)
    const root = add({
      parentHex: null,
      name: 'a2a.dispatch',
      start: 0,
      end: lastEnd + 200,
      agentId: null,
    })
    const stepSpans = f.steps.map((s) =>
      s.end === null
        ? ''
        : add({
            parentHex: root,
            name: 'a2a.proxy',
            start: s.start + 15,
            end: s.end - 10,
            agentId: s.agent.id,
            error: s.status === 'failed',
          }),
    )
    f.extra?.((i) => stepSpans[i] ?? root, add)
    flows.push({
      id: flowId(id),
      userId: f.userId ?? me,
      start: f.start,
      root: ORCH,
      title: f.title,
      status: f.status,
      end: f.end,
      error: f.error,
      metadata: {},
      steps: f.steps,
      spans,
    })
  }

  // 1. The showcase: research fans out three searches at once, then a fact check; the writer drafts (F3, F4, F27).
  orchestrated({
    start: now - 2 * HOUR,
    title: 'Compare 2026 pricing for three vendors and draft a summary',
    status: 'completed',
    end: 8_400,
    steps: [
      {
        agent: research,
        start: 400,
        end: 4_600,
        status: 'completed',
        input: 'Collect current pricing for Acme, Globex and Initech',
        output: 'Pricing pages found for all three; Globex lists two tiers.',
      },
      {
        agent: writer,
        start: 4_800,
        end: 8_100,
        status: 'completed',
        input: 'Draft a one-page comparison from the research notes',
        output: 'Drafted a comparison table and a three-line recommendation.',
      },
    ],
    extra: (stepSpan, add) => {
      const r = stepSpan(0)
      add({ parentHex: r, name: 'a2a.proxy', start: 900, end: 2_100, agentId: search.id })
      add({ parentHex: r, name: 'a2a.proxy', start: 950, end: 3_800, agentId: search.id })
      add({ parentHex: r, name: 'a2a.proxy', start: 1_000, end: 2_600, agentId: search.id })
      add({ parentHex: r, name: 'a2a.proxy', start: 3_900, end: 4_400, agentId: factCheck.id })
    },
  })

  // 2. A direct call: no steps (F12); its chat session is known (O1).
  flows.push({
    id: flowId(++n),
    userId: me,
    start: now - 3 * HOUR,
    root: { id: docs.id, name: docs.name },
    title: null,
    status: 'completed',
    end: 1_240,
    metadata: { context_id: '5eed-sess-pr-481' },
    steps: [],
    spans: [
      {
        hex: spanHex(n, 1),
        parentHex: null,
        name: 'a2a.proxy',
        start: 0,
        end: 1_240,
        agentId: docs.id,
      },
    ],
  })

  // 3. A failed step (F15).
  orchestrated({
    start: now - 5 * HOUR,
    title: 'Check the Q3 contract for renewal terms',
    status: 'failed',
    end: 6_300,
    error: 'agent call failed: fact-check',
    steps: [
      {
        agent: research,
        start: 300,
        end: 2_900,
        status: 'completed',
        input: 'Find the renewal clause in the Q3 contract',
        output: 'Clause 14.2: auto-renews for 12 months unless cancelled 60 days before.',
      },
      {
        agent: factCheck,
        start: 3_100,
        end: 6_200,
        status: 'failed',
        input: 'Confirm the 60-day notice period against the signed copy',
        error: 'upstream timeout after 3 s',
      },
    ],
  })

  // 4. Paused on a human (F13).
  orchestrated({
    start: now - 25 * MIN,
    title: 'Refund order #4471 after checking the policy',
    status: 'paused',
    end: null,
    steps: [
      {
        agent: research,
        start: 300,
        end: 2_100,
        status: 'completed',
        input: 'Look up the refund policy for damaged items',
        output: 'Damaged items are refundable within 30 days with a photo.',
      },
      {
        agent: approver,
        start: 2_300,
        end: null,
        status: 'awaiting_human',
        input: 'Approve a refund of $84.20 for order #4471',
      },
    ],
  })

  // 5. Running, with the clock: research for ~2.4 s, then the writer; done after ~6.5 s.
  const t0 = now
  orchestrated({
    start: t0,
    title: 'Summarise this week’s support tickets',
    status: 'running',
    end: 6_500,
    steps: [
      {
        agent: research,
        start: 300,
        end: 2_400,
        status: 'completed',
        input: 'Collect this week’s support tickets by topic',
        output: '41 tickets in 6 topics; billing leads with 14.',
      },
      {
        agent: writer,
        start: 2_600,
        end: 6_200,
        status: 'completed',
        input: 'Summarise the topics in five bullets',
        output: 'Five bullets drafted.',
      },
    ],
  })
  const live = flows[flows.length - 1]
  if (live) live.status = 'live'

  // 6. A workflow step (MAF, `metadata.mode = "free_flowing"`).
  flows.push({
    id: flowId(++n),
    userId: me,
    start: now - 26 * HOUR,
    root: { id: writer.id, name: writer.name },
    title: 'Quarterly report: draft the revenue section',
    status: 'completed',
    end: 3_900,
    // maf/executor.rs: `context_id` is the workflow execution id, never a chat session.
    metadata: { mode: 'free_flowing', context_id: executionId(n) },
    steps: [],
    spans: [
      {
        hex: spanHex(n, 1),
        parentHex: null,
        name: 'a2a.proxy',
        start: 0,
        end: 3_900,
        agentId: writer.id,
      },
    ],
  })

  // 7. FL-8 / A4: the server says completed while the writer's step is still running (static).
  orchestrated({
    start: now - 40 * MIN,
    title: 'Draft the launch announcement',
    status: 'completed',
    end: 2_000,
    steps: [
      {
        agent: research,
        start: 200,
        end: 1_900,
        status: 'completed',
        input: 'Gather the launch facts',
        output: 'Facts gathered.',
      },
      {
        agent: writer,
        start: 2_100,
        end: null,
        status: 'running',
        input: 'Draft the announcement',
      },
    ],
  })

  // 8. FL-8 / O5: a direct call marked completed at 1 s while its fan-out ran to 2.5 s (trace only).
  {
    const id = ++n
    flows.push({
      id: flowId(id),
      userId: me,
      start: now - 4 * HOUR,
      root: { id: research.id, name: research.name },
      title: null,
      status: 'completed',
      end: 1_000,
      metadata: {},
      steps: [],
      spans: [
        {
          hex: spanHex(id, 1),
          parentHex: null,
          name: 'a2a.proxy',
          start: 0,
          end: 2_600,
          agentId: research.id,
        },
        {
          hex: spanHex(id, 2),
          parentHex: spanHex(id, 1),
          name: 'a2a.proxy',
          start: 300,
          end: 1_000,
          agentId: search.id,
        },
        {
          hex: spanHex(id, 3),
          parentHex: spanHex(id, 1),
          name: 'a2a.proxy',
          start: 320,
          end: 2_500,
          agentId: factCheck.id,
        },
      ],
    })
  }

  // 9. Another user's flow: never listed, a 404 here.
  orchestrated({
    userId: OTHER_USER,
    start: now - HOUR,
    title: 'Someone else’s question',
    status: 'completed',
    end: 3_000,
    steps: [
      {
        agent: research,
        start: 200,
        end: 2_800,
        status: 'completed',
        input: 'Elsewhere',
        output: 'Done.',
      },
    ],
  })

  // Filler for the list: 60 flows over 14 days, mostly direct calls, no traces (expired).
  for (let i = 0; i < 60; i++) {
    const id = ++n
    const r = vary(id, 7)
    const start = now - Math.round((0.3 + vary(id, 3) * 13.6) * DAY)
    const failed = r < 0.08
    const kind = vary(id, 11)
    const agent = ag(Math.floor(vary(id, 5) * 6))
    const dur = 600 + Math.round(vary(id, 13) * 9_000)
    if (kind < 0.3) {
      const second = ag(Math.floor(vary(id, 17) * 6) + 1)
      const mid = Math.round(dur * 0.45)
      orchestrated({
        start,
        title: FILLER_TITLES[i % FILLER_TITLES.length] ?? 'A question',
        status: failed ? 'failed' : 'completed',
        end: dur,
        error: failed ? `agent call failed: ${second.name}` : undefined,
        steps: [
          {
            agent,
            start: 200,
            end: mid,
            status: 'completed',
            input: 'Gather what the question needs',
            output: 'Gathered.',
          },
          {
            agent: second,
            start: mid + 150,
            end: dur - 100,
            status: failed ? 'failed' : 'completed',
            input: 'Answer from the notes',
            output: failed ? undefined : 'Answered.',
            error: failed ? 'agent returned an error' : undefined,
          },
        ],
      })
      const last = flows[flows.length - 1]
      if (last) last.spans = []
    } else {
      flows.push({
        id: flowId(id),
        userId: me,
        start,
        root: { id: agent.id, name: agent.name },
        title: null,
        status: failed ? 'failed' : 'completed',
        end: dur,
        error: failed ? 'agent returned 500' : undefined,
        metadata: kind > 0.85 ? { mode: 'free_flowing', context_id: executionId(id) } : {},
        steps: [],
        spans: [],
      })
    }
  }
  return { flows }
}

const FILLER_TITLES = [
  'Which plan fits a 40-seat team?',
  'Summarise yesterday’s incidents',
  'Draft a reply to the vendor',
  'Find the onboarding checklist',
  'Compare last month’s spend by team',
  'What changed in release 2.14?',
]

// ---------------------------------------------------------------------------------------------------------------
// Wire shapes, read at `now` (the running flow moves with the clock).

/** The running flow's view at `now`: steps start and finish on their offsets, the flow completes at its end. */
function at(
  f: MockFlow,
  now: number,
): { status: string; end: number | null; steps: MockStepDef[] } {
  if (f.status !== 'live') return { status: f.status, end: f.end, steps: f.steps }
  const t = now - f.start
  const done = f.end !== null && t >= f.end
  return {
    status: done ? 'completed' : 'running',
    end: done ? f.end : null,
    steps: f.steps
      .filter((s) => s.start <= t)
      .map((s) =>
        s.end !== null && s.end <= t
          ? s
          : { ...s, end: null, status: 'running', output: undefined },
      ),
  }
}

function toFlow(f: MockFlow, now: number): Flow {
  const v = at(f, now)
  return {
    id: `5eed0016-0000-4000-8000-${f.id.slice(-12)}`,
    flow_id: f.id,
    user_id: f.userId,
    root_agent_id: f.root.id,
    root_agent_name: f.root.name,
    title: f.title,
    status: v.status,
    max_depth_reached: 0,
    total_invocations: 0,
    total_tokens_used: 0,
    total_cost_usd: null,
    duration_ms: v.end === null ? null : wholeSeconds(v.end),
    error_message: v.status === 'failed' ? (f.error ?? null) : null,
    metadata: f.metadata,
    created_at: iso(f.start),
    completed_at: v.end === null ? null : iso(f.start + v.end),
  }
}

function toSteps(f: MockFlow, now: number): FlowStep[] {
  const flowNumber = parseInt(f.id.slice(8), 16)
  return at(f, now).steps.map((s, i) => ({
    id: stepId(flowNumber, i + 1),
    flow_id: f.id,
    step_order: i + 1,
    depth: 1,
    // router/a2a_dispatch.rs at 6ab60326 inserts `agent_name` only: routed steps have no agent id (FL-9).
    agent_id: null,
    agent_name: s.agent.name,
    caller_agent_name: 'orchestrator',
    input_summary: s.input,
    output_summary: s.output ?? null,
    status: s.status,
    tokens_used: 0,
    latency_ms: s.end === null ? null : wholeSeconds(s.end - s.start),
    error_message: s.error ?? null,
    created_at: iso(f.start + s.start),
    // The dispatcher stamps `completed_at` when a step pauses for a human (a2a_dispatch.rs).
    completed_at:
      s.status === 'awaiting_human'
        ? iso(f.start + s.start + PAUSE_AFTER_MS)
        : s.end === null
          ? null
          : iso(f.start + s.end),
  }))
}

/** Tempo has finished spans only: a span shows once it has ended. */
const visibleSpans = (f: MockFlow, now: number) => f.spans.filter((s) => f.start + s.end <= now)

function spanNode(f: MockFlow, s: MockSpan): SpanNode {
  return {
    id: encodeSpanId(s.hex),
    span_id: s.hex,
    name: s.name,
    span_kind: s.name === 'a2a.proxy' ? 'server' : 'internal',
    status_code: s.error ? 'ERROR' : 'OK',
    start_time: iso(f.start + s.start),
    end_time: iso(f.start + s.end),
    parent_id: s.parentHex ? encodeSpanId(s.parentHex) : null,
    latency_ms: s.end - s.start,
    token_count_total: 0,
    input_tokens: 0,
    output_tokens: 0,
    cache_read_tokens: 0,
    cache_creation_tokens: 0,
    model: null,
    operation: s.name === 'a2a.proxy' ? 'invoke_agent' : null,
    provider: null,
    span_annotation_summaries: [],
    children: [],
  }
}

function traceDetail(f: MockFlow, now: number): TraceDetail | null {
  const spans = visibleSpans(f, now)
  if (!spans.length) return null
  const nodes = new Map(spans.map((s) => [s.hex, spanNode(f, s)]))
  const lookup: Record<string, SpanNode> = {}
  for (const s of spans) lookup[encodeSpanId(s.hex)] = spanNode(f, s)
  for (const s of spans) {
    const parent = s.parentHex ? nodes.get(s.parentHex) : undefined
    const node = nodes.get(s.hex)
    if (parent && node) parent.children.push(node)
  }
  const roots = spans
    .filter((s) => !s.parentHex || !nodes.has(s.parentHex))
    .flatMap((s) => nodes.get(s.hex) ?? [])
  const start = Math.min(...spans.map((s) => s.start))
  const end = Math.max(...spans.map((s) => s.end))
  const zero = { cost: 0 }
  return {
    id: f.id,
    project_session_id: typeof f.metadata.context_id === 'string' ? f.metadata.context_id : '',
    num_spans: spans.length,
    latency_ms: end - start,
    cost_summary: {
      total: zero,
      prompt: zero,
      completion: zero,
      cache_read: zero,
      cache_creation: zero,
    },
    root_spans: {
      edges: roots.map((r) => ({
        span: {
          id: r.id,
          span_id: r.span_id,
          parent_id: r.parent_id ?? null,
          status_code: r.status_code,
        },
      })),
    },
    spans: roots,
    span_lookup: lookup,
  } satisfies TraceDetail
}

function spanDetail(f: MockFlow, hex: string, now: number): SpanDetail | null {
  const s = visibleSpans(f, now).find((x) => x.hex === hex)
  if (!s) return null
  const cw = { cost: 0, tokens: 0 }
  const content = (value: string) => ({ value, mime_type: 'text/plain', parsed_value: value })
  return {
    id: encodeSpanId(s.hex),
    span_id: s.hex,
    trace: { id: f.id, trace_id: f.id },
    name: s.name,
    span_kind: s.name === 'a2a.proxy' ? 'server' : 'internal',
    status_code: s.error ? 'ERROR' : 'OK',
    code: s.error ? 'ERROR' : 'OK',
    status_message: s.error ? 'agent call failed' : '',
    start_time: iso(f.start + s.start),
    end_time: iso(f.start + s.end),
    // SpanDetail.parent_id is the raw hex id (unlike SpanNode's base64).
    parent_id: s.parentHex,
    latency_ms: s.end - s.start,
    token_count_total: 0,
    provider: null,
    model: null,
    cache_read_tokens: 0,
    cache_creation_tokens: 0,
    cost_summary: { total: cw, prompt: cw, completion: cw, cache_read: cw, cache_creation: cw },
    input: content(''),
    output: content(''),
    // agent_proxy.rs: `info_span!("a2a.proxy", agent.id = …)`; unflattened like service.rs `unflatten_attrs`.
    attributes: (s.agentId
      ? { agent: { id: s.agentId }, gen_ai: { operation: { name: 'invoke_agent' } } }
      : {}) as Record<string, never>,
    events: [],
    span_annotations: [],
    span_annotation_summaries: [],
    document_retrieval_metrics: [],
    document_evaluations: [],
    project: {
      id: '',
      annotation_configs: { configs: [], edges: [] } as unknown as Record<string, never>,
    },
  } satisfies SpanDetail
}

// ---------------------------------------------------------------------------------------------------------------

const OBS = '/api/observability'
const unauthorized = () => new HttpResponse('unauthorized', { status: 401 })
const text = (body: string, status: number) => new HttpResponse(body, { status })

export function flowHandlers(ctx: FlowsMockCtx & { state: () => FlowsState }): HttpHandler[] {
  const find = (id: string) => ctx.state().flows.find((f) => f.id === id)
  const own = (f: MockFlow | undefined) => !!f && f.userId === ctx.me().id
  const traceStore = (fn: () => Response): Response => {
    if (ctx.hasVariant('trace-503'))
      return text('observability backend not configured (set TEMPO_URL and LOKI_URL)', 503)
    if (ctx.hasVariant('trace-500')) return text('internal error', 500)
    return fn()
  }
  return [
    http.get('/api/flows', ({ request }) => {
      if (!ctx.loggedIn()) return unauthorized()
      if (ctx.hasVariant('flows-absent')) return new HttpResponse(null, { status: 404 })
      const url = new URL(request.url)
      const status = url.searchParams.get('status')
      const q = url.searchParams.get('q')?.toLowerCase()
      const limit = Number(url.searchParams.get('limit') ?? 50)
      const offset = Number(url.searchParams.get('offset') ?? 0)
      const now = ctx.now()
      const rows = ctx.hasVariant('flows-empty')
        ? []
        : ctx
            .state()
            .flows.filter((f) => f.userId === ctx.me().id && f.start <= now)
            .map((f) => toFlow(f, now))
            .filter((f) => !status || f.status === status)
            .filter(
              (f) =>
                !q ||
                (f.root_agent_name ?? '').toLowerCase().includes(q) ||
                (f.title ?? '').toLowerCase().includes(q),
            )
            .sort((a, b) => b.created_at.localeCompare(a.created_at))
      const page = rows.slice(offset, offset + limit)
      return HttpResponse.json({ data: page, total: page.length })
    }),
    http.get('/api/flows/:id/steps', ({ params }) => {
      if (!ctx.loggedIn()) return unauthorized()
      const f = find(String(params.id))
      if (!f) return undefined
      // list_steps checks the owner the same way.
      return own(f)
        ? HttpResponse.json(toSteps(f, ctx.now()))
        : new HttpResponse(null, { status: 404 })
    }),
    http.get('/api/flows/:id', ({ params }) => {
      if (!ctx.loggedIn()) return unauthorized()
      const f = find(String(params.id))
      if (!f) {
        // A request from the observability seed (Sessions, the trace page); otherwise chat's routed-turn flows answer.
        const seeded = ctx.seedTrace?.(String(params.id))
        return seeded
          ? HttpResponse.json({ flow: seedFlow(seeded, ctx.me().id), steps: [] })
          : undefined
      }
      if (!own(f)) return new HttpResponse(null, { status: 404 })
      const now = ctx.now()
      return HttpResponse.json({ flow: toFlow(f, now), steps: toSteps(f, now) })
    }),
    http.get(`${OBS}/trace/:traceId`, ({ params }) => {
      const f = find(String(params.traceId))
      if (!f) return undefined
      if (!ctx.loggedIn()) return unauthorized()
      // Another user's flow: its trace isn't theirs to read here either.
      if (!own(f)) return text(`trace '${f.id}' not found`, 404)
      return traceStore(() => {
        const t = traceDetail(f, ctx.now())
        return t
          ? HttpResponse.json({ data: { trace: t } })
          : text(`trace '${f.id}' not found`, 404)
      })
    }),
    http.get(`${OBS}/span/:traceId/:spanId`, ({ params }) => {
      const f = find(String(params.traceId))
      if (!f) return undefined
      if (!ctx.loggedIn()) return unauthorized()
      if (!own(f)) return text(`span '${String(params.spanId)}' in trace '${f.id}' not found`, 404)
      return traceStore(() => {
        const s = spanDetail(f, String(params.spanId), ctx.now())
        return s
          ? HttpResponse.json({ data: { span: s } })
          : text(`span '${String(params.spanId)}' in trace '${f.id}' not found`, 404)
      })
    }),
  ]
}
