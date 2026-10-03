/**
 * Mock observability data (sessions, traces, spans, logs), derived from the ONE seed so
 * every number agrees with TokenOps.
 *
 * Mirrors nasiko-server @ cb3aaf0c (oss/server/src/observability/service.rs):
 * - session/list reads chat_sessions: `created_at >= start_time`, `ORDER BY created_at DESC,
 *   session_id DESC`, limit clamped 1–100 (default 25), offset. `agent_id` is the RAW agent
 *   name, "" when unresolved (deleted agents). A session's cost sums its traces in
 *   [start_time, now].
 * - trace/{id}: `spans` is the tree of root nodes (children sorted by start), `span_lookup`
 *   the flat map keyed by base64 `id` ("Span:" + hex). `span_kind` is the OTel kind.
 *   Status strings are OK / ERROR / UNSET. A `coding_agent.turn` span carries the whole
 *   trace's usage (make_node), on SpanNode and SpanDetail alike.
 * - span/{trace}/{span} matches the raw hex span id.
 *
 * Seed shape for the demo:
 * - Chat sessions group chat traces per agent per UTC day (~20/day).
 * - Chat traffic = every trace except workflow (MAF) traces on non-spike days: those have
 *   no chat session, so the "Chat sessions only" note is honest. The spike day is all chat,
 *   so its session sum equals the TokenOps day total.
 * - The spike agent's costliest trace on the spike day is its own one-trace session: a
 *   retry loop on a failing tool plus a cross-agent cascade through the proxy.
 * - One trace has a `coding_agent.turn` root; one agent has content capture off.
 *
 * Import-light on purpose (only types): like seed.ts, keep it pure and deterministic.
 */
import type {
  LogLine,
  SessionDetail,
  SessionSummary,
  SpanDetail,
  SpanNode,
  TraceDetail,
  TraceEntry,
} from '@/features/observability/types'
import type { components } from '@/lib/api/schema.gen'
import { round6, type Seed, type SeedTrace } from './seed'
import {
  allTokens,
  b64,
  CAPTURE_OFF_AGENT,
  encodeSpanId,
  encodeTraceId,
  generateSpans,
  hash,
  iso,
  observabilityData,
  prng,
  spanAttributes,
  type GenSpan,
  type MockSession,
} from './spanBuilder'

// The session model and span generator live in spanBuilder.ts (shared with scripts/seed-live.ts).
export {
  encodeSpanId,
  generateSpans,
  observabilityData,
  SHOWCASE_SESSION,
  type MockSession,
} from './spanBuilder'

// ─── wire shapes ────────────────────────────────────────────────────────────

const content = (value: string) => ({ value, mime_type: 'text/plain', parsed_value: null })

/** Span content as service.rs get_span_details builds it: any value that parses as JSON is "json" and parsed, else "text". */
function spanContent(value: string): { value: string; mime_type: string; parsed_value: unknown } {
  let parsed: unknown = null
  try {
    parsed = value ? JSON.parse(value) : null
  } catch {
    // Plain text.
  }
  return { value, mime_type: parsed === null ? 'text' : 'json', parsed_value: parsed }
}

/** service.rs `unflatten_attrs`: dotted keys become nested objects; a key under a scalar is dropped, as there. */
function unflatten(attrs: Record<string, unknown>): Record<string, unknown> {
  const root: Record<string, unknown> = {}
  for (const [key, value] of Object.entries(attrs)) {
    const parts = key.split('.')
    let map = root
    for (const p of parts.slice(0, -1)) {
      const next = (map[p] ??= {})
      if (!next || typeof next !== 'object') {
        map = {}
        break
      }
      map = next as Record<string, unknown>
    }
    map[parts[parts.length - 1]!] = value
  }
  return root
}

function toNode(s: GenSpan): SpanNode {
  return {
    id: encodeSpanId(s.hex),
    span_id: s.hex,
    name: s.name,
    span_kind: s.kind,
    status_code: s.status,
    start_time: iso(s.start),
    end_time: iso(s.end),
    parent_id: s.parentHex ? encodeSpanId(s.parentHex) : null,
    latency_ms: s.end - s.start,
    token_count_total: s.input + s.output + s.cacheRead + s.cacheCreation,
    input_tokens: s.input,
    output_tokens: s.output,
    cache_read_tokens: s.cacheRead,
    cache_creation_tokens: s.cacheCreation,
    model: s.model,
    operation: s.operation,
    provider: s.provider,
    span_annotation_summaries: [],
    children: [],
  }
}

export function traceDetail(seed: Seed, traceId: string): TraceDetail | null {
  const found = observabilityData(seed).traceById.get(traceId)
  if (!found) return null
  const { trace: t, session } = found
  const gen = generateSpans(seed, traceId)
  const nodes = new Map(gen.map((s) => [s.hex, toNode(s)]))
  const lookup: Record<string, SpanNode> = {}
  for (const s of gen) lookup[encodeSpanId(s.hex)] = toNode(s)
  const kids = new Map<string, string[]>()
  for (const s of gen)
    if (s.parentHex && nodes.has(s.parentHex))
      kids.set(s.parentHex, [...(kids.get(s.parentHex) ?? []), s.hex])
  const attach = (hex: string): SpanNode => {
    const node = nodes.get(hex)!
    node.children = (kids.get(hex) ?? [])
      .map(attach)
      .sort((a, b) => (a.start_time ?? '').localeCompare(b.start_time ?? ''))
    return node
  }
  const roots = gen.filter((s) => !s.parentHex || !nodes.has(s.parentHex)).map((s) => attach(s.hex))
  const start = Math.min(...gen.map((s) => s.start))
  const end = Math.max(...gen.map((s) => s.end))
  return {
    id: t.trace_id,
    project_session_id: session.session_id,
    num_spans: gen.length,
    latency_ms: end - start,
    cost_summary: {
      total: { cost: t.cost_usd },
      prompt: { cost: t.prompt_cost_usd },
      completion: { cost: t.completion_cost_usd },
      cache_read: { cost: 0 },
      cache_creation: { cost: 0 },
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

export function spanDetail(seed: Seed, traceId: string, hex: string): SpanDetail | null {
  const found = observabilityData(seed).traceById.get(traceId)
  if (!found) return null
  const gen = generateSpans(seed, traceId)
  const s = gen.find((x) => x.hex === hex)
  if (!s) return null
  const t = found.trace
  const attrs = spanAttributes(s, found.session)
  const first = (...keys: string[]) =>
    keys.map((k) => attrs[k]).find((v) => typeof v === 'string' && v) as string | undefined
  const whole = s.name === 'coding_agent.turn'
  const cw = (cost: number, tokens: number) => ({ cost, tokens })
  return {
    id: encodeSpanId(s.hex),
    span_id: s.hex,
    trace: { id: t.trace_id, trace_id: t.trace_id },
    name: s.name,
    span_kind: s.kind,
    status_code: s.status,
    code: s.status,
    status_message: s.statusMessage,
    start_time: iso(s.start),
    end_time: iso(s.end),
    // SpanDetail.parent_id is the RAW hex id (unlike SpanNode's base64).
    parent_id: s.parentHex ?? null,
    latency_ms: s.end - s.start,
    token_count_total: s.input + s.output + s.cacheRead + s.cacheCreation,
    provider: s.provider,
    model: s.model,
    cache_read_tokens: s.cacheRead,
    cache_creation_tokens: s.cacheCreation,
    cost_summary: whole
      ? {
          total: cw(t.cost_usd, t.input_tokens + t.output_tokens),
          prompt: cw(t.prompt_cost_usd, t.input_tokens),
          completion: cw(t.completion_cost_usd, t.output_tokens),
          cache_read: cw(0, t.cache_read_tokens),
          cache_creation: cw(0, t.cache_creation_tokens),
        }
      : {
          total: cw(s.cost, s.input + s.output),
          prompt: cw(s.promptCost, s.input),
          completion: cw(round6(s.cost - s.promptCost), s.output),
          cache_read: cw(0, s.cacheRead),
          cache_creation: cw(0, s.cacheCreation),
        },
    // get_span_details' precedence at ea233d20 (Loki content comes next; the mock has none).
    input: spanContent(first('input.value', 'gen_ai.input.messages', 'tool.arguments') ?? ''),
    output: spanContent(
      first('output.value', 'gen_ai.output.messages', 'tool.result', 'error.message') ?? '',
    ),
    attributes: unflatten(attrs) as Record<string, never>,
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

/** The session's traces whose start falls in [start, now]. */
function visibleTraces(s: MockSession, start: number, now: number): SeedTrace[] {
  return s.traces.filter((t) => t.ts >= start && t.ts <= now)
}

function sessionEnd(seed: Seed, s: MockSession): number {
  const last = s.traces[s.traces.length - 1]
  const spans = generateSpans(seed, last.trace_id)
  return spans.length ? Math.max(...spans.map((x) => x.end)) : last.ts + last.latency_ms
}

function sessionSummary(
  seed: Seed,
  s: MockSession,
  start: number,
  now: number,
  tempoDown = false,
): SessionSummary {
  const traces = visibleTraces(s, start, now)
  const agentName = s.agent.deleted ? '' : s.agent.name
  const base = {
    id: s.session_id,
    session_id: s.session_id,
    agent_id: agentName,
    session_annotations: [],
    session_annotation_summaries: [],
  }
  if (tempoDown || !traces.length) {
    // DB-only fallback row (service.rs: provider NotFound/unavailable → minimal summary).
    return {
      ...base,
      num_traces: null,
      start_time: iso(s.created),
      end_time: iso(s.created),
      duration_ms: null,
      first_input: null,
      last_output: null,
      token_usage: { total: null },
      trace_latency_ms_p50: null,
      trace_latency_ms_p99: null,
      cost_summary: { total: { cost: null } },
    }
  }
  const end = sessionEnd(seed, s)
  const lat = traces.map((t) => t.latency_ms).sort((a, b) => a - b)
  const pct = (q: number) =>
    lat[Math.min(lat.length - 1, Math.max(0, Math.ceil(q * lat.length) - 1))]
  return {
    ...base,
    num_traces: traces.length,
    start_time: iso(traces[0].ts),
    end_time: iso(end),
    duration_ms: end - traces[0].ts,
    first_input: s.firstInput,
    last_output: s.lastOutput,
    token_usage: { total: traces.reduce((acc, t) => acc + allTokens(t), 0) },
    trace_latency_ms_p50: pct(0.5),
    trace_latency_ms_p99: pct(0.99),
    cost_summary: { total: { cost: round6(traces.reduce((acc, t) => acc + t.cost_usd, 0)) } },
  } satisfies SessionSummary
}

export function sessionDetail(seed: Seed, s: MockSession): SessionDetail {
  const traces = s.traces
  const sum = (f: (t: SeedTrace) => number) => traces.reduce((acc, t) => acc + f(t), 0)
  const cw = (cost: number, tokens: number) => ({ cost: round6(cost), tokens })
  const lat = traces.map((t) => t.latency_ms).sort((a, b) => a - b)
  const entries: TraceEntry[] = traces.map((t, i) => {
    const root = generateSpans(seed, t.trace_id)[0]
    return {
      id: encodeTraceId(t.trace_id),
      trace_id: t.trace_id,
      cursor: b64(`connection:${i}`),
      root_span: {
        id: encodeSpanId(root.hex),
        span_id: root.hex,
        attributes: JSON.stringify(root.attrs),
        cumulative_token_count_total: allTokens(t),
        input_tokens: t.input_tokens,
        output_tokens: t.output_tokens,
        cache_read_tokens: t.cache_read_tokens,
        cache_creation_tokens: t.cache_creation_tokens,
        latency_ms: root.end - root.start,
        start_time: iso(root.start),
        span_annotations: [],
        span_annotation_summaries: [],
        project: { id: 'default' },
        input: content(s.agent.name === CAPTURE_OFF_AGENT ? '' : s.firstInput),
        output: content(s.agent.name === CAPTURE_OFF_AGENT ? '' : s.lastOutput),
        trace: {
          id: encodeTraceId(t.trace_id),
          cost_summary: { total: { cost: t.cost_usd } } as unknown as Record<string, never>,
        },
      },
    }
  })
  return {
    id: s.session_id,
    session_id: s.session_id,
    title: s.title,
    agent_name: s.agent.deleted ? null : s.agent.name,
    num_traces: traces.length,
    token_usage: { total: sum(allTokens) },
    cost_summary: {
      total: cw(
        sum((t) => t.cost_usd),
        sum(allTokens),
      ),
      prompt: cw(
        sum((t) => t.prompt_cost_usd),
        sum((t) => t.input_tokens),
      ),
      completion: cw(
        sum((t) => t.completion_cost_usd),
        sum((t) => t.output_tokens),
      ),
      cache_read: cw(
        0,
        sum((t) => t.cache_read_tokens),
      ),
      cache_creation: cw(
        0,
        sum((t) => t.cache_creation_tokens),
      ),
    },
    latency_p50: lat[Math.floor(lat.length / 2)] ?? null,
    latency_p99: lat[lat.length - 1] ?? null,
    latency_avg: lat.length ? sum((t) => t.latency_ms) / lat.length : null,
    cache_read_tokens: sum((t) => t.cache_read_tokens),
    cache_creation_tokens: sum((t) => t.cache_creation_tokens),
    metrics_complete: true,
    traces: entries,
    pagination: { end_cursor: null, has_next_page: false },
  } satisfies SessionDetail
}

/** GET /session/list: created_at >= start_time, newest first, limit clamped 1–100. */
export function sessionList(
  seed: Seed,
  p: { start_time?: string | null; limit?: string | null; offset?: string | null },
  now: number,
  tempoDown = false,
) {
  const parsed = p.start_time ? Date.parse(p.start_time) : Number.NaN
  const start = Number.isNaN(parsed) ? now - 7 * 86_400_000 : parsed
  const limit = Math.min(100, Math.max(1, Number.parseInt(p.limit ?? '', 10) || 25))
  const offset = Math.max(0, Number.parseInt(p.offset ?? '', 10) || 0)
  const data = observabilityData(seed)
  const inWindow = data.sessions.filter((s) => s.created >= start && s.created <= now)
  const page = inWindow.slice(offset, offset + limit)
  const sessions = page.map((s) => sessionSummary(seed, s, start, now, tempoDown))
  return {
    data: {
      sessions,
      total_agents: seed.agents.filter((a) => !a.deleted).length,
      successful_agents: sessions.filter((s) => s.num_traces != null).length,
      pagination: {
        end_cursor: inWindow.length > offset + limit ? String(offset + limit) : null,
        has_next_page: inWindow.length > offset + limit,
      },
    },
  }
}

/** Deterministic log lines for an agent, spread over the last hour before `now`. */
export function agentLogs(seed: Seed, agentRef: string, now: number, count = 30): LogLine[] | null {
  // routes.rs resolves agent_ref as a UUID or the exact agent name only.
  const agent = seed.agents.find((a) => a.name === agentRef || a.id === agentRef)
  if (!agent || agent.deleted) return null
  const rand = prng(hash(agent.name))
  const msgs = [
    'POST /a2a message/send 200',
    'tool call completed',
    'LLM call finished',
    'session resumed',
    'upstream latency high',
    'retrying tool call',
  ]
  return Array.from({ length: count }, (_, i) => {
    const ts = now - (count - i) * 90_000 - Math.floor(rand() * 60_000)
    const m = msgs[Math.floor(rand() * msgs.length)]
    return {
      timestamp: iso(ts),
      level: m.includes('retry') || m.includes('high') ? 'WARN' : 'INFO',
      message: `${agent.name}: ${m}`,
      source: i % 3 === 0 ? 'proxy' : 'container',
      trace_id: null,
    }
  })
}

/** GET /api/agents (superuser: all live agents). Deleted agents are not listed. */
export function agentsList(
  seed: Seed,
  p: { limit?: string | null; offset?: string | null } = {},
): components['schemas']['Agent'][] {
  // catalog/routes.rs: limit defaults to 50, clamped 1–100; offset paging.
  const limit = Math.min(100, Math.max(1, Number.parseInt(p.limit ?? '', 10) || 50))
  const offset = Math.max(0, Number.parseInt(p.offset ?? '', 10) || 0)
  return seed.agents
    .filter((a) => !a.deleted)
    .slice(offset, offset + limit)
    .map((a) => ({
      id: a.id,
      name: a.name,
      display_name: a.display_name,
      version: a.version,
      status: 'running',
      owner_id: '5eed0000-0000-4000-8000-00000000a001',
      created_at: seed.anchor,
      updated_at: seed.anchor,
      capabilities: {},
      metadata: {},
      security_schemes: {},
      default_input_modes: ['text'],
      default_output_modes: ['text'],
      preferred_transport: 'JSONRPC',
      protocol_version: '0.3.0',
      skills: [],
      tags: [],
    })) satisfies components['schemas']['Agent'][]
}
