/**
 * The `chat` mock group (plan §11): chats, messages, requests and the a2a stream, mirroring
 * nasiko-server at cb3aaf0c. Every quirk below is the server's, marked with the §10 item that
 * would remove it. State lives in memory; `resetChatMock()` drops it between tests.
 */
import { http, HttpResponse, type HttpHandler } from 'msw'
import type { ChatMessage, ChatSessionRow, HitlDto, HitlKind } from '@/features/chat/types'
import type { FlowStep } from '@/features/chat/activity'
import {
  CODING_SESSION_TITLE,
  RATE_LIMIT_BODY,
  TRUNCATION_MARKER,
} from '@/features/chat/serverContract'
import { SHOWCASE_SESSION } from './observability'
import {
  AGENT_1,
  AGENT_2,
  artifact,
  CHAT_SCENARIOS,
  ROUTED_TRACE,
  chainedQuestion,
  routedChainedContinuation,
  routedContinuation,
  routedReplyOf,
  routedUsage,
  SCENARIO_TRACE,
  sseResponse,
  status,
  type ChatScenario,
  type MockFrame,
} from './chat'

interface StoredChat {
  row: ChatSessionRow
  messages: ChatMessage[]
  hitl: HitlDto[]
}

export interface ChatMockContext {
  loggedIn(): boolean
  userId(): string
  now(): number
  /** Agents the viewer can use: id → {name, running}. */
  agents(): { id: string; name: string; running: boolean }[]
  /** `?mock=` chat scenario (full mock mode only), or a test override. */
  scenario(): ChatScenario | null
  /** A trace the observability mock knows, so View trace opens a real waterfall. */
  traceId?(): string | null
  /** The viewer's username: the list prefixes harness agent names with it (routes.rs, §2.2). */
  username?(): string
  /** A coding-harness agent the viewer owns, for the recorded chats (v1c P-5). */
  harness?(): { id: string; name: string } | null
  /** `?mock=` page variants in the comma list (v1c DX1), e.g. `many-chats`. */
  hasVariant?(v: string): boolean
  /** The mock `me` is a superuser (the seed admin by default; `?superuser=0` or `configureMocks({ superuser })`). */
  superuser?(): boolean
}

const withTrace = (ctx: ChatMockContext, frames: MockFrame[]): MockFrame[] => {
  const real = ctx.traceId?.()
  if (!real) return frames
  return frames.map((f) =>
    f.data === undefined
      ? f
      : {
          ...f,
          data: JSON.parse(JSON.stringify(f.data).replaceAll(SCENARIO_TRACE, real)) as unknown,
        },
  )
}

/** A resolved routed request's continuation: it ran (and saved) at resolve time; a reconnect only replays it. */
interface Continuation {
  frames: MockFrame[]
  truncated: boolean
  expired: boolean
}

let chats = new Map<string, StoredChat>()
let seeded = false
let fixedScenario: ChatScenario | null = null
let saveFails = false
let counter = 0
let createDelayMs: number | null = null
/** `GET /api/flows/{trace_id}` steps, per routed turn (flows.rs). */
let flows = new Map<string, FlowStep[]>()
let continuations = new Map<string, Continuation>()
/** Every reconnect a test made, by hitl id. */
let reconnects: string[] = []
/** The opt-in 60-row rail seed (v1c P-5); `?mock=many-chats` in the browser. */
let manyChats = false
/** 60 recorded sessions today (v1c §5.7, DS12); `?mock=many-recorded`. */
let manyRecorded = false
/** The opt-in Waiting seeds (v1c §5.11); `?mock=waiting`. */
let waiting = false
/** Pending rows outside any of the viewer's chats: a `maf` step and another user's request (§2.1). */
let standalonePending: { row: HitlDto; othersOnly: boolean }[] = []
/** `pending-flaky`: polls answered so far (success once, fail the next 3, then work). */
let pendingPolls = 0

export function resetChatMock() {
  chats = new Map()
  seeded = false
  fixedScenario = null
  saveFails = false
  counter = 0
  createDelayMs = null
  flows = new Map()
  continuations = new Map()
  reconnects = []
  manyChats = false
  manyRecorded = false
  waiting = false
  standalonePending = []
  pendingPolls = 0
}

/** Tests: force a stream scenario, make assistant saves fail with a 500, or delay create. */
export function configureChatMock(opts: {
  scenario?: ChatScenario | null
  saveFails?: boolean
  createDelayMs?: number | null
  manyChats?: boolean
  manyRecorded?: boolean
  waiting?: boolean
}) {
  if (opts.waiting !== undefined) waiting = opts.waiting
  if (opts.manyChats !== undefined) manyChats = opts.manyChats
  if (opts.manyRecorded !== undefined) manyRecorded = opts.manyRecorded
  if (opts.scenario !== undefined) fixedScenario = opts.scenario
  if (opts.saveFails !== undefined) saveFails = opts.saveFails
  if (opts.createDelayMs !== undefined) createDelayMs = opts.createDelayMs
}

/** Tests: the messages the "server" holds for a chat, and the reconnects made. */
export const chatMockMessages = (sessionId: string): ChatMessage[] => [
  ...(chats.get(sessionId)?.messages ?? []),
]
export const chatMockReconnects = (): string[] => [...reconnects]
/** A chat's stored requests, live (tests set server-side fields such as `resume_status`). */
export const chatMockRequests = (sessionId: string): HitlDto[] => chats.get(sessionId)?.hitl ?? []

const isRoutedScenario = (sc: ChatScenario | null | undefined): sc is ChatScenario =>
  !!sc && (sc.startsWith('routed-') || sc === 'create-slow')

/** The display form `tool_call.agent` carries (tool.rs:80-86 then react_loop.rs:817-820). */
const displayName = (name: string) => name.replace(/[-_ ./]/g, '-')

/** Swap the scenario placeholders for real agents and a fresh trace id. */
function routedFrames(
  ctx: ChatMockContext,
  frames: readonly MockFrame[],
  traceId: string,
): MockFrame[] {
  const running = ctx.agents().filter((a) => a.running)
  const a1 = displayName(running[0]?.name ?? 'agent')
  const a2 = displayName(running[1]?.name ?? running[0]?.name ?? 'agent')
  return frames.map((f) =>
    f.data === undefined
      ? f
      : {
          ...f,
          data: JSON.parse(
            JSON.stringify(f.data)
              .replaceAll(AGENT_1, a1)
              .replaceAll(AGENT_2, a2)
              .replaceAll(ROUTED_TRACE, traceId),
          ) as unknown,
        },
  )
}

const newTrace = () => `5eedf${String(++counter).padStart(27, '0')}`

type Data = Record<string, unknown>
const dataParts = (f: MockFrame): Data[] => {
  const parts =
    (
      f.data as
        { statusUpdate?: { status?: { message?: { parts?: { data?: Data }[] } } } } | undefined
    )?.statusUpdate?.status?.message?.parts ?? []
  return parts.flatMap((p) => (p.data ? [p.data] : []))
}

/**
 * `flow_steps` for a routed turn (a2a_dispatch.rs:681-715): one row per call with
 * `step_order = turn`; a `tool_result` overwrites every row of its turn (G-1); relayed calls sit
 * at depth 2 under the relaying agent; a pause adds an `awaiting_human` row.
 */
function flowSteps(frames: readonly MockFrame[]): FlowStep[] {
  const steps: FlowStep[] = []
  for (const d of frames.flatMap(dataParts)) {
    const turn = Number(d.turn ?? 0)
    const via = typeof d.via_agent === 'string' ? d.via_agent : null
    if (d.type === 'tool_call')
      steps.push({
        step_order: turn,
        depth: via ? 2 : 1,
        agent_name: String(d.agent),
        caller_agent_name: via ?? 'orchestrator',
        status: 'running',
      })
    if (d.type === 'tool_result')
      for (const st of steps)
        if (st.step_order === turn && st.depth === (via ? 2 : 1))
          st.status = d.success === false ? 'failed' : 'completed'
    // The server marks the agent's newest running call as paused; it adds no row (a2a_dispatch.rs, cb3aaf0c).
    if (d.type === 'awaiting_human') {
      const open = steps
        .filter(
          (st) => st.depth === 1 && st.agent_name === String(d.agent) && st.status === 'running',
        )
        .at(-1)
      if (open) open.status = 'awaiting_human'
    }
  }
  return steps
}

/** Read-only view for other mock groups and tests. */
export const chatMockRows = (): ChatSessionRow[] => [...chats.values()].map((c) => c.row)

const iso = (ms: number) => new Date(ms).toISOString()
const id = (prefix: string) =>
  `5eedc${prefix}-0000-4000-8000-${String(++counter).padStart(12, '0')}`
const text = (body: string, s: number) =>
  new HttpResponse(body, { status: s, headers: { 'Content-Type': 'text/plain' } })
const unauthorized = () =>
  HttpResponse.json(
    { data: null, status_code: 401, message: 'missing or invalid token' },
    { status: 401 },
  )
const rpcError = (s: number, code: number, message: string) =>
  HttpResponse.json({ jsonrpc: '2.0', id: null, error: { code, message } }, { status: s })

function seed(ctx: ChatMockContext) {
  if (seeded) return
  seeded = true
  const now = ctx.now()
  const running = ctx.agents().filter((a) => a.running)
  const a = running[0]
  if (!a) return
  const sid = '5eedc000-0000-4000-8000-00000000c001'
  const t0 = now - 3 * 3_600_000
  chats.set(sid, {
    row: row(ctx, sid, a.id, a.name, 'Summarise last week’s incidents', t0, t0 + 60_000),
    messages: [
      msg(sid, 'user', 'Summarise last week’s incidents', t0),
      {
        ...msg(
          sid,
          'assistant',
          '**Three incidents** last week:\n\n1. Login latency spike (resolved)\n2. Queue backlog on Tuesday\n3. One failed deploy, rolled back\n\n```bash\nnasiko logs weather --since 7d\n```',
          t0 + 60_000,
        ),
        input_tokens: 812,
        output_tokens: 96,
        model: 'gpt-4o-mini',
        duration_ms: 1240,
        cost_usd: '0.00210000',
        usage_estimated: false,
        trace_id: ctx.traceId?.() || SCENARIO_TRACE,
      },
    ],
    hitl: [],
  })
  // A chat whose newest user row never got a reply (the lost-reply path, plan §6.7).
  const lost = '5eedc000-0000-4000-8000-00000000c002'
  const t1 = now - 26 * 3_600_000
  chats.set(lost, {
    row: row(ctx, lost, a.id, a.name, 'Check the nightly export', t1, t1),
    messages: [msg(lost, 'user', 'Check the nightly export', t1)],
    hitl: [],
  })
  // A routed chat (v1 shows it read-only).
  const routed = '5eedc000-0000-4000-8000-00000000c003'
  const t2 = now - 50 * 3_600_000
  chats.set(routed, {
    row: {
      ...row(ctx, routed, null, null, 'Which agent handles invoices?', t2, t2 + 30_000),
      agent_url: '/api/orchestrator/a2a',
    },
    messages: [
      msg(routed, 'user', 'Which agent handles invoices?', t2),
      msg(routed, 'assistant', 'The **finance** agent handles invoices.', t2 + 30_000),
    ],
    hitl: [],
  })
  seedRecorded(ctx, now)
  // v1c M2: the observability mock's showcase session is also one of the viewer's chats, so Sessions and the
  // trace page have a chat to open (a Sessions row id is a chat id, §2.4). Other seeded sessions have none.
  const t3 = now - 6 * 86_400_000
  chats.set(SHOWCASE_SESSION, {
    row: row(ctx, SHOWCASE_SESSION, a.id, a.name, 'Review PR #481', t3, t3 + 90_000),
    messages: [
      msg(SHOWCASE_SESSION, 'user', 'Review PR #481', t3),
      msg(
        SHOWCASE_SESSION,
        'assistant',
        "Couldn't fetch the diff: github.get_diff kept failing and the last attempt timed out.",
        t3 + 90_000,
      ),
    ],
    hitl: [],
  })
  if (manyChats || ctx.hasVariant?.('many-chats')) seedManyChats(ctx, now, running)
  // Last, so c060 (many-chats) exists when its request is added.
  if (waiting || ctx.hasVariant?.('waiting')) seedWaiting(ctx, now, a)
}

/**
 * v1c P-5: a recorded harness chat (c004) and a metadata-only one (c005), older than c001-c003. The list
 * names a harness agent `<username>-<agent>` and marks it `is_coding_agent` (§2.2); stored messages are
 * written only under the `content` policy, so they always say it (§2.3).
 */
function seedRecorded(ctx: ChatMockContext, now: number) {
  const h = ctx.harness?.() ?? null
  const user = ctx.username?.() ?? 'admin'
  const agentName = `${user}-${h?.name ?? 'claude-code'}`
  const recorded = (sid: string, title: string, at: number): ChatSessionRow => ({
    ...row(ctx, sid, h?.id ?? null, agentName, title, at, at + 600_000),
    agent_url: h ? `/api/agents/${h.id}` : 'https://harness.local/claude-code',
    is_coding_agent: true,
  })
  const c4 = '5eedc000-0000-4000-8000-00000000c004'
  const t4 = now - 4 * 86_400_000
  const turn = (i: number, prompt: string, reply: string, calls: ToolCallSeed[]) => [
    msg(c4, 'user', prompt, t4 + i * 120_000),
    {
      ...msg(c4, 'assistant', reply, t4 + i * 120_000 + 60_000),
      model: 'claude-sonnet-4',
      metadata: {
        coding_agent: {
          capture_policy: 'content',
          tool_calls: calls.map((c, n) => toolCall(c, t4 + i * 120_000 + n * 4_000)),
        },
      },
    },
  ]
  chats.set(c4, {
    row: recorded(c4, 'Refactor billing webhook', t4),
    messages: [
      // Six calls, one failed: the summary chip reads "6 tool calls · 1 failed" (§7 test 12).
      ...turn(
        0,
        'Refactor the billing webhook handler and run the tests.',
        'I split the handler into validation, routing and response building. One test fails on an external timeout.',
        [
          {
            name: 'read_file',
            status: 'succeeded',
            args: { path: 'src/billing/webhook.ts' },
            out: { lines: 214 },
            ms: 40,
          },
          {
            name: 'grep',
            status: 'succeeded',
            args: { pattern: 'retryCount' },
            out: { matches: 3 },
            ms: 25,
            association: 'turn',
          },
          {
            name: 'edit_file',
            status: 'succeeded',
            args: { path: 'src/billing/webhook.ts' },
            out: { changed: 3 },
            ms: 120,
          },
          {
            name: 'write_file',
            status: 'succeeded',
            args: { path: 'src/billing/validate.ts' },
            out: '',
            ms: 30,
          },
          {
            name: 'run_tests',
            status: 'failed',
            args: { cmd: 'npm test -- billing' },
            error: 'Timeout: stripe-mock did not answer in 5000 ms',
            ms: 4100,
          },
          {
            name: 'glob',
            status: 'succeeded',
            args: { pattern: 'src/billing/*.ts' },
            out: ['webhook.ts', 'validate.ts'],
            ms: 5,
          },
        ],
      ),
      // Five calls (shown one by one): the other failure statuses and the calls with no result.
      ...turn(1, 'Why did the test time out?', 'Still checking the mock server.', [
        {
          name: 'bash',
          status: 'denied',
          args: { cmd: 'rm -rf node_modules' },
          error: 'The user denied this command.',
        },
        {
          name: 'web_fetch',
          status: 'timed_out',
          args: { url: 'https://status.stripe.com' },
          ms: 30_000,
        },
        { name: 'grep', status: 'cancelled', args: { pattern: 'stripe-mock' } },
        { name: 'bash', status: 'pending', args: { cmd: 'docker ps' }, quality: 'inferred' },
        {
          name: 'read_file',
          status: 'running',
          args: { path: 'docker-compose.yml' },
          quality: 'receipt',
        },
      ]),
      ...turn(
        2,
        'Summarise the diff.',
        'Three functions extracted; behaviour unchanged except clearer 4xx errors.',
        [
          // A value the CLI already cut (CLI integration/report.rs canonical_event, cb3aaf0c): a string where JSON was expected, longer than the UI cap.
          {
            name: 'git_diff',
            status: 'succeeded',
            args: { staged: true },
            out: `{"diff":"${'+ extracted line\\n'.repeat(200)}… [truncated by nasiko CLI: 1048576 bytes]`,
            ms: 60,
          },
          { name: 'telemetry', status: 'unknown', association: 'unknown' },
        ],
      ),
    ],
    hitl: [],
  })
  const c5 = '5eedc000-0000-4000-8000-00000000c005'
  chats.set(c5, {
    row: recorded(c5, CODING_SESSION_TITLE, now - 9 * 86_400_000),
    messages: [],
    hitl: [],
  })
  // 60 recorded sessions in the last hour (c101-c160): newer than every live chat, so the rail's first page is
  // all recorded, and Chats shows "No live chats in the loaded list" until Load more (DS12).
  if (manyRecorded || ctx.hasVariant?.('many-recorded')) {
    for (let n = 101; n <= 160; n++) {
      const sid = `5eedc000-0000-4000-8000-00000000c${n}`
      const at = now - (n - 100) * 60_000 - 600_000
      chats.set(sid, {
        row: recorded(sid, `Harness session ${n - 100}`, at),
        messages: [
          msg(sid, 'user', 'Run the linter', at),
          msg(sid, 'assistant', 'Lint is clean.', at + 60_000),
        ],
        hitl: [],
      })
    }
  }
}

/**
 * v1c §5.11 Waiting seeds, opt-in so c001-c005 and existing tests stay as they are. They cover each way a
 * pending request finds its chat (§5.9): c006 an orchestrator request (`chat_session_id` set); c007 a
 * direct_chat request with `context_id` only, plus an `mcp_tool` request with `chat_session_id: null` that
 * only c007's history `hitl[]` places; c008 two requests on one chat. Outside any chat: a `maf` step, and a
 * request on someone else's chat, returned only to a superuser (router/hitl.rs list_pending, cb3aaf0c). Under many-chats, c060
 * (rail page 2) also waits.
 */
function seedWaiting(ctx: ChatMockContext, now: number, a: { id: string; name: string }) {
  const ask = (message: string) => ({
    message,
    options: [{ label: 'Yes' }, { label: 'No' }],
    allow_custom_input: true,
  })
  const at = (h: number) => now - h * 3_600_000
  const c6 = '5eedc000-0000-4000-8000-00000000c006'
  chats.set(c6, {
    row: { ...row(ctx, c6, null, null, 'Plan the Q3 migration', at(2), at(2)), agent_url: null },
    messages: [msg(c6, 'user', 'Plan the Q3 migration', at(2))],
    hitl: [
      {
        ...hitlRow(
          c6,
          a.id,
          'input_required',
          ask('Migrate the **billing** tables first?'),
          at(2),
          undefined,
          'orchestrator',
        ),
      },
    ],
  })
  const c7 = '5eedc000-0000-4000-8000-00000000c007'
  // Tool approvals name their tool, as the server's do (`question.tool_name`, hitl notifier / dispatcher tests, cb3aaf0c).
  const direct7 = hitlRow(
    c7,
    a.id,
    'tool_approval',
    { ...ask('Allow the agent to run `rm -rf build`?'), tool_name: 'bash' },
    at(5),
  )
  const mcp7: HitlDto = {
    ...hitlRow(
      c7,
      a.id,
      'tool_approval',
      {
        ...ask('Allow the MCP tool <b>github.merge</b>?'),
        tool_name: 'github.merge',
        connector_id: 'github',
      },
      at(4),
    ),
    execution: {
      origin: 'mcp_tool',
      agent_id: a.id,
      task_id: null,
      context_id: null,
      chat_session_id: null,
      maf_execution_id: null,
      maf_step_index: null,
    },
  }
  chats.set(c7, {
    row: row(ctx, c7, a.id, a.name, 'Clean the build folder', at(5), at(4)),
    messages: [msg(c7, 'user', 'Clean the build folder', at(5))],
    hitl: [direct7, mcp7],
  })
  const c8 = '5eedc000-0000-4000-8000-00000000c008'
  const two = (h: number, q: string) => ({
    ...hitlRow(c8, a.id, 'input_required', ask(q), at(h)),
    execution: {
      origin: 'direct_chat',
      agent_id: a.id,
      task_id: 'task-5eed',
      context_id: c8,
      chat_session_id: c8,
      maf_execution_id: null,
      maf_step_index: null,
    },
  })
  chats.set(c8, {
    row: row(ctx, c8, a.id, a.name, 'Rotate the API keys', at(8), at(7)),
    messages: [msg(c8, 'user', 'Rotate the API keys', at(8))],
    hitl: [two(8, 'Rotate the staging key too?'), two(7, 'Notify the on-call channel?')],
  })
  const none = {
    task_id: null,
    context_id: null,
    chat_session_id: null,
    maf_execution_id: null,
    maf_step_index: null,
  }
  standalonePending = [
    {
      row: {
        ...hitlRow(
          'none',
          a.id,
          'input_required',
          ask('Approve the nightly workflow step?'),
          at(10),
        ),
        execution: {
          origin: 'maf',
          agent_id: a.id,
          ...none,
          maf_execution_id: 'maf-1',
          maf_step_index: 2,
        },
      },
      othersOnly: false,
    },
    {
      row: {
        ...hitlRow(
          '5eedc000-0000-4000-8000-00000000c099',
          a.id,
          'input_required',
          ask("Someone else's question"),
          at(1),
          undefined,
          'orchestrator',
        ),
      },
      othersOnly: true,
    },
  ]
  const c60 = chats.get('5eedc000-0000-4000-8000-00000000c060')
  if (c60)
    c60.hitl.push({
      ...hitlRow(
        c60.row.session_id,
        a.id,
        'input_required',
        ask('Ship the old export too?'),
        at(30),
      ),
      execution: {
        origin: 'direct_chat',
        agent_id: a.id,
        task_id: 'task-5eed',
        context_id: c60.row.session_id,
        chat_session_id: c60.row.session_id,
        maf_execution_id: null,
        maf_step_index: null,
      },
    })
}

interface ToolCallSeed {
  name: string
  status: string
  args?: unknown
  out?: unknown
  error?: string
  ms?: number
  association?: 'exact' | 'turn' | 'unknown'
  quality?: 'exact' | 'inferred' | 'receipt' | 'unknown'
}

/** A stored tool call (`types/src/coding_agent.rs` ToolCall, cb3aaf0c): Option fields left out when absent. */
function toolCall(c: ToolCallSeed, at: number): Record<string, unknown> {
  const quality = c.quality ?? 'exact'
  return {
    id: id('t'),
    name: c.name,
    kind: c.name === 'bash' ? 'shell' : 'tool',
    status: c.status,
    ...(c.args !== undefined ? { arguments: c.args } : {}),
    ...(c.out !== undefined ? { output: c.out } : {}),
    ...(c.error !== undefined ? { error: c.error } : {}),
    ...(quality === 'exact'
      ? {
          started_at: iso(at),
          ...(c.ms !== undefined ? { ended_at: iso(at + c.ms), duration_ms: c.ms } : {}),
        }
      : { started_at: iso(at) }),
    association: c.association ?? 'exact',
    timestamp_quality: quality,
  }
}

/** v1c P-5: 55 more rows (c011-c065, 60 in all), spread over every date group, so Load more pages (limit 50). */
function seedManyChats(ctx: ChatMockContext, now: number, running: { id: string; name: string }[]) {
  const titles = [
    'Draft release notes',
    'Check the nightly export',
    'Triage billing issue',
    'Review PR',
    'Summarise support tickets',
    'Plan the migration',
    'Find auth docs',
    'Weekly metrics',
  ]
  for (let i = 0; i < 55; i++) {
    const n = 11 + i
    const sid = `5eedc000-0000-4000-8000-00000000c${String(n).padStart(3, '0')}`
    // From 20 minutes ago to about 6 weeks back, so Today, Yesterday, Previous 7 days and Older all fill.
    const at = now - (20 * 60_000 + i * i * 1_300_000)
    const a = running[i % Math.max(1, running.length)]
    const routedRow = i % 5 === 0 || !a
    const r = routedRow
      ? {
          ...row(ctx, sid, null, null, `${titles[i % titles.length]} ${n}`, at, at),
          agent_url: null,
        }
      : row(ctx, sid, a.id, a.name, `${titles[i % titles.length]} ${n}`, at, at)
    chats.set(sid, {
      row: r,
      messages: [
        msg(sid, 'user', r.title, at),
        msg(sid, 'assistant', `Done: ${r.title.toLowerCase()}.`, at),
      ],
      hitl: [],
    })
  }
}

/** `SessionData` (create): no updated_at, user_id, agent_name or rollups (routes.rs session_response; live 2026-09-27). */
function sessionData(r: ChatSessionRow) {
  return {
    session_id: r.session_id,
    created_at: r.created_at,
    title: r.title,
    agent_id: r.agent_id,
    agent_url: r.agent_url,
  }
}

/** `ChatSession` (PUT): the stored row, without the list view's agent_name, is_coding_agent or rollups (models.rs:15-23). */
function chatSession(r: ChatSessionRow) {
  return {
    session_id: r.session_id,
    user_id: r.user_id,
    agent_id: r.agent_id,
    agent_url: r.agent_url,
    title: r.title,
    created_at: r.created_at,
    updated_at: r.updated_at,
  }
}

function row(
  ctx: ChatMockContext,
  sessionId: string,
  agentId: string | null,
  agentName: string | null,
  title: string,
  created: number,
  updated: number,
): ChatSessionRow {
  return {
    session_id: sessionId,
    user_id: ctx.userId(),
    agent_id: agentId,
    agent_url: agentId ? `/api/agents/${agentId}` : null,
    title,
    created_at: iso(created),
    updated_at: iso(updated),
    agent_name: agentName,
    is_coding_agent: false,
    last_message: null,
    message_count: 0,
    trace_count: 0,
    total_tokens: null,
    latency_p50_ms: null,
  }
}

function msg(
  sessionId: string,
  role: 'user' | 'assistant',
  content: string,
  at: number,
): ChatMessage {
  return {
    id: id('m'),
    session_id: sessionId,
    external_turn_id: null,
    role,
    content,
    file_parts: null,
    has_file_parts: false,
    timestamp: iso(at),
    input_tokens: null,
    output_tokens: null,
    cache_read_tokens: null,
    cache_creation_tokens: null,
    model: null,
    duration_ms: null,
    cost_usd: null,
    usage_estimated: null,
    trace_id: null,
    metadata: null,
  }
}

/** The routed assistant row the server writes at Done (a2a_dispatch.rs:946-971): no agent name, no metadata. */
function persistRouted(
  ctx: ChatMockContext,
  c: StoredChat,
  frames: readonly MockFrame[],
  traceId: string,
) {
  const reply = routedReplyOf(frames)
  if (!reply.trim()) return
  // The same usage the stream's usage_meta carried.
  const u = routedUsage
  c.messages.push({
    ...msg(c.row.session_id, 'assistant', reply, ctx.now()),
    trace_id: traceId,
    duration_ms: u.duration_ms,
    input_tokens: u.input_tokens,
    output_tokens: u.output_tokens,
    model: u.model,
    cost_usd: u.cost_usd.toFixed(8),
    usage_estimated: u.estimated,
  })
  c.row = { ...c.row, updated_at: iso(ctx.now()) }
}

function listRow(c: StoredChat): ChatSessionRow {
  const last = c.messages[c.messages.length - 1]
  return {
    ...c.row,
    last_message: last ? last.content.slice(0, 200) : null,
    message_count: c.messages.length,
    trace_count: c.messages.filter((m) => m.trace_id).length,
    total_tokens:
      c.messages.reduce((n, m) => n + (m.input_tokens ?? 0) + (m.output_tokens ?? 0), 0) || null,
  }
}

function hitlRow(
  sessionId: string,
  agentId: string,
  kind: HitlKind,
  question: HitlDto['question'],
  now: number,
  requestId?: string,
  origin: 'direct_chat' | 'orchestrator' = 'direct_chat',
): HitlDto {
  const actions: Record<HitlKind, string[]> = {
    input_required: ['answer', 'cancel'],
    auth_required: ['start', 'confirm', 'cancel'],
    tool_approval: ['approve', 'reject', 'cancel'],
  }
  return {
    id: requestId ?? id('h'),
    kind,
    status: 'pending',
    resume_status: 'not_started',
    question,
    human_response: null,
    // A routed pause is the orchestrator's: chat_session_id is the chat, task/context the sub-agent's (§2.7).
    execution:
      origin === 'orchestrator'
        ? {
            origin,
            agent_id: agentId,
            task_id: 'sub-task',
            context_id: 'sub-ctx',
            chat_session_id: sessionId,
            maf_execution_id: null,
            maf_step_index: null,
          }
        : // A direct pause carries the chat as chat_session_id too (agent_proxy.rs persist_direct_chat_pause callers, cb3aaf0c).
          {
            origin,
            agent_id: agentId,
            task_id: 'task-5eed',
            context_id: sessionId,
            chat_session_id: sessionId,
            maf_execution_id: null,
            maf_step_index: null,
          },
    // quirk: §10.5 — static per kind, not filtered by status (hitl.rs:256-262).
    allowed_actions: actions[kind],
    expires_at: iso(now + 7 * 86_400_000),
    created_at: iso(now),
    resolved_at: null,
  }
}

/**
 * `resolve_structured_answer` (router/hitl.rs:159-220): trim; single-select takes one offered
 * label (or custom text when allowed); multi-select takes an array of offered labels, deduped,
 * plus an optional custom answer. Returns the stored `human_response`, or the 400 message.
 */
function structuredAnswer(
  h: HitlDto,
  answer: unknown,
  custom: unknown,
): Record<string, unknown> | string {
  const labels = (h.question?.options ?? []).map((o) => o.label)
  const allowCustom = h.question?.allow_custom_input === true || !labels.length
  const customAnswer = typeof custom === 'string' && custom.trim() ? custom.trim() : undefined
  if (h.question?.multi_select !== true) {
    if (Array.isArray(answer)) return 'answer must be a single string for a single-select question'
    const a = typeof answer === 'string' ? answer.trim() : ''
    if (!a) return 'answer is required for input_required'
    if (!labels.includes(a) && !allowCustom)
      return 'answer does not match any offered option, and custom input is not allowed for this question'
    return { answer: a }
  }
  if (
    answer !== undefined &&
    !Array.isArray(answer) &&
    !(typeof answer === 'string' && !answer.trim())
  )
    return 'answer must be an array of selected options for a multi-select question'
  const picked: string[] = []
  for (const raw of Array.isArray(answer) ? answer : []) {
    const label = String(raw).trim()
    if (!labels.includes(label))
      return 'answer contains an option that was not offered by this question'
    if (!picked.includes(label)) picked.push(label)
  }
  if (!picked.length && !customAnswer)
    return 'at least one selected option or a custom answer is required'
  return customAnswer ? { answer: picked, custom_answer: customAnswer } : { answer: picked }
}

const HITL_SCENARIOS: Partial<Record<ChatScenario, true>> = { 'hitl-options': true }

export function chatHandlers(ctx: ChatMockContext): HttpHandler[] {
  const guard = () => (ctx.loggedIn() ? null : unauthorized())

  /** A routed dispatch: the orchestrator's stream, with the assistant row saved server-side at Done (§2.3). */
  const routedDispatch = (sessionId: string) => {
    const picked = fixedScenario ?? ctx.scenario()
    const scenario: ChatScenario = isRoutedScenario(picked) ? picked : 'routed-plain'
    // Pre-stream errors (a2a_dispatch.rs:2204-2242); 429 is plain text with no Retry-After.
    if (scenario === 'routed-400')
      return rpcError(400, -32602, 'invalid params: message has no parts')
    if (scenario === 'routed-429') return text(RATE_LIMIT_BODY, 429)
    if (scenario === 'routed-500') return rpcError(500, -32603, 'internal error')
    if (scenario === 'routed-503' || !ctx.agents().some((a) => a.running))
      return rpcError(503, -32603, 'no agents available')
    const c = chats.get(sessionId)
    const traceId = newTrace()
    let frames = routedFrames(ctx, CHAT_SCENARIOS[scenario], traceId)
    if (scenario.startsWith('routed-hitl') || scenario.startsWith('routed-reconnect')) {
      const running = ctx.agents().filter((a) => a.running)
      const hitlFrame = frames.find((f) => dataParts(f).some((d) => d.type === 'hitl'))
      const part = hitlFrame ? dataParts(hitlFrame).find((d) => d.type === 'hitl') : undefined
      if (c && part) {
        const req = hitlRow(
          sessionId,
          running[0]!.id,
          'input_required',
          (part.question as HitlDto['question']) ?? null,
          ctx.now(),
          id('h'),
          'orchestrator',
        )
        c.hitl.push(req)
        frames = frames.map((f) =>
          f === hitlFrame
            ? { data: status('TASK_STATE_WORKING', [{ data: { ...part, id: req.id } }]) }
            : f,
        )
      }
    }
    flows.set(traceId, flowSteps(frames))
    return sseResponse(
      frames.map((f, i) => (i === 0 ? f : { ...f, delayMs: f.delayMs ?? 15 })),
      {
        failAtEnd: scenario === 'routed-cut',
        // Done only when the stream ran to the end: a client that disconnected early loses it (§2.3).
        onDone: () => {
          if (c) persistRouted(ctx, c, frames, traceId)
        },
      },
    )
  }
  const own = (sid: string) => {
    const c = chats.get(sid)
    return c && c.row.user_id === ctx.userId() ? c : null
  }
  return [
    // GET /api/chat/sessions: own-only, updated_at DESC, keyset (chat/routes.rs:157).
    http.get('/api/chat/sessions', ({ request }) => {
      const denied = guard()
      if (denied) return denied
      seed(ctx)
      const url = new URL(request.url)
      const limit = Math.min(100, Math.max(1, Number(url.searchParams.get('limit')) || 50))
      const agentId = url.searchParams.get('agent_id')
      const cursor = url.searchParams.get('cursor')
      const rows = [...chats.values()]
        .filter((c) => c.row.user_id === ctx.userId() && (!agentId || c.row.agent_id === agentId))
        .map(listRow)
        .sort(
          (a, b) =>
            (b.updated_at ?? '').localeCompare(a.updated_at ?? '') ||
            b.session_id.localeCompare(a.session_id),
        )
      const start = cursor ? Math.max(0, rows.findIndex((r) => r.session_id === cursor) + 1) : 0
      const page = rows.slice(start, start + limit)
      const has_more = start + limit < rows.length
      return HttpResponse.json({
        data: page,
        has_more,
        next_cursor: has_more ? page[page.length - 1]!.session_id : null,
        prev_cursor: null,
      })
    }),

    // POST /api/chat/sessions: a known own session_id returns the existing row (routes.rs:332-345).
    http.post('/api/chat/sessions', async ({ request }) => {
      const denied = guard()
      if (denied) return denied
      seed(ctx)
      const body = (await request.json().catch(() => ({}))) as {
        session_id?: string
        agent_id?: string
        first_prompt?: string
      }
      // The server titles synchronously with an LLM call (routes.rs:365-370); `create-slow` makes it slow.
      const delay =
        createDelayMs ?? ((fixedScenario ?? ctx.scenario()) === 'create-slow' ? 25_000 : 0)
      if (delay) await new Promise((r) => setTimeout(r, delay))
      const sid = body.session_id?.trim()
      if (sid && chats.has(sid)) {
        const existing = own(sid)
        if (!existing) return text('session_id already in use', 409)
        // quirk: §10.12 — status_code says 201 on this 200 too; same SessionData as a fresh create.
        return HttpResponse.json(
          { data: sessionData(existing.row), status_code: 201, message: '' },
          { status: 200 },
        )
      }
      const agent = body.agent_id
        ? ctx.agents().find((a) => a.id === body.agent_id || a.name === body.agent_id)
        : undefined
      if (body.agent_id && !agent) return text('agent not found', 400)
      const now = ctx.now()
      const title = body.first_prompt?.trim()
        ? body.first_prompt.trim().replace(/\s+/g, ' ').slice(0, 60)
        : 'New chat'
      const newId = sid || id('s')
      const created = {
        row: row(ctx, newId, agent?.id ?? null, agent?.name ?? null, title, now, now),
        messages: [],
        hitl: [],
      }
      chats.set(newId, created)
      return HttpResponse.json(
        { data: sessionData(created.row), status_code: 201, message: '' },
        { status: 201 },
      )
    }),

    // PUT: bare row, no trim or cap (routes.rs:490).
    http.put('/api/chat/sessions/:id', async ({ params, request }) => {
      const denied = guard()
      if (denied) return denied
      const c = own(String(params.id))
      if (!c) return new HttpResponse(null, { status: 404 })
      const body = (await request.json().catch(() => ({}))) as { title?: string }
      if (typeof body.title === 'string')
        c.row = { ...c.row, title: body.title, updated_at: iso(ctx.now()) }
      return HttpResponse.json(chatSession(c.row))
    }),

    // DELETE: 204; fails when the chat has requests (hitl_requests FK has no cascade, 0007_hitl.sql:36).
    http.delete('/api/chat/sessions/:id', ({ params }) => {
      const denied = guard()
      if (denied) return denied
      const c = own(String(params.id))
      if (!c) return new HttpResponse(null, { status: 404 })
      if (c.hitl.length) return text('internal error', 500)
      chats.delete(String(params.id))
      return new HttpResponse(null, { status: 204 })
    }),

    // GET messages: newest page ascending; page older with ?prev_cursor= (routes.rs:611).
    http.get('/api/chat/sessions/:id/messages', ({ params, request }) => {
      const denied = guard()
      if (denied) return denied
      seed(ctx)
      const c = own(String(params.id))
      if (!c) return new HttpResponse(null, { status: 404 })
      const url = new URL(request.url)
      // `?mock=probe-500` (v1c M2): the Open chat probe (limit=1) fails; chat history still loads.
      if (url.searchParams.get('limit') === '1' && ctx.hasVariant?.('probe-500'))
        return text('internal error', 500)
      const limit = Math.min(500, Math.max(1, Number(url.searchParams.get('limit')) || 100))
      const before = url.searchParams.get('prev_cursor')
      const all = c.messages
      const end = before
        ? Math.max(
            0,
            all.findIndex((m) => m.id === before),
          )
        : all.length
      const start = Math.max(0, end - limit)
      const page = all.slice(start, end)
      const has_more = start > 0
      return HttpResponse.json({
        data: page,
        has_more,
        next_cursor: has_more ? (page[page.length - 1]?.id ?? null) : null,
        prev_cursor: page[0]?.id ?? null,
        hitl: c.hitl,
      })
    }),

    // POST messages: bare ChatMessage; cost_usd as a Decimal string (models.rs).
    http.post('/api/chat/sessions/:id/messages', async ({ params, request }) => {
      const denied = guard()
      if (denied) return denied
      const c = own(String(params.id))
      if (!c) return new HttpResponse(null, { status: 404 })
      const body = (await request.json().catch(() => ({}))) as {
        role?: string
        content?: string
        usage?: Record<string, unknown>
      }
      if (body.role !== 'user' && body.role !== 'assistant')
        return text('role must be "user" or "assistant"', 400)
      if (body.role === 'assistant' && saveFails) return text('internal error', 500)
      const m = msg(c.row.session_id, body.role, String(body.content ?? ''), ctx.now())
      const u = body.usage ?? {}
      const saved: ChatMessage = {
        ...m,
        input_tokens: typeof u.input_tokens === 'number' ? u.input_tokens : null,
        output_tokens: typeof u.output_tokens === 'number' ? u.output_tokens : null,
        model: typeof u.model === 'string' ? u.model : null,
        duration_ms: typeof u.duration_ms === 'number' ? u.duration_ms : null,
        // quirk: §10.12 — rust_decimal serialises as a string.
        cost_usd: typeof u.cost_usd === 'number' ? u.cost_usd.toFixed(8) : null,
        usage_estimated: typeof u.estimated === 'boolean' ? u.estimated : null,
        trace_id: typeof u.trace_id === 'string' ? u.trace_id : null,
      }
      c.messages.push(saved)
      c.row = { ...c.row, updated_at: saved.timestamp }
      return HttpResponse.json(saved, { status: 201 })
    }),

    // GET /api/flows/{flow_id}: owner-scoped {flow, steps}, steps by step_order (flows.rs:143-175).
    http.get('/api/flows/:id', ({ params }) => {
      const denied = guard()
      if (denied) return denied
      const steps = flows.get(String(params.id))
      if (!steps) return new HttpResponse(null, { status: 404 })
      return HttpResponse.json({
        flow: { flow_id: String(params.id), status: 'completed', root_agent_name: 'orchestrator' },
        steps: [...steps].sort((a, b) => a.step_order - b.step_order),
      })
    }),

    // GET /api/hitl/pending: a normal user's own rows, every user's for a superuser, oldest first, no owner field,
    // no paging (router/hitl.rs list_pending, hitl store.rs list_pending_for, cb3aaf0c). 500 is JSON (§2.1).
    http.get('/api/hitl/pending', () => {
      const denied = guard()
      if (denied) return denied
      seed(ctx)
      if (ctx.hasVariant?.('pending-fail'))
        return HttpResponse.json(
          { error: 'internal error', correlation_id: 'corr-5eed-pending' },
          { status: 500 },
        )
      if (ctx.hasVariant?.('pending-flaky')) {
        const n = ++pendingPolls
        if (n >= 2 && n <= 4)
          return HttpResponse.json(
            { error: 'internal error', correlation_id: `corr-5eed-flaky-${n}` },
            { status: 500 },
          )
      }
      const su = ctx.superuser?.() ?? true
      const own = [...chats.values()].flatMap((c) => c.hitl.filter((h) => h.status === 'pending'))
      const rest = standalonePending
        .filter((x) => x.row.status === 'pending' && (su || !x.othersOnly))
        .map((x) => x.row)
      return HttpResponse.json({
        data: [...own, ...rest].sort((p, q) => p.created_at.localeCompare(q.created_at)),
      })
    }),

    http.get('/api/hitl/:id', ({ params }) => {
      const denied = guard()
      if (denied) return denied
      const h = [...chats.values()].flatMap((c) => c.hitl).find((x) => x.id === String(params.id))
      return h ? HttpResponse.json(h) : text('not found', 404)
    }),

    http.post('/api/hitl/:id/resolve', async ({ params, request }) => {
      const denied = guard()
      if (denied) return denied
      const h = [...chats.values()].flatMap((c) => c.hitl).find((x) => x.id === String(params.id))
      if (!h) return text('not found', 404)
      if (h.status === 'expired' || h.status === 'canceled')
        return text(`this HITL request was ${h.status} before it was answered`, 409)
      if (h.status !== 'pending') return HttpResponse.json({ ...h, already_resolved: true })
      const body = (await request.json().catch(() => ({}))) as {
        answer?: unknown
        custom_answer?: unknown
        decision?: string
        scope?: string
        note?: unknown
        auth_action?: string
      }
      // Per-kind checks and messages as router/hitl.rs:358-383, 440-503.
      if (h.kind === 'tool_approval') {
        if (body.decision !== 'approve' && body.decision !== 'reject')
          return text('decision must be "approve" or "reject" for tool_approval', 400)
        if (body.scope !== undefined && body.scope !== 'once' && body.scope !== 'session')
          return text('scope must be "once" or "session"', 400)
      }
      if (h.kind === 'auth_required') {
        if (body.auth_action !== 'start' && body.auth_action !== 'confirm')
          return text('auth_action must be "start" or "confirm" for auth_required', 400)
        // start only records the click; the server returns the bare row (hitl.rs:472).
        if (body.auth_action === 'start') return HttpResponse.json(h)
      }
      let response: unknown
      if (h.kind === 'input_required') {
        const answered = structuredAnswer(h, body.answer, body.custom_answer)
        if (typeof answered === 'string') return text(answered, 400)
        response = answered
      } else if (h.kind === 'tool_approval') {
        response = {
          decision: body.decision,
          scope: body.decision === 'approve' ? (body.scope ?? 'once') : null,
          note: body.note ?? null,
        }
      } else {
        response = { auth_outcome: 'confirmed' }
      }
      h.status = body.decision === 'reject' ? 'rejected' : 'resolved'
      h.human_response = response
      h.resolved_at = iso(ctx.now())
      // The reconnect-refusal scenarios model an execution with no continuation to run.
      if (
        h.execution.origin === 'orchestrator' &&
        !String(fixedScenario ?? ctx.scenario() ?? '').startsWith('routed-reconnect')
      ) {
        // trigger_new_orchestrator_turn (hitl/mod.rs:1083-1200): a fresh turn with its own trace
        // runs now and saves its reply at Done, whether or not anyone reconnects (EN-10).
        const c = [...chats.values()].find((x) => x.hitl.includes(h))!
        const sc = fixedScenario ?? ctx.scenario()
        const traceId = newTrace()
        const answer = String(
          (response as { answer?: unknown }).answer ??
            (response as { decision?: unknown }).decision ??
            'that',
        )
        // routed-hitl-chained: the first answer makes the sub-agent ask again (a new orchestrator-origin
        // request for the same agent), with no orchestrator turn and so no reply yet.
        const chained =
          sc === 'routed-hitl-chained' &&
          c.hitl.filter((x) => x.execution.origin === 'orchestrator').length === 1
        const next = chained
          ? hitlRow(
              c.row.session_id,
              h.execution.agent_id ?? '',
              'input_required',
              null,
              ctx.now(),
              id('h'),
              'orchestrator',
            )
          : null
        const frames = routedFrames(
          ctx,
          next ? routedChainedContinuation(answer, next.id) : routedContinuation(answer),
          traceId,
        )
        if (next) {
          next.question = chainedQuestion(answer) as HitlDto['question']
          c.hitl.push(next)
        } else {
          flows.set(traceId, flowSteps(frames))
          persistRouted(ctx, c, frames, traceId)
        }
        h.resume_status = 'completed'
        continuations.set(h.id, {
          frames,
          truncated: sc === 'routed-hitl-truncated',
          expired: sc === 'routed-hitl-expired',
        })
      }
      // The server adds already_resolved to every successful answer.
      return HttpResponse.json({ ...h, already_resolved: false })
    }),

    http.post('/api/hitl/:id/cancel', ({ params }) => {
      const denied = guard()
      if (denied) return denied
      const h = [...chats.values()].flatMap((c) => c.hitl).find((x) => x.id === String(params.id))
      if (!h) return text('not found', 404)
      // Already canceled → 200 with already_canceled; any other decided status → 409 (hitl.rs:776-790).
      if (h.status === 'canceled') return HttpResponse.json({ ...h, already_canceled: true })
      if (h.status !== 'pending')
        return text(`this HITL request is already ${h.status}, not pending`, 409)
      h.status = 'canceled'
      h.resolved_at = iso(ctx.now())
      return HttpResponse.json(h)
    }),

    // POST /api/orchestrator/a2a: always SSE; JSON-RPC errors before any frame (a2a_dispatch.rs:122).
    http.post('/api/orchestrator/a2a', async ({ request }) => {
      if (!ctx.loggedIn()) return unauthorized()
      seed(ctx)
      const body = (await request.json().catch(() => ({}))) as {
        params?: {
          message?: { contextId?: string; parts?: { text?: string }[] }
          metadata?: Record<string, unknown>
        }
      }
      const meta = body.params?.metadata ?? {}
      const sessionId = String(meta.session_id ?? body.params?.message?.contextId ?? '')
      const resumeId =
        typeof meta.reconnect_after_hitl_id === 'string' ? meta.reconnect_after_hitl_id : null
      if (resumeId) {
        const c = [...chats.values()].find((x) => x.hitl.some((h) => h.id === resumeId))
        const h = c?.hitl.find((x) => x.id === resumeId)
        // a2a_dispatch.rs:280-314: InvalidRequest (400/-32602) for unknown or unresolved, Forbidden (403/-32605) for another user's.
        if (!c || !h) return rpcError(400, -32602, 'no such HITL request to reconnect to')
        if (c.row.user_id !== ctx.userId())
          return rpcError(403, -32605, 'not authorized to reconnect to this execution')
        if (h.status === 'pending')
          return rpcError(
            400,
            -32602,
            'this HITL request has not been resolved yet — nothing to reconnect to',
          )
        if (h.execution.origin === 'orchestrator') {
          reconnects.push(h.id)
          const sc = fixedScenario ?? ctx.scenario()
          if (sc === 'routed-reconnect-400')
            return rpcError(400, -32602, 'reconnect is not available for this execution')
          if (sc === 'routed-reconnect-403')
            return rpcError(403, -32605, 'not authorized to reconnect to this execution')
          const cont = continuations.get(h.id)
          // After BUFFER_TTL the server's watch() recreates a non-terminal buffer: a 200 stream that
          // stays open with no frames until the stale sweep, about an hour (continuation.rs:215-245, 313).
          if (!cont || cont.expired)
            return new Response(new ReadableStream<Uint8Array>({ start() {} }), {
              status: 200,
              headers: { 'Content-Type': 'text/event-stream' },
            })
          // A full buffer ends with the truncation marker, byte for byte (continuation.rs:46); the row is saved anyway.
          const frames: MockFrame[] = cont.truncated
            ? [...cont.frames.slice(0, 2), { raw: TRUNCATION_MARKER }]
            : cont.frames
          return sseResponse(frames.map((f) => ({ ...f, delayMs: f.delayMs ?? 10 })))
        }
        const answer =
          h.human_response && typeof h.human_response === 'object'
            ? JSON.stringify(
                (h.human_response as { answer?: unknown; decision?: unknown }).answer ??
                  (h.human_response as { decision?: unknown }).decision ??
                  '',
              )
            : '""'
        const reply =
          h.status === 'rejected'
            ? 'Understood, I won’t run that tool.'
            : `Thanks. Continuing with ${answer}.`
        const frames: MockFrame[] = [
          { data: status('TASK_STATE_WORKING') },
          { data: artifact(reply, { lastChunk: true }), delayMs: 20 },
          { data: status('TASK_STATE_COMPLETED') },
        ]
        // §3.3: the server saves the resumed reply (persist_resume_reply), with no usage or trace id.
        return sseResponse(frames, {
          onDone: () => {
            h.resume_status = 'completed'
            c.messages.push(msg(c.row.session_id, 'assistant', reply, ctx.now()))
          },
        })
      }
      const agentId = typeof meta.agent_id === 'string' ? meta.agent_id : null
      const text0 = (body.params?.message?.parts ?? []).map((p) => p.text ?? '').join('')
      if (!text0.trim()) return rpcError(400, -32602, 'message has no text')
      // No agent_id (or "orchestrator"): the ReAct orchestrator (a2a_dispatch.rs:199, 584-592).
      if (!agentId || agentId === 'orchestrator') return routedDispatch(sessionId)
      // resolve_agent filters on status = 'running' (a2a_dispatch.rs:1067-1074): a stopped agent is AgentNotFound too.
      const agent = ctx.agents().find((a) => (a.id === agentId || a.name === agentId) && a.running)
      if (!agent) return rpcError(404, -32604, `agent '${agentId}' not found or not running`)
      const picked = fixedScenario ?? ctx.scenario()
      const scenario = picked && !isRoutedScenario(picked) ? picked : 'direct-steps'
      const c = chats.get(sessionId)
      if (HITL_SCENARIOS[scenario] && c) {
        const hitlFrame = CHAT_SCENARIOS[scenario].find((f) =>
          JSON.stringify(f.data).includes('"hitl"'),
        )
        const part = (
          hitlFrame?.data as {
            statusUpdate?: {
              status?: {
                message?: {
                  parts?: {
                    data?: { id?: string; kind?: HitlKind; question?: HitlDto['question'] }
                  }[]
                }
              }
            }
          }
        )?.statusUpdate?.status?.message?.parts?.[0]?.data
        const req = hitlRow(
          sessionId,
          agent.id,
          part?.kind ?? 'input_required',
          part?.question ?? null,
          ctx.now(),
          id('h'),
        )
        c.hitl.push(req)
        const frames = CHAT_SCENARIOS[scenario].map((f) =>
          f === hitlFrame
            ? { data: status('TASK_STATE_WORKING', [{ data: { ...part, id: req.id } }]) }
            : f,
        )
        return sseResponse(withTrace(ctx, frames))
      }
      return sseResponse(
        withTrace(
          ctx,
          CHAT_SCENARIOS[scenario].map((f, i) =>
            i === 0 ? f : { ...f, delayMs: f.delayMs ?? 15 },
          ),
        ),
      )
    }),
  ]
}
