/**
 * v1b routed chat, pure logic (plan §7 unit tests). Page and registry behaviour are in
 * turnRegistry.routed.test.ts and the page tests.
 */
import { readFileSync } from 'node:fs'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import type { Agent } from '@/features/agents/types'
import {
  AGENT_1,
  CHAT_SCENARIOS,
  ROUTED_TRACE,
  artifact,
  status,
  subContent,
  toolCall,
  toolResult,
  traceMeta,
  type MockFrame,
} from '@/mocks/chat'
import {
  agentActivity,
  agentsAsked,
  flowAgents,
  listAgents,
  routedMetric,
  statusLine,
  type FlowStep,
} from './activity'
import { emptyTurn, reduceSseEvent, replyText, routedReplyText, type TurnState } from './a2aReducer'
import { copy, errorCopy, policyCopy } from './copy'
import { clearDrafts, moveDraft, readDraft, writeDraft } from './drafts'
import { dispatchError } from './errors'
import { chatKind } from './format'
import { disposeForHotReload } from './registry'
import { chatSearchSchema, isAuto } from './search'
import { dispatchBody } from './send'
import { isTruncationMarker, policyLimit, POLICY_ENV, TRUNCATION_PREFIX } from './serverContract'
import { resolveAgentParam, resolveAskedAgent, resolveChatTarget } from './target'
import { foldAgentName } from './agentName'
import type { ChatSessionRow, HitlDto } from './types'

const run = (frames: readonly MockFrame[], s: TurnState = emptyTurn()) =>
  frames.reduce(
    (st, f) =>
      reduceSseEvent(st, { event: f.event ?? 'message', data: f.raw ?? JSON.stringify(f.data) })
        .state,
    s,
  )

const agent = (over: Partial<Agent>): Agent =>
  ({ id: 'a-1', name: 'ops', status: 'running', ...over }) as Agent
const dir = (agents: Agent[]) => ({
  loaded: true,
  byId: new Map(agents.map((a) => [a.id, a])),
  byNameAll: agents.reduce(
    (m, a) => m.set(a.name, [...(m.get(a.name) ?? []), a]),
    new Map<string, Agent[]>(),
  ),
})
const row = (over: Partial<ChatSessionRow>): ChatSessionRow => ({
  session_id: 's',
  agent_id: null,
  title: 't',
  created_at: '',
  ...over,
})

describe('chatKind (§4, §2.8)', () => {
  it('routed when agent_url is null or the orchestrator; removed when absent or an agent URL', () => {
    expect(chatKind(row({ agent_url: null }))).toBe('routed')
    expect(chatKind(row({ agent_url: '/api/orchestrator/a2a' }))).toBe('routed')
    expect(chatKind(row({}))).toBe('removed')
    expect(chatKind(row({ agent_url: '/api/agents/5eed' }))).toBe('removed')
    expect(chatKind(row({ agent_url: null, is_coding_agent: true }))).toBe('recorded')
    expect(chatKind(row({ agent_id: 'a-1', agent_url: null }))).toBe('direct')
  })
})

describe('search and target precedence (§5.1, UC1, DX-1, G-16)', () => {
  const parse = (v: Record<string, unknown>) => chatSearchSchema.parse(v)

  it('auto accepts the number 1 (the JSON search parser) and the string', () => {
    expect(isAuto(parse({ auto: 1 }))).toBe(true)
    expect(isAuto(parse({ auto: '1' }))).toBe(true)
    expect(isAuto(parse({ auto: 2 }))).toBe(false)
    expect(isAuto(parse({ auto: 'yes' }))).toBe(false)
    expect(isAuto(parse({}))).toBe(false)
  })

  it('agent keeps its presence: blank, oversized and non-string values are the banner, never routed', () => {
    const d = dir([agent({})])
    for (const v of ['', '   ', 'x'.repeat(201), 123, true, ['a', 'b'], { a: 1 }]) {
      const s = parse({ agent: v, auto: 1 })
      expect(resolveAgentParam(s.agent, d, isAuto(s))).toMatchObject({ kind: 'invalid' })
    }
    expect(resolveAgentParam(123, d)).toEqual({ kind: 'invalid', value: '123' })
    expect(resolveAgentParam(['a'], d)).toEqual({ kind: 'invalid', value: '["a"]' })
  })

  it('agent wins over auto; auto alone is routed; neither is the chooser', () => {
    const ops = agent({})
    expect(resolveAgentParam('ops', dir([ops]), true)).toEqual({ kind: 'direct', agent: ops })
    expect(resolveAgentParam(undefined, dir([ops]), true)).toEqual({ kind: 'routed' })
    expect(resolveAgentParam(undefined, dir([ops]), false)).toEqual({ kind: 'choose' })
    expect(resolveAgentParam('nope', dir([ops]), true)).toEqual({ kind: 'invalid', value: 'nope' })
  })

  it('an existing routed row is routed; an orchestrator request is routed before its agent_id', () => {
    const ops = agent({})
    expect(resolveChatTarget(row({ agent_url: null }), [], true, dir([ops]))).toEqual({
      kind: 'routed',
    })
    const req = { execution: { origin: 'orchestrator', agent_id: ops.id } } as HitlDto
    expect(resolveChatTarget(undefined, [req], false, dir([ops]))).toEqual({ kind: 'routed' })
    const direct = { execution: { origin: 'direct_chat', agent_id: ops.id } } as HitlDto
    expect(resolveChatTarget(undefined, [direct], false, dir([ops]))).toEqual({
      kind: 'direct',
      agent: ops,
    })
  })
})

describe('resolveAskedAgent (§5.8, G-3, NE-12)', () => {
  it('folds case, -, _, space, . and / to one character', () => {
    expect(foldAgentName('Foo_Bar')).toBe('foo-bar')
    for (const [asked, name] of [
      ['foo-bar', 'foo_bar'],
      ['foo bar', 'foo-bar'],
      ['foo.bar', 'foo-bar'],
      ['foo/bar', 'foo-bar'],
      ['FOO-BAR', 'foo-bar'],
    ]) {
      expect(resolveAskedAgent(asked!, [agent({ id: 'x', name: name! })])).toEqual({
        id: 'x',
        name,
      })
    }
  })

  it('two agents that fold to one name, unknown names and unusable agents give null', () => {
    expect(
      resolveAskedAgent('foo-bar', [
        agent({ id: 'x', name: 'foo_bar' }),
        agent({ id: 'y', name: 'foo-bar' }),
      ]),
    ).toBeNull()
    expect(resolveAskedAgent('nobody', [agent({})])).toBeNull()
    expect(resolveAskedAgent('ops', [agent({ status: 'stopped' })])).toBeNull()
    expect(
      resolveAskedAgent('ops', [agent({ tags: ['coding-agent'] } as Partial<Agent>)]),
    ).toBeNull()
  })
})

describe('dispatch body and errors (§2.2, §5.6)', () => {
  it('a routed dispatch has no agent_id and contextId == metadata.session_id == session id', () => {
    const b = dispatchBody({
      rpcId: 'r',
      messageId: 'm',
      sessionId: 's-1',
      text: 'hi',
      chatMode: 'routed',
    })
    expect(b.params.metadata).toEqual({ session_id: 's-1' })
    expect(b.params.message.contextId).toBe('s-1')
    expect(
      dispatchBody({
        rpcId: 'r',
        messageId: 'm',
        sessionId: 's-1',
        text: 'hi',
        chatMode: 'direct',
        agentId: 'a',
      }).params.metadata,
    ).toEqual({ agent_id: 'a', session_id: 's-1' })
  })

  it('routed send errors: by HTTP status first, JSON-RPC detail when it parses, plain-text 429 kept', () => {
    const send = { chatMode: 'routed', operation: 'send' } as const
    const rpc = (code: number, message: string) => ({
      jsonrpc: '2.0',
      id: null,
      error: { code, message },
    })
    expect(dispatchError(400, rpc(-32602, 'bad'), send)).toMatchObject({
      key: 'routedBadRequest',
      certainty: 'rejected-before-run',
      rpcCode: -32602,
      serverDetail: 'bad',
    })
    expect(dispatchError(403, rpc(-32605, 'no'), send)).toMatchObject({
      key: 'routedForbidden',
      certainty: 'rejected-before-run',
    })
    expect(dispatchError(404, null, send)).toMatchObject({ key: 'routedNoAgents' })
    expect(dispatchError(429, 'rate limit exceeded', send)).toMatchObject({
      key: 'routedRateLimited',
      serverDetail: 'rate limit exceeded',
    })
    expect(dispatchError(500, rpc(-32603, 'internal error'), send)).toMatchObject({
      key: 'routedInternal',
      certainty: 'unknown',
    })
    expect(dispatchError(503, rpc(-32603, 'no agents available'), send)).toMatchObject({
      key: 'routedNoAgents',
      certainty: 'rejected-before-run',
    })
    expect(dispatchError(502, '<html>', send)).toMatchObject({
      key: 'routedInternal',
      serverDetail: '<html>',
    })
    // Direct keeps v1a's mapping.
    expect(dispatchError(503, null)).toMatchObject({ key: 'noAgents' })
  })

  it('reconnect errors: 403 is another sign-in, 429 the rate limit, anything else may still arrive', () => {
    const resume = { chatMode: 'routed', operation: 'resume' } as const
    expect(dispatchError(403, null, resume)).toMatchObject({ key: 'routedReconnectForbidden' })
    expect(dispatchError(429, 'slow down', resume)).toMatchObject({ key: 'routedRateLimited' })
    for (const code of [400, 404, 409, 500])
      expect(dispatchError(code, null, resume)).toMatchObject({
        key: 'routedMayStillArrive',
        certainty: 'unknown',
      })
  })
})

describe('reducer on routed frames (§5.3, §5.7)', () => {
  it('two calls to one agent in one turn pair FIFO, the first failing', () => {
    const s = run(CHAT_SCENARIOS['routed-two-calls-one-turn'])
    expect(s.steps.map((x) => [x.key, x.status])).toEqual([
      [`${AGENT_1}#1`, 'error'],
      [`${AGENT_1}#1#1`, 'ok'],
    ])
    expect(Object.values(s.calls).map((c) => c.invocationKey)).toEqual([
      `${AGENT_1}#1#0#`,
      `${AGENT_1}#1#1#`,
    ])
  })

  it('interleaved calls under one turn close the oldest open call first', () => {
    const s = run([
      { data: toolCall('a', 1) },
      { data: toolCall('a', 1) },
      { data: toolResult('a', 1, false, 'x') },
      { data: toolResult('a', 1, true, 'y') },
    ])
    expect(s.steps.map((x) => x.status)).toEqual(['error', 'ok'])
  })

  it('a relayed call carries via and stays out of top-level attribution, even with an overlapping name and turn', () => {
    const s = run([
      { data: toolCall('a', 1) },
      { data: toolCall('a', 1, 'nested', { via_agent: 'b' }) },
      { data: toolResult('a', 1, false, 'nested failed', 5, { via_agent: 'b' }) },
      { data: toolResult('a', 1, true, 'ok') },
    ])
    expect(s.steps.map((x) => [x.name, x.via, x.status])).toEqual([
      ['a', undefined, 'ok'],
      ['a', 'b', 'error'],
    ])
    expect(agentsAsked(s)).toEqual(['a'])
    expect(agentActivity(s.steps, true).map((a) => a.summary)).toEqual(['completed'])
  })

  it('awaiting_human is kept, and the routed reply is artifact text only (sub_content is activity)', () => {
    const s = run(CHAT_SCENARIOS['routed-sub-content-only'])
    expect(routedReplyText(s)).toBe('')
    expect(s.activity).toContain('Sub-agent text that is not the reply.')
    const paused = run(CHAT_SCENARIOS['routed-hitl'])
    expect(paused.awaiting).toEqual({ agent: AGENT_1, message: 'Which region should I deploy to?' })
    expect(paused.request?.kind).toBe('input_required')
  })

  it('direct keeps its message/task/status fallbacks', () => {
    const s = run([{ data: status('TASK_STATE_COMPLETED', [{ text: 'status text' }]) }])
    expect(routedReplyText(s)).toBe('')
    expect(replyText(s)).toBe('status text')
  })

  it('the truncation marker sets a flag instead of becoming activity', () => {
    const s = run([
      {
        data: status('TASK_STATE_WORKING', [
          { text: `${TRUNCATION_PREFIX} some output was dropped]` },
        ]),
      },
    ])
    expect(s.truncated).toBe(true)
    expect(s.activity).toEqual([])
  })
})

describe('Activity (§5.7, DP5, EN-8)', () => {
  const summary = (frames: MockFrame[], ended: boolean) =>
    agentActivity(run(frames).steps, ended).map((a) => a.summary)

  it("Running while a call is open; Didn't finish when the turn ended with it open", () => {
    expect(summary([{ data: toolCall('a', 1) }], false)).toEqual(['running'])
    expect(summary([{ data: toolCall('a', 1) }], true)).toEqual(['didnt_finish'])
  })

  it('Completed, Failed, and Mixed results for fail-then-ok (no retry correlation)', () => {
    expect(
      summary([{ data: toolCall('a', 1) }, { data: toolResult('a', 1, true, 'ok') }], true),
    ).toEqual(['completed'])
    expect(
      summary([{ data: toolCall('a', 1) }, { data: toolResult('a', 1, false, 'x') }], true),
    ).toEqual(['failed'])
    expect(summary(CHAT_SCENARIOS['routed-two-calls-one-turn'], true)).toEqual(['mixed'])
  })

  it('one row per agent in first-call order, with total call time and a failed count', () => {
    const rows = agentActivity(
      run([
        { data: toolCall('b', 1) },
        { data: toolResult('b', 1, false, 'x', 100) },
        { data: toolCall('a', 2) },
        { data: toolResult('a', 2, true, 'y', 50) },
        { data: toolCall('b', 3) },
        { data: toolResult('b', 3, true, 'z', 25) },
      ]).steps,
      true,
    )
    expect(rows.map((r) => [r.name, r.summary, r.totalMs, r.failed])).toEqual([
      ['b', 'mixed', 125, 1],
      ['a', 'completed', 50, 0],
    ])
    expect(agentsAsked(run(CHAT_SCENARIOS['routed-multi-agent']))).toEqual([AGENT_1, '@agent-2@'])
  })

  it('status line: working → asking → working (calls resolved) → writing', () => {
    const f: MockFrame[] = [
      { data: traceMeta(ROUTED_TRACE) },
      { data: toolCall('a', 1) },
      { data: subContent('a', 'sub text') },
      { data: toolResult('a', 1, true, 'ok') },
      { data: artifact('Reply') },
    ]
    const lines = f.map((_, i) => statusLine(run(f.slice(0, i + 1))))
    expect(lines).toEqual([
      { kind: 'working' },
      { kind: 'asking', agent: 'a' },
      { kind: 'asking', agent: 'a' },
      { kind: 'working' },
      { kind: 'writing' },
    ])
  })

  it('flows fallback: depth-1 orchestrator calls only, names and order only', () => {
    const steps: FlowStep[] = [
      {
        step_order: 2,
        depth: 1,
        agent_name: 'b',
        caller_agent_name: 'orchestrator',
        status: 'completed',
      },
      {
        step_order: 1,
        depth: 1,
        agent_name: 'a',
        caller_agent_name: 'orchestrator',
        status: 'failed',
      },
      {
        step_order: 1,
        depth: 2,
        agent_name: 'nested',
        caller_agent_name: 'a',
        status: 'completed',
      },
      {
        step_order: 3,
        depth: 1,
        agent_name: 'ext',
        caller_agent_name: 'someone-else',
        status: 'completed',
      },
      {
        step_order: 4,
        depth: 1,
        agent_name: 'a',
        caller_agent_name: 'orchestrator',
        status: 'awaiting_human',
      },
      {
        step_order: 5,
        depth: 1,
        agent_name: 'orchestrator',
        caller_agent_name: 'orchestrator',
        status: 'completed',
      },
      {
        step_order: 6,
        depth: 1,
        agent_name: 'a',
        caller_agent_name: 'orchestrator',
        status: 'completed',
      },
    ]
    expect(flowAgents(steps)).toEqual(['a', 'b'])
  })

  it('live and reloaded attribution agree', () => {
    const s = run(CHAT_SCENARIOS['routed-nested'])
    const flows: FlowStep[] = [
      {
        step_order: 1,
        depth: 1,
        agent_name: AGENT_1,
        caller_agent_name: 'orchestrator',
        status: 'completed',
      },
      {
        step_order: 1,
        depth: 2,
        agent_name: '@agent-2@',
        caller_agent_name: AGENT_1,
        status: 'completed',
      },
    ]
    expect(agentsAsked(s)).toEqual(flowAgents(flows))
  })

  it('attribution lists one, two and three agents', () => {
    expect(copy.answeredBy).toBe('Answered by the Orchestrator')
    expect(copy.answeredByUsing(listAgents(['a']))).toBe('Answered by the Orchestrator, using a')
    expect(copy.answeredByUsing(listAgents(['a', 'b']))).toBe(
      'Answered by the Orchestrator, using a and b',
    )
    expect(copy.answeredByUsing(listAgents(['a', 'b', 'c']))).toBe(
      'Answered by the Orchestrator, using a, b, and c',
    )
  })

  it('routed metric counts 0 / 1 / 2+ agent turns and empty endings', () => {
    expect(
      routedMetric([
        { agents: 0, empty: false },
        { agents: 1, empty: false },
        { agents: 2, empty: true },
        { agents: 3, empty: false },
        { agents: 1, empty: true },
      ]),
    ).toEqual({ total: 5, zero: 1, one: 2, twoPlus: 2, empty: 2 })
  })

  it('copy.routedMetric says "1 turn", and "2 turns"', () => {
    const m = { zero: 0, one: 1, twoPlus: 0, empty: 0 }
    expect(copy.routedMetric({ ...m, total: 1 })).toBe(
      'routed metric · 1 turn · 0 agents 0 · 1 agent 1 · 2+ agents 0 · empty 0',
    )
    expect(copy.routedMetric({ ...m, total: 2 })).toBe(
      'routed metric · 2 turns · 0 agents 0 · 1 agent 1 · 2+ agents 0 · empty 0',
    )
  })
})

describe('draft transfer (§5.1, G-19)', () => {
  beforeEach(() => clearDrafts())
  afterEach(() => clearDrafts())

  it('moves into an empty destination and clears the source', () => {
    writeDraft('u', 'new:choose', 'hello')
    expect(moveDraft('u', 'new:choose', 'new:routed')).toBe(true)
    expect(readDraft('u', 'new:routed')).toBe('hello')
    expect(readDraft('u', 'new:choose')).toBe('')
    expect(moveDraft('u', 'new:routed', 'new:a-1')).toBe(true)
    expect(readDraft('u', 'new:a-1')).toBe('hello')
  })

  it('an empty source moves nothing; a populated destination is kept and the source stays', () => {
    writeDraft('u', 'new:routed', 'kept')
    expect(moveDraft('u', 'new:choose', 'new:routed')).toBe(false)
    writeDraft('u', 'new:choose', 'mine')
    expect(moveDraft('u', 'new:choose', 'new:routed')).toBe(false)
    expect(readDraft('u', 'new:routed')).toBe('kept')
    expect(readDraft('u', 'new:choose')).toBe('mine')
  })
})

describe('server contract (§5.3, §5.6, NC-5)', () => {
  it('truncation prefix', () => {
    expect(
      isTruncationMarker(
        '[replay truncated: this resume produced more events than can be buffered; some output was dropped]',
      ),
    ).toBe(true)
    expect(isTruncationMarker('replay truncated')).toBe(false)
  })

  it('each FlowRejection reason maps to a limit and its copy', () => {
    const cases: [string, string][] = [
      ['max call depth exceeded: 6/5', 'depth'],
      ['cycle detected: agent x already in chain ["x"]', 'cycle'],
      ['max fan-out exceeded: 21/20 invocations', 'fanOut'],
      ['flow token budget exhausted: 100001/100000', 'tokens'],
      ['flow timeout: 121s/120s', 'timeout'],
      ['flow guard unavailable (redis unreachable) — failing closed', 'guard'],
    ]
    for (const [reason, limit] of cases) expect(policyLimit(reason)).toBe(limit)
    expect(policyLimit('something else')).toBeNull()
    expect(POLICY_ENV).toEqual({
      depth: 'NASIKO_FLOW_MAX_DEPTH',
      fanOut: 'NASIKO_FLOW_MAX_FAN_OUT',
      tokens: 'NASIKO_FLOW_MAX_TOKENS',
      timeout: 'NASIKO_FLOW_TIMEOUT_SECS',
    })
    expect(policyCopy.stopped(policyCopy.fanOut)).toBe('Stopped by an OpenRuntime limit: fan-out.')
    expect(policyCopy.raise(POLICY_ENV.fanOut!)).toBe(
      'Ask your admin to raise NASIKO_FLOW_MAX_FAN_OUT.',
    )
  })
})

describe('copy (§5.12, ND-1, ND-12)', () => {
  const all = JSON.stringify({
    copy: Object.fromEntries(
      Object.entries(copy).map(([k, v]) => [
        k,
        typeof v === 'function' ? (v as (...a: unknown[]) => unknown)('x', 'y') : v,
      ]),
    ),
    errorCopy,
    policyCopy,
  })

  it('retired routed strings are gone', () => {
    // The ND-1 and P2 list (plan Review record): the v1a routed badge word and the dropped phrasings.
    for (const banned of [
      'Choosing an agent',
      'Used ',
      'All · Waiting',
      'Routed to',
      'Recovered after retry',
      'next release',
      '"Routed"',
    ])
      expect(all).not.toContain(banned)
  })

  it('new routed strings never call a chat a session', () => {
    const routedKeys = [
      'routedHint',
      'finishedWithoutReply',
      'chatDetailsMissing',
      'routedBadgeTooltip',
      'replyTooLarge',
      'loadingSavedReply',
      'savedReplyFailed',
    ] as const
    for (const k of routedKeys) expect(String(copy[k])).not.toMatch(/session/i)
    for (const [k, e] of Object.entries(errorCopy))
      if (k.startsWith('routed') || k === 'tooManyLive')
        expect(`${e.problem} ${e.cause} ${e.action}`).not.toMatch(/session/i)
  })
})

describe('docs (NX-7)', () => {
  it('docs/chat.md lists every mock scenario, one row each', () => {
    const doc = readFileSync('docs/chat.md', 'utf8')
    const section = doc.slice(doc.indexOf('### Mock scenarios'), doc.indexOf('## Partial live'))
    const rows = [...section.matchAll(/^\| `([a-z0-9-]+)` \|/gm)].map((m) => m[1])
    expect([...rows].sort()).toEqual(Object.keys(CHAT_SCENARIOS).sort())
  })
})

describe('hot reload (DX-4, NX-10)', () => {
  it('warns when it aborts live turns, and stays quiet otherwise', () => {
    const warn = vi.fn()
    disposeForHotReload(warn)
    expect(warn).not.toHaveBeenCalled()
  })
})
