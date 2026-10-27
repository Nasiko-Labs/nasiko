/**
 * Chat mock building blocks (plan §11, DX-M3). The streams mirror what nasiko-server sends at
 * cb3aaf0c, frame for frame; they never model proposed contracts.
 * - `mockStream(frames, opts)`: an SSE body with per-frame delays, arbitrary byte splits
 *   (inside a UTF-8 code point or between CR and LF) and an optional abrupt error.
 * - `CHAT_SCENARIOS`: named frame scripts, reused by the MSW handlers and the tests.
 */

export interface MockFrame {
  /** JSON payload for a `data:` line. */
  data?: unknown
  /** Raw `data:` text instead of JSON (for garbage frames). */
  raw?: string
  /** SSE event name (`error` for the HITL persist-failure frame). */
  event?: string
  /** Delay before this frame, in ms (fake-timer friendly). */
  delayMs?: number
}

export interface MockStreamOptions {
  /** Enqueue the byte stream in chunks of this size (1 splits everything, code points included). */
  chunkBytes?: number
  /** Line ending between fields: CRLF exercises the split-CR path. */
  lineEnding?: '\n' | '\r\n'
  /** Error the stream instead of closing it after the last frame (a dropped connection). */
  failAtEnd?: boolean
  /** Called when the stream finished normally, so a mock server can persist at "Done". */
  onDone?(): void
}

const enc = new TextEncoder()

export function encodeFrame(f: MockFrame, eol: '\n' | '\r\n' = '\n'): string {
  const lines: string[] = []
  if (f.event) lines.push(`event: ${f.event}`)
  lines.push(`data: ${f.raw ?? JSON.stringify(f.data)}`)
  return lines.join(eol) + eol + eol
}

export function mockStream(
  frames: readonly MockFrame[],
  opts: MockStreamOptions = {},
): ReadableStream<Uint8Array> {
  const eol = opts.lineEnding ?? '\n'
  let i = 0
  let cancelled = false
  return new ReadableStream<Uint8Array>({
    async pull(controller) {
      if (cancelled) return
      if (i >= frames.length) {
        if (opts.failAtEnd) controller.error(new TypeError('network error'))
        else {
          opts.onDone?.()
          controller.close()
        }
        return
      }
      const f = frames[i++]
      if (f.delayMs) await new Promise((r) => setTimeout(r, f.delayMs))
      if (cancelled) return
      const bytes = enc.encode(encodeFrame(f, eol))
      const size = opts.chunkBytes ?? bytes.length
      for (let at = 0; at < bytes.length; at += size) controller.enqueue(bytes.slice(at, at + size))
    },
    cancel() {
      cancelled = true
    },
  })
}

export const sseResponse = (frames: readonly MockFrame[], opts?: MockStreamOptions) =>
  new Response(mockStream(frames, opts), {
    status: 200,
    headers: { 'Content-Type': 'text/event-stream' },
  })

// ── Frame builders (shapes from oss/types/src/a2a.rs and a2a_dispatch.rs) ─────────────────

const TASK = 'task-5eed'
export const status = (state: string, parts: unknown[] = [], ctx = 'ctx') => ({
  statusUpdate: {
    taskId: TASK,
    contextId: ctx,
    status: { state, message: parts.length ? { role: 'ROLE_AGENT', parts } : undefined },
  },
})
export const dataPart = (data: Record<string, unknown>) => ({ data })
export const artifact = (
  text: string,
  opts: { append?: boolean; lastChunk?: boolean; id?: string } = {},
) => ({
  artifactUpdate: {
    taskId: TASK,
    contextId: 'ctx',
    artifact: { artifactId: opts.id ?? 'a1', parts: [{ text }] },
    append: opts.append ?? false,
    lastChunk: opts.lastChunk ?? false,
  },
})
export const traceMeta = (traceId: string) =>
  status('TASK_STATE_WORKING', [dataPart({ type: 'trace_meta', trace_id: traceId })])
export const usageMeta = (usage: Record<string, unknown>) =>
  status('TASK_STATE_WORKING', [dataPart({ type: 'usage_meta', ...usage })])

export type ChatScenario =
  | 'direct-plain'
  | 'direct-steps'
  | 'terminal-usage-terminal'
  | 'a2a03-message'
  | 'a2a03-task'
  | 'a2a10-task'
  | 'lowercase-states'
  | 'append-reset'
  | 'multi-artifact'
  | 'hitl-options'
  | 'stream-garbage'
  | 'failed'
  | 'empty-reply'
  | RoutedScenario

/** Routed-chat scenarios (v1b §5.13): the orchestrator's stream, no `agent_id` on dispatch. */
type RoutedScenario =
  | 'routed-plain'
  | 'routed-multi-agent'
  | 'routed-no-tool'
  | 'routed-empty'
  | 'routed-sub-content-only'
  | 'routed-two-calls-one-turn'
  | 'routed-nested'
  | 'routed-hitl'
  | 'routed-hitl-truncated'
  | 'routed-hitl-no-reconnect'
  | 'routed-hitl-cancelled'
  | 'routed-hitl-expired'
  | 'routed-hitl-repeat'
  | 'routed-hitl-chained'
  | 'routed-agent-failed'
  | 'routed-policy-rejected'
  | 'routed-failed'
  | 'routed-oversized'
  | 'routed-malformed'
  | 'routed-cut'
  | 'routed-400'
  | 'routed-429'
  | 'routed-500'
  | 'routed-503'
  | 'routed-reconnect-400'
  | 'routed-reconnect-403'
  | 'create-slow'
  | 'direct-slow'

/**
 * Placeholders the chat store swaps for the first and second running seed agents' names, so
 * `tool_call.agent` names a real agent (the server sends the display form, react_loop.rs:817-820).
 */
export const AGENT_1 = '@agent-1@'
export const AGENT_2 = '@agent-2@'
/** Placeholder the chat store swaps for a fresh trace id per routed turn (settle is by trace, EN-2). */
export const ROUTED_TRACE = '@routed-trace@'

// Routed data parts (a2a_dispatch.rs:680-770): all ride in WORKING statuses.
const working = (data: Record<string, unknown>) => status('TASK_STATE_WORKING', [dataPart(data)])
export const toolCall = (
  agent: string,
  turn: number,
  message = 'Looking into it',
  extra: Record<string, unknown> = {},
) => working({ type: 'tool_call', agent, message, turn, ...extra })
export const toolResult = (
  agent: string,
  turn: number,
  success: boolean,
  result: string,
  durationMs = 800,
  extra: Record<string, unknown> = {},
) =>
  working({ type: 'tool_result', agent, result, success, turn, duration_ms: durationMs, ...extra })
export const subStatus = (agent: string, message: string) =>
  working({ type: 'sub_status', agent, message })
export const subContent = (agent: string, content: string) =>
  working({ type: 'sub_content', agent, content })
export const routedUsage = {
  duration_ms: 2400,
  trace_id: ROUTED_TRACE,
  input_tokens: 1450,
  output_tokens: 210,
  total_tokens: 1660,
  cost_usd: 0.0041,
  estimated: false,
  model: 'gpt-4o-mini',
}
const routedEnd: MockFrame[] = [
  { data: usageMeta(routedUsage) },
  { data: status('TASK_STATE_COMPLETED') },
]
const routedHitl: MockFrame[] = [
  { data: traceMeta(ROUTED_TRACE) },
  { data: toolCall(AGENT_1, 1, 'Checking the deploy window') },
  {
    data: status('TASK_STATE_INPUT_REQUIRED', [
      dataPart({
        type: 'awaiting_human',
        agent: AGENT_1,
        message: 'Which region should I deploy to?',
      }),
    ]),
  },
  {
    data: working({
      type: 'hitl',
      id: '5eedc000-0000-4000-8000-00000000b001',
      kind: 'input_required',
      task_id: 'sub-task',
      context_id: 'sub-ctx',
      agent: AGENT_1,
      question: {
        message: 'Which region should I deploy to?',
        options: [{ label: 'us-east-1' }, { label: 'eu-west-1' }],
      },
    }),
  },
]
/** Pre-stream failures carry no frames: the store answers them before the stream starts. */
const NO_FRAMES: MockFrame[] = []

/** The trace id baked into the scenarios (chatStore swaps it for a seeded trace). */
export const SCENARIO_TRACE = '5eedc0000000000000000000000000a1'
const usage = {
  duration_ms: 1240,
  trace_id: SCENARIO_TRACE,
  input_tokens: 812,
  output_tokens: 96,
  total_tokens: 908,
  cost_usd: 0.0021,
  estimated: false,
  model: 'gpt-4o-mini',
}

/** A direct turn as the server relays it: working, trace, chunks, agent terminal, usage, terminal. */
export const CHAT_SCENARIOS: Record<ChatScenario, MockFrame[]> = {
  'direct-plain': [
    { data: status('TASK_STATE_WORKING') },
    { data: traceMeta(usage.trace_id) },
    { data: artifact('Hello ', { append: false }) },
    { data: artifact('from the agent.', { append: true, lastChunk: true }) },
    { data: status('TASK_STATE_COMPLETED') },
    { data: usageMeta(usage) },
    { data: status('TASK_STATE_COMPLETED') },
  ],
  'direct-steps': [
    { data: status('TASK_STATE_WORKING') },
    { data: traceMeta(usage.trace_id) },
    {
      data: status('TASK_STATE_WORKING', [
        dataPart({ type: 'tool_call', agent: 'search_docs', message: 'searching', turn: 1 }),
      ]),
    },
    {
      data: status('TASK_STATE_WORKING', [
        dataPart({
          type: 'tool_result',
          agent: 'search_docs',
          result: '3 hits',
          success: true,
          turn: 1,
          duration_ms: 1200,
        }),
      ]),
    },
    { data: artifact('Found three matches.', { lastChunk: true }) },
    { data: status('TASK_STATE_COMPLETED') },
    { data: usageMeta(usage) },
    { data: status('TASK_STATE_COMPLETED') },
  ],
  'terminal-usage-terminal': [
    { data: artifact('Done.', { lastChunk: true }) },
    { data: status('completed') },
    { data: usageMeta(usage) },
    { data: status('TASK_STATE_COMPLETED') },
  ],
  // 0.3 JSON-RPC results the server passes through unchanged.
  'a2a03-message': [
    {
      data: {
        jsonrpc: '2.0',
        id: 1,
        result: {
          kind: 'message',
          role: 'agent',
          parts: [{ kind: 'text', text: 'A 0.3 message reply.' }],
        },
      },
    },
    { data: usageMeta({ duration_ms: 300 }) },
    { data: status('TASK_STATE_COMPLETED') },
  ],
  'a2a03-task': [
    {
      data: {
        jsonrpc: '2.0',
        id: 1,
        result: {
          kind: 'task',
          id: 't',
          status: { state: 'completed' },
          artifacts: [{ artifactId: 'x', parts: [{ kind: 'text', text: 'A 0.3 task reply.' }] }],
        },
      },
    },
    { data: status('TASK_STATE_COMPLETED') },
  ],
  'a2a10-task': [
    {
      data: {
        task: {
          id: 't',
          status: { state: 'TASK_STATE_COMPLETED' },
          artifacts: [{ artifactId: 'x', parts: [{ text: 'A 1.0 task reply.' }] }],
        },
      },
    },
    { data: status('TASK_STATE_COMPLETED') },
  ],
  'lowercase-states': [
    { data: status('working') },
    { data: artifact('lower', { lastChunk: true }) },
    { data: status('completed') },
  ],
  'append-reset': [
    { data: artifact('draft that gets replaced', { append: false }) },
    { data: artifact('', { append: false }) },
    { data: artifact('final', { append: true, lastChunk: true }) },
    { data: status('TASK_STATE_COMPLETED') },
  ],
  'multi-artifact': [
    { data: artifact('Part one. ', { id: 'a1', lastChunk: true }) },
    { data: artifact('Part two.', { id: 'a2', lastChunk: true }) },
    { data: status('TASK_STATE_COMPLETED') },
  ],
  'hitl-options': [
    { data: status('TASK_STATE_WORKING') },
    { data: status('TASK_STATE_INPUT_REQUIRED', [{ text: 'Which region?' }]) },
    {
      data: status('TASK_STATE_WORKING', [
        dataPart({
          type: 'hitl',
          id: '5eedc000-0000-4000-8000-00000000a001',
          kind: 'input_required',
          question: {
            message: 'Which region?',
            options: [{ label: 'us-east-1' }, { label: 'eu-west-1' }],
          },
        }),
      ]),
    },
  ],
  'stream-garbage': [
    { raw: 'not json' },
    { data: artifact('still here', { lastChunk: true }) },
    { data: status('TASK_STATE_COMPLETED') },
  ],
  failed: [
    { data: status('TASK_STATE_WORKING') },
    { data: status('TASK_STATE_FAILED', [{ text: 'upstream timeout' }]) },
  ],
  'empty-reply': [{ data: status('TASK_STATE_WORKING') }, { data: status('TASK_STATE_COMPLETED') }],

  // ── Routed (v1b §2.4): trace, calls as tools, the orchestrator's own artifact text, usage, terminal.
  'routed-plain': [
    { data: traceMeta(ROUTED_TRACE) },
    { data: working({ type: 'thinking', content: 'Choosing who can answer this.' }) },
    { data: toolCall(AGENT_1, 1, 'Summarise the latest incidents') },
    { data: subStatus(AGENT_1, 'Reading incident reports') },
    { data: subContent(AGENT_1, 'Three incidents: login latency, queue backlog, failed deploy.') },
    {
      data: toolResult(
        AGENT_1,
        1,
        true,
        'Three incidents last week: login latency, queue backlog, one failed deploy.',
        1300,
      ),
    },
    { data: artifact('Last week had **three incidents**: ', { append: false }) },
    {
      data: artifact('login latency, a queue backlog and one failed deploy.', {
        append: true,
        lastChunk: true,
      }),
    },
    ...routedEnd,
  ],
  'routed-multi-agent': [
    { data: traceMeta(ROUTED_TRACE) },
    { data: toolCall(AGENT_1, 1, 'Find the failing deploy') },
    { data: toolResult(AGENT_1, 1, true, 'Deploy 42 failed on a missing secret.', 900) },
    { data: toolCall(AGENT_2, 2, 'Draft a fix') },
    { data: subStatus(AGENT_2, 'Drafting the change') },
    { data: toolResult(AGENT_2, 2, true, 'Add the secret to the staging vault.', 1100) },
    {
      data: artifact(
        'Deploy 42 failed on a missing secret. Add it to the staging vault and redeploy.',
        { lastChunk: true },
      ),
    },
    ...routedEnd,
  ],
  'routed-no-tool': [
    { data: traceMeta(ROUTED_TRACE) },
    {
      data: artifact('Hello! Ask me about your agents, deploys or incidents.', { lastChunk: true }),
    },
    ...routedEnd,
  ],
  'routed-empty': [
    { data: traceMeta(ROUTED_TRACE) },
    { data: toolCall(AGENT_1, 1) },
    { data: toolResult(AGENT_1, 1, true, 'ok') },
    ...routedEnd,
  ],
  // Q2: sub-agent text is activity, never the reply.
  'routed-sub-content-only': [
    { data: traceMeta(ROUTED_TRACE) },
    { data: toolCall(AGENT_1, 1) },
    { data: subContent(AGENT_1, 'Sub-agent text that is not the reply.') },
    { data: toolResult(AGENT_1, 1, true, 'done') },
    ...routedEnd,
  ],
  // G-1: two calls to one agent under one turn, the first failing.
  'routed-two-calls-one-turn': [
    { data: traceMeta(ROUTED_TRACE) },
    { data: toolCall(AGENT_1, 1, 'First try') },
    { data: toolResult(AGENT_1, 1, false, 'connection refused', 300) },
    { data: toolCall(AGENT_1, 1, 'Second try') },
    { data: toolResult(AGENT_1, 1, true, 'Found it.', 700) },
    { data: artifact('Found it on the second try.', { lastChunk: true }) },
    ...routedEnd,
  ],
  // EN-7 / G-2: a nested agent's own call is relayed with `via_agent` and isn't a top-level call.
  'routed-nested': [
    { data: traceMeta(ROUTED_TRACE) },
    { data: toolCall(AGENT_1, 1, 'Plan the rollout') },
    { data: toolCall(AGENT_2, 1, 'Nested lookup', { via_agent: AGENT_1 }) },
    { data: toolResult(AGENT_2, 1, true, 'nested ok', 200, { via_agent: AGENT_1 }) },
    { data: toolResult(AGENT_1, 1, true, 'Rollout planned.', 1200) },
    { data: artifact('The rollout is planned in two waves.', { lastChunk: true }) },
    ...routedEnd,
  ],
  'routed-hitl': routedHitl,
  'routed-hitl-truncated': routedHitl,
  'routed-hitl-no-reconnect': routedHitl,
  'routed-hitl-cancelled': routedHitl,
  'routed-hitl-expired': routedHitl,
  'routed-hitl-repeat': routedHitl,
  'routed-hitl-chained': routedHitl,
  'routed-agent-failed': [
    { data: traceMeta(ROUTED_TRACE) },
    { data: toolCall(AGENT_1, 1) },
    { data: toolResult(AGENT_1, 1, false, 'agent unreachable', 60_000) },
    {
      data: artifact(
        'I couldn’t reach the agent that knows this, so here is what I can tell you directly.',
        { lastChunk: true },
      ),
    },
    ...routedEnd,
  ],
  'routed-policy-rejected': [
    { data: traceMeta(ROUTED_TRACE) },
    { data: toolCall(AGENT_1, 1) },
    { data: toolResult(AGENT_1, 1, true, 'part one') },
    {
      data: working({
        type: 'policy_rejected',
        agent: AGENT_2,
        reason: 'max fan-out exceeded: 21/20 invocations',
        turn: 2,
      }),
    },
    {
      data: artifact('I could only finish part of this before hitting a limit.', {
        lastChunk: true,
      }),
    },
    ...routedEnd,
  ],
  'routed-failed': [
    { data: traceMeta(ROUTED_TRACE) },
    { data: toolCall(AGENT_1, 1) },
    { data: status('TASK_STATE_FAILED', [{ text: 'orchestrator error: model unavailable' }]) },
  ],
  // EN-1: 3 × 64 KiB of artifact text; tests lower MAX_TURN_BYTES to trip the drain.
  'routed-oversized': [
    { data: traceMeta(ROUTED_TRACE) },
    { data: artifact('x'.repeat(65_536), { append: false }) },
    { data: artifact('y'.repeat(65_536), { append: true }) },
    { data: artifact('z'.repeat(65_536), { append: true, lastChunk: true }) },
    ...routedEnd,
  ],
  'routed-malformed': [
    { data: traceMeta(ROUTED_TRACE) },
    ...Array.from({ length: 25 }, () => ({ raw: 'not json' })),
    { data: artifact('Still answered.', { lastChunk: true }) },
    ...routedEnd,
  ],
  'routed-cut': [
    { data: traceMeta(ROUTED_TRACE) },
    { data: toolCall(AGENT_1, 1) },
    { data: toolResult(AGENT_1, 1, true, 'ok') },
    { data: artifact('The first half of the answer', { append: false }) },
  ],
  'routed-400': NO_FRAMES,
  'routed-429': NO_FRAMES,
  'routed-500': NO_FRAMES,
  'routed-503': NO_FRAMES,
  'routed-reconnect-400': routedHitl,
  'routed-reconnect-403': routedHitl,
  'create-slow': [
    { data: traceMeta(ROUTED_TRACE) },
    { data: artifact('Answered after a slow start.', { lastChunk: true }) },
    ...routedEnd,
  ],
  // v1c §5.11: a direct reply slow enough to leave the chat mid-stream (background-turn signals).
  'direct-slow': [
    { data: status('TASK_STATE_WORKING') },
    { data: traceMeta(usage.trace_id) },
    { data: artifact('A slow ', { append: false }), delayMs: 400 },
    { data: artifact('reply, ', { append: true }), delayMs: 400 },
    { data: artifact('finally done.', { append: true, lastChunk: true }), delayMs: 400 },
    { data: status('TASK_STATE_COMPLETED') },
    { data: usageMeta(usage) },
    { data: status('TASK_STATE_COMPLETED') },
  ],
}

/**
 * What a resolved routed request's continuation buffer holds (hitl/mod.rs): first the sub-agent's
 * own resumed stream, verbatim (deliver → consume_sse_to_terminal), then the orchestrator's new
 * turn with its own flow and trace (trigger_new_orchestrator_turn). Only the latter is the reply.
 */
export const routedContinuation = (answer: string): MockFrame[] => [
  { data: status('TASK_STATE_WORKING') },
  { data: artifact(`Sub-agent: region ${answer} confirmed.`, { id: 'sub-a1', lastChunk: true }) },
  { data: status('TASK_STATE_COMPLETED') },
  { data: traceMeta(ROUTED_TRACE) },
  { data: toolCall(AGENT_1, 1, `Deploying to ${answer}`) },
  { data: toolResult(AGENT_1, 1, true, `Deployed to ${answer}.`, 900) },
  { data: artifact(`Done: deployed to ${answer}.`, { lastChunk: true }) },
  ...routedEnd,
]

/** The chained pause's second question (stream frame and stored row alike). */
export const chainedQuestion = (answer: string) => ({
  message: `Deploy to ${answer} now or at the next window?`,
  options: [{ label: 'now' }, { label: 'next window' }],
})

/**
 * A resumed sub-agent that asks again (hitl/mod.rs:612-620, cb3aaf0c): its own INPUT_REQUIRED
 * status, then a `hitl` frame built with no agent. No awaiting_human and no orchestrator turn.
 */
export const routedChainedContinuation = (answer: string, requestId: string): MockFrame[] => [
  { data: status('TASK_STATE_WORKING') },
  { data: status('TASK_STATE_INPUT_REQUIRED') },
  {
    data: working({
      type: 'hitl',
      id: requestId,
      kind: 'input_required',
      task_id: 'sub-task',
      context_id: 'sub-ctx',
      question: chainedQuestion(answer),
    }),
  },
]

/** The routed reply text the server would persist at Done: the orchestrator's artifact text only, from its trace_meta on (§2.3). */
export function routedReplyOf(frames: readonly MockFrame[]): string {
  const arts = new Map<string, string>()
  const from = frames.findIndex((f) => JSON.stringify(f.data ?? '').includes('"trace_meta"'))
  for (const f of frames.slice(Math.max(0, from))) {
    const u = (
      f.data as
        | {
            artifactUpdate?: {
              artifact?: { artifactId?: string; parts?: { text?: string }[] }
              append?: boolean
            }
          }
        | undefined
    )?.artifactUpdate
    if (!u) continue
    const id = u.artifact?.artifactId ?? 'a1'
    const t = (u.artifact?.parts ?? []).map((p) => p.text ?? '').join('')
    arts.set(id, u.append ? (arts.get(id) ?? '') + t : t)
  }
  return [...arts.values()].join('')
}
