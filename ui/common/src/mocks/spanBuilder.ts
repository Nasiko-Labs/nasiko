/**
 * The seed's chat sessions and their span trees, shared by the observability mock (src/mocks/observability.ts, which
 * turns them into server responses) and the live seed (scripts/seed-live.ts, which posts them to Tempo as OTLP, plan
 * feat-live-contract §6). Import-free apart from ./seed.ts and erasable-only, so Node runs it directly; like seed.ts,
 * keep it pure and deterministic.
 */
import { round6, utcDate, type Seed, type SeedAgent, type SeedTrace } from './seed.ts'

/** The costliest of the spike agent's PR-review sessions on the spike day (see below). */
export const SHOWCASE_SESSION = '5eed-sess-pr-481'
/** The spike agent's spike-day traffic is split into this many PR-review sessions. */
const STORM_SESSIONS = 6
const CODING_AGENT = 'seed-doc-writer'
export const CAPTURE_OFF_AGENT = 'seed-hr-helpdesk'
const CALLEE = 'seed-qa-tester'

export interface MockSession {
  session_id: string
  agent: SeedAgent
  traces: SeedTrace[]
  created: number
  title: string
  firstInput: string
  lastOutput: string
  showcase: boolean
}

export interface ObservabilityData {
  sessions: MockSession[]
  byId: Map<string, MockSession>
  traceById: Map<string, { trace: SeedTrace; session: MockSession }>
  showcaseTraceId: string
  /** Traces with the retry-storm span structure (the spike agent on the spike day). */
  storm: Set<string>
  codingTraceId: string | null
  /** Traces with no chat session (MAF workflows on non-spike days). */
  nonChat: SeedTrace[]
}

// ─── deterministic helpers ──────────────────────────────────────────────────

export function hash(s: string): number {
  let h = 2166136261
  for (let i = 0; i < s.length; i++) h = Math.imul(h ^ s.charCodeAt(i), 16777619)
  return h >>> 0
}

export function prng(seed: number): () => number {
  let a = seed >>> 0
  return () => {
    a = (a + 0x6d2b79f5) >>> 0
    let t = a
    t = Math.imul(t ^ (t >>> 15), t | 1)
    t ^= t + Math.imul(t ^ (t >>> 7), t | 61)
    return ((t ^ (t >>> 14)) >>> 0) / 4294967296
  }
}

function hexId(rand: () => number, n = 16): string {
  let s = ''
  for (let i = 0; i < n; i++) s += Math.floor(rand() * 16).toString(16)
  return s
}

/** base64("Span:" + hex), like service.rs `encode_span_id`. Works in Node and browsers. */
export function encodeSpanId(hex: string): string {
  const raw = `Span:${hex}`
  return typeof btoa === 'function' ? btoa(raw) : Buffer.from(raw).toString('base64')
}

export const iso = (ms: number) => new Date(ms).toISOString()
/** The server's token totals include cache reads and writes (service.rs session/trace sums). */
export const allTokens = (t: SeedTrace) =>
  t.input_tokens + t.output_tokens + t.cache_read_tokens + t.cache_creation_tokens
export const b64 = (raw: string) =>
  typeof btoa === 'function' ? btoa(raw) : Buffer.from(raw).toString('base64')
/** base64("Trace:" + id), like service.rs `encode_trace_id` (TraceEntry.id, TraceRef.id). */
export const encodeTraceId = (id: string) => b64(`Trace:${id}`)

// ─── personas ───────────────────────────────────────────────────────────────

const PERSONA: Record<
  string,
  { asks: string[]; answer: string; tools: string[]; everyTool?: boolean }
> = {
  'seed-support-bot': {
    asks: [
      "Customer can't reset their password",
      'Refund request for order #88213',
      'Why was I charged twice?',
    ],
    answer: 'Resolved and replied to the customer.',
    tools: ['tool.lookup_account', 'tool.search_kb'],
  },
  'seed-research-agent': {
    asks: [
      'Summarise recent papers on RAG evaluation',
      'Compare vector DBs for 10M docs',
      'What changed in the EU AI Act this month?',
    ],
    answer: 'Wrote a sourced summary with 6 citations.',
    // The long traces (8 steps, ~40 spans): every research run calls each tool once.
    everyTool: true,
    tools: [
      'tool.web_search',
      'tool.fetch_url',
      'tool.extract_pdf',
      'tool.web_search_followup',
      'tool.fetch_url_secondary',
      'tool.dedupe_sources',
      'tool.extract_citations',
      'tool.write_notes',
    ],
  },
  'seed-code-reviewer': {
    asks: [
      'Review PR #481 for race conditions',
      'Review PR #479: retry middleware',
      'Check PR #476 for SQL injection',
    ],
    answer: 'Left 4 review comments and requested changes.',
    tools: ['tool.get_diff', 'tool.run_linter'],
  },
  'seed-sql-analyst': {
    asks: [
      'Weekly active users by plan',
      'Why did churn jump in March?',
      'Top 10 accounts by expansion revenue',
    ],
    answer: 'Returned the query and a 3-line summary.',
    tools: ['tool.run_sql', 'tool.describe_table'],
  },
  'seed-triage-router': {
    asks: [
      'Route: "app crashes on login"',
      'Route: "invoice missing VAT"',
      'Route: "need SSO for 400 seats"',
    ],
    answer: 'Routed to the right team with priority.',
    tools: ['tool.classify'],
  },
  'seed-doc-writer': {
    asks: [
      'Draft release notes for v2.14',
      'Write the webhook retry guide',
      'Update the SSO setup page',
    ],
    answer: 'Drafted the page for review.',
    tools: ['tool.read_repo', 'tool.write_file'],
  },
  'seed-sales-assistant': {
    asks: [
      'Prep notes for the Acme renewal call',
      'Draft a follow-up to Globex',
      'Summarise the Initech thread',
    ],
    answer: 'Prepared the brief.',
    tools: ['tool.crm_lookup'],
  },
  'seed-invoice-parser': {
    asks: [
      'Extract totals from INV-2231.pdf',
      'Parse the Contoso March invoices',
      'Validate VAT on INV-2244',
    ],
    answer: 'Extracted 12 line items; totals match.',
    tools: ['tool.ocr', 'tool.validate_vat'],
  },
  'seed-translator': {
    asks: ['Translate the onboarding email to German', 'Localise the pricing page for Japan'],
    answer: 'Translated and kept the formatting.',
    tools: [],
  },
  'seed-summarizer': {
    asks: ["Summarise yesterday's incident channel", 'TL;DR of the Q3 board deck'],
    answer: 'Wrote a 5-bullet summary.',
    tools: ['tool.fetch_url'],
  },
  'seed-qa-tester': {
    asks: ['Run the checkout regression suite', 'Smoke test the staging deploy'],
    answer: 'Ran 48 checks; 2 flaky.',
    tools: ['tool.run_tests'],
  },
}

function persona(agent: SeedAgent) {
  return (
    PERSONA[agent.name] ?? {
      asks: [`Help with a ${agent.display_name} task`],
      answer: 'Done.',
      tools: ['tool.lookup'],
    }
  )
}

// ─── sessions ───────────────────────────────────────────────────────────────

const cache = new WeakMap<Seed, ObservabilityData>()

export function observabilityData(seed: Seed): ObservabilityData {
  const hit = cache.get(seed)
  if (hit) return hit
  const agentById = new Map(seed.agents.map((a) => [a.id, a]))
  const nonChat: SeedTrace[] = []
  const chat: SeedTrace[] = []
  for (const t of seed.traces) {
    if (t.workflow_id && utcDate(t.started_at) !== seed.spikeDate) nonChat.push(t)
    else chat.push(t)
  }
  // The spike: the spike agent's spike-day traffic is PR reviews stuck in retry storms.
  // It is split into STORM_SESSIONS PR-review sessions (each ~1/6 of the spike, so they top
  // the day); every one of those traces retries a failing tool. The costliest is PR #481.
  const spikeTraces = chat.filter(
    (t) => t.agent_name === seed.spikeAgentName && utcDate(t.started_at) === seed.spikeDate,
  )
  const storm = new Set(spikeTraces.map((t) => t.trace_id))
  const chunks: SeedTrace[][] = []
  const size = Math.max(1, Math.ceil(spikeTraces.length / STORM_SESSIONS))
  for (let i = 0; i < spikeTraces.length; i += size) chunks.push(spikeTraces.slice(i, i + size))
  const chunkCost = (c: SeedTrace[]) => c.reduce((a, t) => a + t.cost_usd, 0)
  const byCost = [...chunks].sort((a, b) => chunkCost(b) - chunkCost(a))
  const prOf = new Map<string, number>()
  byCost.forEach((c, i) => {
    for (const t of c) prOf.set(t.trace_id, 481 - i * 2)
  })
  // coding_agent.turn: the doc writer's first trace on the day before the anchor.
  const dayBefore = utcDate(new Date(Date.parse(seed.anchor) - 86_400_000))
  const coding =
    chat.find((t) => t.agent_name === CODING_AGENT && utcDate(t.started_at) === dayBefore) ?? null

  // Everyone else: one chat session per agent per UTC day (an A2A contextId that lives for
  // the day). Keeps the fleet at ~20 sessions/day, so the live day scan (3 × 100 rows,
  // newest first) reaches the spike day; the seed's own session_id groups are ~130/day.
  const groups = new Map<string, SeedTrace[]>()
  for (const t of chat) {
    const pr = prOf.get(t.trace_id)
    const sid = pr
      ? `5eed-sess-pr-${pr}`
      : `5eed-sess-${t.agent_name.replace(/^seed-/, '')}-${utcDate(t.started_at)}`
    groups.set(sid, [...(groups.get(sid) ?? []), t])
  }
  const PR_ASKS = [
    'for race conditions',
    'retry middleware',
    'for SQL injection',
    'cache invalidation',
    'the auth refactor',
    'rate limiter tests',
    'flaky CI fix',
    'config loader',
    'error envelope',
    'pagination bug',
  ]
  const sessions: MockSession[] = []
  for (const [sid, traces] of groups) {
    const agent = agentById.get(traces[0].agent_id)!
    const p = persona(agent)
    const rand = prng(hash(sid))
    const pr = prOf.get(traces[0].trace_id)
    const ask = pr
      ? `Review PR #${pr} ${PR_ASKS[((481 - pr) / 2) % PR_ASKS.length]}`
      : p.asks[Math.floor(rand() * p.asks.length)]
    sessions.push({
      session_id: sid,
      agent,
      traces,
      created: traces[0].ts,
      title: ask.length > 72 ? `${ask.slice(0, 69)}...` : ask,
      firstInput: ask,
      lastOutput: pr
        ? "Couldn't fetch the diff: github.get_diff kept failing and the last attempt timed out."
        : p.answer,
      showcase: sid === SHOWCASE_SESSION,
    })
  }
  sessions.sort((a, b) => b.created - a.created || (b.session_id < a.session_id ? -1 : 1))
  const byId = new Map(sessions.map((s) => [s.session_id, s]))
  const traceById = new Map<string, { trace: SeedTrace; session: MockSession }>()
  for (const s of sessions)
    for (const t of s.traces) traceById.set(t.trace_id, { trace: t, session: s })
  const showcaseSession = byId.get(SHOWCASE_SESSION)
  const showcase = showcaseSession?.traces.reduce<SeedTrace | undefined>(
    (best, t) => (!best || t.cost_usd > best.cost_usd ? t : best),
    undefined,
  )
  const data: ObservabilityData = {
    sessions,
    byId,
    traceById,
    showcaseTraceId: showcase?.trace_id ?? '',
    storm,
    codingTraceId: coding?.trace_id ?? null,
    nonChat,
  }
  cache.set(seed, data)
  return data
}

// ─── spans ──────────────────────────────────────────────────────────────────

export interface GenSpan {
  hex: string
  parentHex: string | null
  name: string
  kind: 'internal' | 'server' | 'client'
  status: 'OK' | 'ERROR' | 'UNSET'
  statusMessage: string
  start: number
  end: number
  input: number
  output: number
  cacheRead: number
  cacheCreation: number
  cost: number
  promptCost: number
  completionCost: number
  model: string | null
  provider: string | null
  operation: string | null
  inText: string
  outText: string
  attrs: Record<string, unknown>
}

const spanCache = new Map<string, GenSpan[]>()

/** Split `total` into parts proportional to `weights`, rounded to 6 dp, remainder on the last. */
function allocate(total: number, weights: number[]): number[] {
  const w = weights.reduce((s, x) => s + x, 0) || 1
  const parts = weights.map((x) => round6((total * x) / w))
  const drift = round6(total - parts.reduce((s, x) => s + x, 0))
  parts[parts.length - 1] = round6(parts[parts.length - 1] + drift)
  return parts
}

function allocateInt(total: number, weights: number[]): number[] {
  const w = weights.reduce((s, x) => s + x, 0) || 1
  const parts = weights.map((x) => Math.floor((total * x) / w))
  parts[parts.length - 1] += total - parts.reduce((s, x) => s + x, 0)
  return parts
}

/** How a trace's tokens and cost split across its LLM spans (anything else weighs 1.4). */
const LLM_WEIGHT: Record<string, number> = {
  'llm.plan': 3,
  'llm.respond': 5,
  'llm.decide': 1.2,
  'llm.reflect': 1,
}

export function generateSpans(seed: Seed, traceId: string): GenSpan[] {
  const key = `${seed.anchor}|${traceId}`
  const hit = spanCache.get(key)
  if (hit) return hit
  const data = observabilityData(seed)
  const found = data.traceById.get(traceId)
  if (!found) return []
  const { trace: t, session } = found
  const rand = prng(hash(traceId))
  const p = persona(session.agent)
  const captureOff = session.agent.name === CAPTURE_OFF_AGENT
  const spans: GenSpan[] = []
  const blank = (s: Partial<GenSpan> & Pick<GenSpan, 'name' | 'start' | 'end'>): GenSpan => ({
    hex: hexId(rand),
    parentHex: null,
    kind: 'internal',
    status: 'UNSET',
    statusMessage: '',
    input: 0,
    output: 0,
    cacheRead: 0,
    cacheCreation: 0,
    cost: 0,
    promptCost: 0,
    completionCost: 0,
    model: null,
    provider: null,
    operation: null,
    inText: '',
    outText: '',
    attrs: {},
    ...s,
  })
  const text = (v: string) => (captureOff ? '' : v)
  const llm = (
    name: string,
    parentHex: string,
    start: number,
    dur: number,
    inText: string,
    outText: string,
  ) =>
    blank({
      name,
      parentHex,
      kind: 'client',
      start,
      end: start + dur,
      model: t.model,
      provider: t.provider,
      operation: 'chat',
      inText: text(inText),
      outText: text(outText),
      attrs: {
        'gen_ai.system': t.provider,
        'gen_ai.request.model': t.model,
        'gen_ai.operation.name': 'chat',
      },
    })
  const httpStatus = (msg: string) =>
    msg.includes('timed out')
      ? 504
      : msg.includes('permission denied')
        ? 403
        : msg.includes('503')
          ? 503
          : 500
  const tool = (
    name: string,
    parentHex: string,
    start: number,
    dur: number,
    ok: boolean,
    msg = '',
  ) =>
    blank({
      name,
      parentHex,
      start,
      end: start + dur,
      operation: 'execute_tool',
      status: ok ? 'OK' : 'ERROR',
      statusMessage: msg,
      inText: text(JSON.stringify({ tool: name.replace(/^tool\./, ''), args: { ref: 'main' } })),
      outText: text(ok ? '{"ok":true}' : msg),
      attrs: {
        'gen_ai.operation.name': 'execute_tool',
        'tool.name': name.replace(/^tool\./, ''),
        ...(ok ? {} : { 'http.status_code': httpStatus(msg) }),
      },
    })

  const isStorm = data.storm.has(traceId)
  const calleeId = seed.agents.find((a) => a.name === CALLEE)?.id ?? ''
  // The showcase trace always has the full story: 6 attempts, a timeout, a 3-call cascade.
  const isShowcase = traceId === data.showcaseTraceId
  const attemptsN = isShowcase ? 6 : 3 + Math.floor(rand() * 4)
  const timesOut = isShowcase || rand() < 0.5
  const calls = isShowcase ? 3 : 1 + Math.floor(rand() * 3)
  const isCoding = traceId === data.codingTraceId
  const t0 = t.ts
  const rootName = isCoding ? 'coding_agent.turn' : 'planner'
  const root = blank({
    name: rootName,
    kind: 'server',
    start: t0,
    end: t0,
    operation: 'invoke_agent',
    attrs: { 'gen_ai.operation.name': 'invoke_agent', 'gen_ai.agent.name': session.agent.name },
  })
  spans.push(root)
  const llms: GenSpan[] = []

  if (isStorm) {
    // Retry storm: github.get_diff fails with 500 again and again (each attempt asks the LLM
    // to repair its arguments), back-to-back with no backoff; often the last one times out.
    const plan = llm(
      'llm.plan',
      root.hex,
      t0 + 40,
      2100,
      session.firstInput,
      'Plan: fetch the diff, run the linter, ask QA Tester to run the race-condition suite.',
    )
    spans.push(plan)
    llms.push(plan)
    let cursor = plan.end + 60
    for (let a = 1; a <= attemptsN; a++) {
      const last = a === attemptsN
      const timeout = last && timesOut
      const dur = timeout ? 30_000 : 1_650 + Math.floor(rand() * 300)
      const attempt = tool(
        'tool.get_diff',
        root.hex,
        cursor,
        dur,
        false,
        timeout
          ? 'github.get_diff timed out after 30.0 s'
          : 'GitHub API returned 500 Internal Server Error',
      )
      attempt.name = 'tool.get_diff'
      spans.push(attempt)
      if (!last) {
        const fix = llm(
          'llm.repair_args',
          attempt.hex,
          cursor + 700,
          800,
          'The tool call failed with HTTP 500. Adjust the arguments and retry.',
          '{"ref":"refs/pull/481/head"}',
        )
        spans.push(fix)
        llms.push(fix)
      }
      cursor += dur + 25 // 25 ms between attempts: no backoff
      if (a === Math.min(3, attemptsN - 1)) {
        // Meanwhile the planner fans out to QA Tester through the proxy (A2A cascade).
        for (let c = 0; c < calls; c++) {
          const s = cursor - 400 + c * 1_300
          // Mirrors agent_proxy.rs: otel.kind=server, gen_ai.operation.name=invoke_agent, agent.id=<uuid>.
          const call = blank({
            name: 'a2a.proxy',
            parentHex: root.hex,
            kind: 'server',
            start: s,
            end: s + 1_150,
            operation: 'invoke_agent',
            status: 'OK',
            inText: text('Run the race-condition suite on PR #481'),
            outText: text(c === calls - 1 ? 'Suite passed; 1 flaky test.' : 'Queued.'),
            attrs: { 'gen_ai.operation.name': 'invoke_agent', 'agent.id': calleeId },
          })
          spans.push(call)
          const qa = llm(
            'llm.qa_summary',
            call.hex,
            s + 200,
            800,
            'Summarise the test run for the caller.',
            'Ran 12 checks; 1 flaky.',
          )
          spans.push(qa)
          llms.push(qa)
        }
      }
    }
    root.end = cursor
    root.status = 'UNSET'
  } else {
    // A realistic agent run (3 levels): guardrail, memory, retrieval (embedding + vector
    // search), a plan, one agent.step per tool (decide → tool → its HTTP call), then an
    // optional reflection, an output guardrail, the answer and a memory write.
    // Tool steps are named step.<n>.<tool>.
    const rint = (n: number) => Math.floor(rand() * n)
    const guardIn = blank({
      name: 'guardrail.input',
      parentHex: root.hex,
      start: t0 + 5,
      end: t0 + 25 + rint(40),
      operation: 'guardrail',
      status: 'OK',
      inText: text(session.firstInput),
      outText: text('{"allowed":true,"pii":false,"injection":false}'),
      attrs: { 'guardrail.name': 'pii-and-injection', 'guardrail.result': 'pass' },
    })
    spans.push(guardIn)
    let cursor = guardIn.end + 5
    const memRead = blank({
      name: 'memory.read',
      parentHex: root.hex,
      kind: 'client',
      start: cursor,
      end: cursor + 15 + rint(60),
      operation: 'memory',
      status: 'OK',
      attrs: {
        'db.system': 'redis',
        'db.operation': 'MGET',
        'memory.scope': 'session',
        'memory.keys': 1 + rint(4),
      },
    })
    spans.push(memRead)
    cursor = memRead.end + 5
    if (rand() < 0.75) {
      const retrieval = blank({
        name: 'retriever.search',
        parentHex: root.hex,
        kind: 'client',
        start: cursor,
        end: cursor,
        operation: 'retrieval',
        status: 'OK',
        inText: text(session.firstInput),
        attrs: { 'retrieval.top_k': 8, 'retrieval.collection': `${session.agent.name}-kb` },
      })
      const embed = blank({
        name: 'embeddings.create',
        parentHex: retrieval.hex,
        kind: 'client',
        start: cursor + 5,
        end: cursor + 65 + rint(120),
        operation: 'embeddings',
        status: 'OK',
        attrs: {
          'gen_ai.operation.name': 'embeddings',
          'gen_ai.system': 'openai',
          'gen_ai.request.model': 'text-embedding-3-small',
          'gen_ai.embeddings.dimension.count': 1536,
        },
      })
      const hits = 3 + rint(6)
      const search = blank({
        name: 'qdrant.search',
        parentHex: retrieval.hex,
        kind: 'client',
        start: embed.end + 5,
        end: embed.end + 25 + rint(80),
        operation: 'db.query',
        status: 'OK',
        attrs: {
          'db.system': 'qdrant',
          'db.operation': 'search',
          'db.collection.name': `${session.agent.name}-kb`,
          'retrieval.hits': hits,
        },
      })
      retrieval.end = search.end + 5
      retrieval.outText = text(`${hits} passages retrieved`)
      spans.push(retrieval, embed, search)
      cursor = retrieval.end + 10
    }
    const planDur = 400 + rint(1_600)
    const plan = llm('llm.plan', root.hex, cursor, planDur, session.firstInput, 'Plan the steps.')
    spans.push(plan)
    llms.push(plan)
    cursor = plan.end + 20
    const nTools = !p.tools.length
      ? 0
      : 'everyTool' in p && p.everyTool
        ? p.tools.length
        : 1 + rint(p.tools.length + 1)
    const r = rand()
    // ~6% of traces retry a flaky tool once (and recover); ~0.4% end with a failing tool.
    const retryOnce = nTools > 0 && r < 0.06
    const failFinal = nTools > 0 && !retryOnce && r < 0.064
    for (let i = 0; i < nTools; i++) {
      const name = p.tools[i % p.tools.length]
      const toolName = name.replace(/^tool\./, '')
      // Unique step names: same-name siblings would render as one retry group (×N).
      const step = blank({
        name: `step.${i + 1}.${toolName}`,
        parentHex: root.hex,
        start: cursor,
        end: cursor,
        operation: 'agent_step',
        attrs: { 'agent.step.index': i + 1, 'agent.step.tool': toolName },
      })
      spans.push(step)
      const decide = llm(
        'llm.decide',
        step.hex,
        cursor + 5,
        250 + rint(700),
        `Step ${i + 1}: choose the next tool for "${session.firstInput}".`,
        JSON.stringify({ tool: toolName, args: { ref: 'main' } }),
      )
      spans.push(decide)
      llms.push(decide)
      let at = decide.end + 10
      if (retryOnce && i === 0) {
        // The failed attempt and its retry are siblings under the same step (a recovery).
        const first = tool(name, step.hex, at, 300 + rint(500), false, 'Upstream returned 503')
        spans.push(first)
        at = first.end + 400 + rint(600) // backed off before retrying
      }
      const ok = !(failFinal && i === nTools - 1)
      const call = tool(
        name,
        step.hex,
        at,
        150 + rint(900),
        ok,
        ok ? '' : 'Tool returned an error: permission denied',
      )
      spans.push(call)
      // The tool span owns the failure (the narrative and default selection name the tool);
      // its HTTP child records the same status code without a second error status.
      spans.push(
        blank({
          name: 'http.request',
          parentHex: call.hex,
          kind: 'client',
          start: call.start + 10,
          end: Math.max(call.start + 20, call.end - 10),
          status: ok ? 'OK' : 'UNSET',
          operation: 'http',
          attrs: {
            'http.request.method': 'POST',
            'url.full': `https://tools.internal/${toolName}`,
            'http.response.status_code': ok ? 200 : httpStatus(call.statusMessage),
            'server.address': 'tools.internal',
          },
        }),
      )
      step.end = call.end + 10
      cursor = step.end + 10
    }
    if (rand() < 0.5) {
      const reflect = llm(
        'llm.reflect',
        root.hex,
        cursor,
        300 + rint(600),
        'Check the draft against the plan and the tool results.',
        'The plan is complete; no further tools needed.',
      )
      spans.push(reflect)
      llms.push(reflect)
      cursor = reflect.end + 10
    }
    const respondDur = Math.max(300, Math.round(t.latency_ms * 0.4))
    const respond = llm(
      'llm.respond',
      root.hex,
      cursor,
      respondDur,
      'Write the final answer.',
      session.lastOutput,
    )
    spans.push(respond)
    llms.push(respond)
    const guardOut = blank({
      name: 'guardrail.output',
      parentHex: root.hex,
      start: respond.end + 5,
      end: respond.end + 20 + rint(30),
      operation: 'guardrail',
      status: 'OK',
      inText: text(session.lastOutput),
      outText: text('{"allowed":true}'),
      attrs: { 'guardrail.name': 'toxicity-and-leakage', 'guardrail.result': 'pass' },
    })
    const memWrite = blank({
      name: 'memory.write',
      parentHex: root.hex,
      kind: 'client',
      start: guardOut.end + 5,
      end: guardOut.end + 15 + rint(40),
      operation: 'memory',
      status: 'OK',
      attrs: { 'db.system': 'redis', 'db.operation': 'SET', 'memory.scope': 'session' },
    })
    spans.push(guardOut, memWrite)
    root.end = memWrite.end + 5
  }

  // Tokens and money: the trace's totals, split across its LLM spans. Sums are exact.
  const weights = llms.map((s) => LLM_WEIGHT[s.name] ?? 1.4)
  const inputs = allocateInt(t.input_tokens, weights)
  const outputs = allocateInt(t.output_tokens, weights)
  const prompt = allocate(t.prompt_cost_usd, weights)
  const completion = allocate(t.completion_cost_usd, weights)
  llms.forEach((s, i) => {
    s.input = inputs[i]
    s.output = outputs[i]
    s.promptCost = prompt[i]
    s.completionCost = completion[i]
    s.cost = round6(prompt[i] + completion[i])
    s.attrs = {
      ...s.attrs,
      'gen_ai.usage.input_tokens': inputs[i],
      'gen_ai.usage.output_tokens': outputs[i],
      'gen_ai.request.temperature': 0.2,
      'gen_ai.request.max_tokens': 2048,
      'gen_ai.response.finish_reasons': '["stop"]',
    }
    if (i === 0) {
      s.cacheRead = t.cache_read_tokens
      s.cacheCreation = t.cache_creation_tokens
    }
  })
  const cost = llms.reduce((acc, s) => round6(acc + s.cost), 0)
  // Put any rounding drift on the last LLM span so the trace total is exact.
  if (llms.length)
    llms[llms.length - 1].cost = round6(llms[llms.length - 1].cost + round6(t.cost_usd - cost))
  if (isCoding) {
    // make_node assigns the whole trace's usage to coding_agent.turn (and SpanDetail its cost).
    root.input = t.input_tokens
    root.output = t.output_tokens
    root.cacheRead = t.cache_read_tokens
    root.cacheCreation = t.cache_creation_tokens
    root.model = t.model
  }
  spanCache.set(key, spans)
  return spans
}

// ─── span attributes (what the seed writes to Tempo, and what the mock's span detail un-flattens) ─────────────

/** OTel GenAI message JSON, as the server's own emitter writes it (coding_agent_otlp.rs). "" when there is no text. */
const messagesJson = (role: string, text: string) =>
  text ? JSON.stringify([{ role, parts: [{ type: 'text', content: text }] }]) : ''

/**
 * A span's attributes, flat and dotted, with empty values dropped: the generator's own attributes, usage on LLM spans
 * (`gen_ai.usage.*`, extractor keys at ea233d20, plus `nasiko.usage.prompt_convention` = "exclusive"), and on the
 * root the session id and conversation. A `coding_agent.turn` root carries no usage: the server sums the trace onto it.
 * Content is left out for the capture-off agent, like its session rows.
 */
export function spanAttributes(
  s: GenSpan,
  session: MockSession,
): Record<string, string | number | boolean> {
  const root = s.parentHex === null
  const captureOff = session.agent.name === CAPTURE_OFF_AGENT
  const usage =
    s.operation === 'chat'
      ? {
          'gen_ai.usage.input_tokens': s.input,
          'gen_ai.usage.output_tokens': s.output,
          'gen_ai.usage.cache_read_input_tokens': s.cacheRead || null,
          'gen_ai.usage.cache_creation_input_tokens': s.cacheCreation || null,
          'nasiko.usage.prompt_convention': 'exclusive',
          'gen_ai.provider.name': s.provider,
        }
      : {}
  const all: Record<string, unknown> = {
    ...s.attrs,
    ...usage,
    ...(root
      ? {
          'session.id': session.session_id,
          'gen_ai.input.messages': captureOff ? '' : messagesJson('user', session.firstInput),
          'gen_ai.output.messages': captureOff ? '' : messagesJson('assistant', session.lastOutput),
        }
      : {}),
    'input.value': s.inText,
    'output.value': s.outText,
    'error.message': s.status === 'ERROR' ? s.statusMessage : null,
  }
  const out: Record<string, string | number | boolean> = {}
  for (const [k, v] of Object.entries(all)) {
    if (v === null || v === undefined || v === '') continue
    out[k] =
      typeof v === 'string' || typeof v === 'number' || typeof v === 'boolean'
        ? v
        : JSON.stringify(v)
  }
  return out
}
