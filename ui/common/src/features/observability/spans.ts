/**
 * Pure span derivations shared by mock and live mode (no fetching, no React).
 *
 * The server's `TraceDetail.spans` is a tree of root nodes; `span_lookup` is the flat map
 * keyed by base64 `id`. `span_kind` is the OTel kind, so what a span *is* (LLM call, tool,
 * agent call) comes from `classifySpan`.
 */
import type { SpanNode, TraceDetail } from './types'

export type SpanClass = 'llm' | 'tool' | 'agent' | 'planner' | 'other'

/** Carries the whole trace's usage (service.rs `make_node`): excluded from every span sum. */
export const TRACE_TOTAL_SPAN = 'coding_agent.turn'

/**
 * Span attributes as flat dotted keys. The server un-flattens them on dots (service.rs `unflatten_attrs`:
 * `{gen_ai: {operation: {name}}}`); older data and hand-built fixtures may already be flat, so both forms read the
 * same. Arrays and scalars are leaves.
 */
export function flattenAttributes(attrs: unknown): Record<string, unknown> {
  const out: Record<string, unknown> = {}
  const walk = (v: unknown, prefix: string) => {
    if (v && typeof v === 'object' && !Array.isArray(v)) {
      const entries = Object.entries(v as Record<string, unknown>)
      if (!entries.length && prefix) out[prefix] = v
      for (const [k, child] of entries) walk(child, prefix ? `${prefix}.${k}` : k)
    } else if (prefix) out[prefix] = v
  }
  walk(attrs, '')
  return out
}

const AGENT_CALL = /^a2a\.(proxy|dispatch)\b/
const TOOL_NAME = /^(tool\.|execute_tool\b)/

export function classifySpan(
  span: Pick<SpanNode, 'name' | 'model' | 'provider' | 'operation' | 'parent_id' | 'span_kind'>,
  hasChildren = false,
): SpanClass {
  if (span.model || span.provider) return 'llm'
  if (AGENT_CALL.test(span.name)) return 'agent'
  if (
    TOOL_NAME.test(span.name) ||
    span.operation === 'execute_tool' ||
    (span.span_kind === 'client' && !!span.operation && !hasChildren)
  )
    return 'tool'
  if (!span.parent_id || (span.span_kind === 'internal' && hasChildren)) return 'planner'
  return 'other'
}

export const CLASS_LABEL: Record<SpanClass, string> = {
  llm: 'LLM',
  tool: 'Tool',
  agent: 'Agent call',
  planner: 'Planner',
  other: 'Other',
}

/** A span flattened out of the tree, with its depth and timing relative to the trace start. */
export interface FlatSpan {
  node: SpanNode
  depth: number
  cls: SpanClass
  startMs: number
  durationMs: number
  childCount: number
}

function ts(iso: string | null | undefined): number | null {
  if (!iso) return null
  const t = Date.parse(iso)
  return Number.isNaN(t) ? null : t
}

/**
 * Depth-first flattening of the tree in `trace.spans`, children in start order. Nodes in
 * `span_lookup` that the tree doesn't reach (shouldn't happen, but the server builds the
 * two separately) are appended as extra roots so nothing silently disappears.
 */
export function flattenSpans(trace: Pick<TraceDetail, 'spans' | 'span_lookup'>): FlatSpan[] {
  const roots = trace.spans ?? []
  const all: { node: SpanNode; depth: number }[] = []
  const seen = new Set<string>()
  const walk = (node: SpanNode, depth: number) => {
    if (seen.has(node.id)) return
    seen.add(node.id)
    all.push({ node, depth })
    const kids = [...(node.children ?? [])].sort(
      (a, b) => (ts(a.start_time) ?? 0) - (ts(b.start_time) ?? 0),
    )
    for (const k of kids) walk(k, depth + 1)
  }
  for (const r of [...roots].sort((a, b) => (ts(a.start_time) ?? 0) - (ts(b.start_time) ?? 0)))
    walk(r, 0)
  for (const node of Object.values(trace.span_lookup ?? {})) if (!seen.has(node.id)) walk(node, 0)

  const origin = Math.min(...all.map((s) => ts(s.node.start_time) ?? Infinity))
  const base = Number.isFinite(origin) ? origin : 0
  return all.map(({ node, depth }) => {
    const start = ts(node.start_time)
    const end = ts(node.end_time)
    const duration = node.latency_ms ?? (start !== null && end !== null ? end - start : 0)
    const kids = node.children?.length ?? 0
    return {
      node,
      depth,
      cls: classifySpan(node, kids > 0),
      startMs: start === null ? 0 : start - base,
      durationMs: Math.max(0, duration),
      childCount: kids,
    }
  })
}

export function traceDurationMs(spans: FlatSpan[]): number {
  return spans.reduce((m, s) => Math.max(m, s.startMs + s.durationMs), 0)
}

export const isError = (s: Pick<SpanNode, 'status_code'>) => s.status_code === 'ERROR'

/** The span status as the UI prints it. */
export const statusText = (s: Pick<SpanNode, 'status_code'>) =>
  s.status_code === 'ERROR' ? 'error' : s.status_code === 'OK' ? 'ok' : 'unset'

/** A real tool invocation (not an HTTP/DB/memory client call that the class also colours as a tool). */
export const isToolCall = (s: Pick<SpanNode, 'name' | 'operation'>) =>
  TOOL_NAME.test(s.name) || s.operation === 'execute_tool'

/** `latency_ms` is rounded: a retry starting within this of the failure's end is still after it. */
const RETRY_GAP_TOLERANCE_MS = 1

/**
 * Errors that nothing recovered from: an ERROR span with no same-name sibling (same parent)
 * that started after it ENDED and succeeded. A retry that eventually worked is not a
 * failure; an overlapping parallel call with the same name is not a retry.
 */
export function unrecoveredErrors(spans: FlatSpan[]): FlatSpan[] {
  return spans.filter(
    (s) =>
      isError(s.node) &&
      !spans.some(
        (o) =>
          o !== s &&
          o.node.parent_id === s.node.parent_id &&
          o.node.name === s.node.name &&
          o.startMs >= s.startMs + s.durationMs - RETRY_GAP_TOLERANCE_MS &&
          !isError(o.node),
      ),
  )
}

/**
 * Failing = the trace has an unrecovered error in ANY span (roots alone undercount: OTel
 * rarely propagates child errors to the root).
 */
export function isTraceFailing(trace: Pick<TraceDetail, 'spans' | 'span_lookup'>): boolean {
  return unrecoveredErrors(flattenSpans(trace)).length > 0
}

/** Token counts that are safe to sum: `coding_agent.turn` repeats the trace total. */
export function ownTokens(s: Pick<SpanNode, 'name' | 'input_tokens' | 'output_tokens'>): number {
  return s.name === TRACE_TOTAL_SPAN ? 0 : s.input_tokens + s.output_tokens
}

export function tokenSplit(spans: FlatSpan[]): {
  total: number
  byClass: Record<SpanClass, number>
} {
  const byClass: Record<SpanClass, number> = { llm: 0, tool: 0, agent: 0, planner: 0, other: 0 }
  let total = 0
  for (const s of spans) {
    const t = ownTokens(s.node)
    byClass[s.cls] += t
    total += t
  }
  return { total, byClass }
}

/** A run of ≥2 sibling spans with the same name, at least one failing before the last: a retry loop. */
export interface RetryLoop {
  name: string
  attempts: FlatSpan[]
  failures: number
  /** Largest gap between one attempt ending and the next starting. */
  maxGapMs: number
  /** LLM/planner spans started between the first and last attempt (the re-plans). */
  between: FlatSpan[]
}

export function findRetryLoops(spans: FlatSpan[]): RetryLoop[] {
  const byParent = new Map<string, FlatSpan[]>()
  for (const s of spans) {
    const key = `${s.node.parent_id ?? ''}|${s.node.name}`
    if (s.cls !== 'tool' && s.cls !== 'agent') continue
    byParent.set(key, [...(byParent.get(key) ?? []), s])
  }
  const loops: RetryLoop[] = []
  for (const attempts of byParent.values()) {
    if (attempts.length < 2) continue
    const failures = attempts.filter((a) => isError(a.node)).length
    const failedBeforeLast = attempts.slice(0, -1).some((a) => isError(a.node))
    if (!failedBeforeLast) continue
    let maxGapMs = 0
    for (let i = 1; i < attempts.length; i++)
      maxGapMs = Math.max(
        maxGapMs,
        attempts[i].startMs - (attempts[i - 1].startMs + attempts[i - 1].durationMs),
      )
    const first = attempts[0].startMs
    const last = attempts[attempts.length - 1].startMs
    const attemptIds = new Set(attempts.map((a) => a.node.id))
    const parent = attempts[0].node.parent_id
    // Re-plans: LLM/planner spans inside an attempt, or siblings started between attempts.
    const between = spans.filter(
      (s) =>
        (s.cls === 'llm' || s.cls === 'planner') &&
        s.node.name !== TRACE_TOTAL_SPAN &&
        ((s.node.parent_id != null && attemptIds.has(s.node.parent_id)) ||
          (s.node.parent_id === parent && s.startMs > first && s.startMs < last)),
    )
    loops.push({
      name: attempts[0].node.name,
      attempts,
      failures,
      maxGapMs: Math.max(0, maxGapMs),
      between,
    })
  }
  return loops.sort((a, b) => b.attempts.length - a.attempts.length)
}

/** Default selection: the failing span (latest first), else most tokens, else slowest. */
export function defaultSpan(spans: FlatSpan[]): FlatSpan | undefined {
  const leafish = spans.filter((s) => s.node.name !== TRACE_TOTAL_SPAN)
  const failing = unrecoveredErrors(leafish)
  const anyError = failing.length ? failing : leafish.filter((s) => isError(s.node))
  if (anyError.length) return anyError.reduce((a, b) => (b.startMs >= a.startMs ? b : a))
  const byTokens = [...leafish].sort((a, b) => ownTokens(b.node) - ownTokens(a.node))
  if (byTokens.length && ownTokens(byTokens[0].node) > 0) return byTokens[0]
  return [...leafish].sort((a, b) => b.durationMs - a.durationMs)[0]
}

/** Rows for the tree: consecutive same-name siblings collapse into one "×N" group row. */
export type TreeRow =
  | { kind: 'span'; span: FlatSpan; attempt?: { index: number; of: number } }
  | {
      kind: 'group'
      key: string
      name: string
      depth: number
      cls: SpanClass
      members: FlatSpan[]
      failures: number
      startMs: number
      durationMs: number
    }

export function treeRows(
  spans: FlatSpan[],
  expanded: ReadonlySet<string>,
  collapsed: ReadonlySet<string>,
): TreeRow[] {
  // Same-name siblings (same parent) form one group, placed at the first member, even when
  // other spans (a cascade, a re-plan) start between the attempts.
  const groupOf = new Map<string, FlatSpan[]>()
  for (const s of spans) {
    const key = `${s.node.parent_id ?? ''}|${s.node.name}`
    groupOf.set(key, [...(groupOf.get(key) ?? []), s])
  }
  const skip = new Set<string>()
  const rows: TreeRow[] = []
  let hiddenBelow: number | null = null
  let skippingBelow: number | null = null
  for (const s of spans) {
    if (hiddenBelow !== null) {
      if (s.depth > hiddenBelow) continue
      hiddenBelow = null
    }
    if (skippingBelow !== null) {
      if (s.depth > skippingBelow) continue
      skippingBelow = null
    }
    if (skip.has(s.node.id)) {
      skippingBelow = s.depth
      continue
    }
    const key = `${s.node.parent_id ?? ''}|${s.node.name}`
    const members = groupOf.get(key) ?? [s]
    if (members.length > 1) {
      for (const m of members) skip.add(m.node.id)
      const end = Math.max(...members.map((m) => m.startMs + m.durationMs))
      rows.push({
        kind: 'group',
        key,
        name: s.node.name,
        depth: s.depth,
        cls: s.cls,
        members,
        failures: members.filter((m) => isError(m.node)).length,
        startMs: s.startMs,
        durationMs: end - s.startMs,
      })
      if (expanded.has(key)) {
        members.forEach((m, i) => {
          rows.push({
            kind: 'span',
            span: { ...m, depth: m.depth + 1 },
            attempt: { index: i + 1, of: members.length },
          })
          // Each attempt keeps its own subtree (e.g. the LLM call inside a tool call).
          if (m.childCount && !collapsed.has(m.node.id)) {
            let hidden: number | null = null
            for (const d of subtree(spans, m)) {
              if (hidden !== null && d.depth > hidden) continue
              hidden = d.childCount && collapsed.has(d.node.id) ? d.depth : null
              rows.push({ kind: 'span', span: { ...d, depth: d.depth + 1 } })
            }
          }
        })
      }
      skippingBelow = s.depth
      continue
    }
    rows.push({ kind: 'span', span: s })
    if (s.childCount && collapsed.has(s.node.id)) hiddenBelow = s.depth
  }
  return rows
}

/** A span's descendants in tree order (`spans` is depth-first, as flattenSpans returns it). */
function subtree(spans: FlatSpan[], root: FlatSpan): FlatSpan[] {
  const i = spans.indexOf(root)
  const out: FlatSpan[] = []
  for (let j = i + 1; j < spans.length && spans[j].depth > root.depth; j++) out.push(spans[j])
  return out
}

/** The group key a span belongs to, if it is one of several same-name siblings. */
export function groupKeyOf(spans: FlatSpan[], id: string): string | undefined {
  const target = spans.find((s) => s.node.id === id)
  if (!target) return undefined
  const siblings = spans.filter(
    (s) =>
      s.node.parent_id === target.node.parent_id &&
      s.node.name === target.node.name &&
      s.depth === target.depth,
  )
  return siblings.length > 1 ? `${target.node.parent_id ?? ''}|${target.node.name}` : undefined
}

/** Group keys of a span and of every ancestor, so a deep-linked child opens its groups. */
export function groupKeysOf(spans: FlatSpan[], id: string): string[] {
  const byId = new Map(spans.map((s) => [s.node.id, s]))
  const keys: string[] = []
  const seen = new Set<string>()
  for (
    let cur = byId.get(id);
    cur && !seen.has(cur.node.id);
    cur = cur.node.parent_id ? byId.get(cur.node.parent_id) : undefined
  ) {
    seen.add(cur.node.id)
    const key = groupKeyOf(spans, cur.node.id)
    if (key) keys.push(key)
  }
  return keys
}

/** base64("Span:" + hex) — how the server builds `SpanNode.id` (service.rs `encode_span_id`). */
export function encodeSpanId(hex: string): string {
  return btoa(`Span:${hex}`)
}
