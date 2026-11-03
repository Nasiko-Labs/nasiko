/**
 * The harness session ledger's rules (plans/feat-agent-centric.md PR1b; X4; eng "Ledger"; design "Ledger"). Pure.
 */
import { describe, expect, it } from 'vitest'
import type { ChatMessage, MessagesPage } from '@nasiko/features/chat/types'
import {
  budgetLines,
  buildLedger,
  controlsOf,
  groupTurns,
  ledgerCsv,
  mergeMessages,
  runningShare,
  tightest,
} from './ledger'
import { copy } from './copy'
import type { SessionBudget } from './types'

let seq = 0
const at = (min: number) => new Date(Date.UTC(2026, 9, 1, 12, min)).toISOString()
const msg = (
  role: 'user' | 'assistant',
  min: number,
  over: Partial<ChatMessage> = {},
): ChatMessage => ({
  id: `m${String(++seq).padStart(4, '0')}`,
  session_id: 's',
  role,
  content: `${role} ${min}`,
  timestamp: at(min),
  ...over,
})
const call = (name: string, over: Record<string, unknown> = {}) => ({
  id: `t-${name}-${++seq}`,
  name,
  status: 'succeeded',
  arguments: { secret: 'ARG-TEXT' },
  output: 'OUTPUT-TEXT',
  association: 'exact',
  ...over,
})
const reply = (min: number, extra: Record<string, unknown> = {}, over: Partial<ChatMessage> = {}) =>
  msg('assistant', min, {
    input_tokens: 1000,
    output_tokens: 100,
    cost_usd: '0.0045',
    metadata: { coding_agent: { capture_policy: 'content', ...extra } },
    ...over,
  })
const page = (data: ChatMessage[], has_more = false): MessagesPage => ({
  data,
  has_more,
  next_cursor: null,
  prev_cursor: data[0]?.id ?? null,
  hitl: [],
})

describe('turns', () => {
  it('a user message starts a turn and the replies after it join it', () => {
    const turns = groupTurns([msg('user', 0), reply(1), reply(2), msg('user', 3), reply(4)])
    expect(turns.map((t) => t.map((m) => m.role))).toEqual([
      ['user', 'assistant', 'assistant'],
      ['user', 'assistant'],
    ])
  })

  it('a turn id groups messages even when they interleave', () => {
    const turns = groupTurns([
      msg('user', 0, { external_turn_id: 'a' }),
      msg('user', 1, { external_turn_id: 'b' }),
      reply(2, {}, { external_turn_id: 'a' }),
      reply(3, {}, { external_turn_id: 'b' }),
    ])
    expect(turns.map((t) => t.map((m) => m.external_turn_id))).toEqual([
      ['a', 'a'],
      ['b', 'b'],
    ])
  })

  it('pages merge newest first, deduped by id, oldest first (a refetch can repeat rows)', () => {
    const older = [msg('user', 0), reply(1)]
    const newer = [msg('user', 2), reply(3)]
    const merged = mergeMessages([page(newer), page([...older, newer[0]!])])
    expect(merged.map((m) => m.timestamp)).toEqual([at(0), at(1), at(2), at(3)])
  })

  it('keeps the server’s order: never re-sorted by millisecond (review: Codex, adversarial)', () => {
    // Microseconds apart on the server (the browser parses both to one millisecond), and the reply's id sorts first.
    const user = msg('user', 0, { id: 'z-user', timestamp: '2026-10-01T12:00:00.000001Z' })
    const answer = reply(0, {}, { id: 'a-reply', timestamp: '2026-10-01T12:00:00.000002Z' })
    const l = buildLedger([page([user, answer])], { complete: true, preview: false })
    expect(l.turns).toHaveLength(1)
    expect(l.turns[0]!.prompt).toBe(user.content)
    expect(l.turns[0]!.tokensIn).toBe(1000)
  })

  it('a prompt saved without a turn id takes its reply’s, and later messages with it join', () => {
    const turns = groupTurns([
      msg('user', 0),
      reply(1, {}, { external_turn_id: 't1' }),
      reply(2, {}, { external_turn_id: 't1' }),
      msg('user', 3),
    ])
    expect(turns.map((t) => t.length)).toEqual([3, 1])
  })

  it('a prompt with an id after a turn without one starts its own turn (review: verify)', () => {
    const turns = groupTurns([
      msg('user', 0),
      reply(1),
      msg('user', 2, { external_turn_id: 't2' }),
      reply(3, {}, { external_turn_id: 't2' }),
    ])
    expect(turns.map((t) => t.length)).toEqual([2, 2])
  })

  it('a turn with one unpriced reply is not counted as reported in the totals', () => {
    const l = buildLedger([page([msg('user', 0), reply(1), reply(2, {}, { cost_usd: null })])], {
      complete: true,
      preview: false,
    })
    expect(l.totals.cost).toMatchObject({ reported: 0 })
    expect(l.turns[0]!.runningFloor).toBe(true)
  })

  it('system messages are skipped; a reply before any prompt starts its own turn', () => {
    const turns = groupTurns([
      reply(0, {}, { external_turn_id: 'x' }),
      msg('user', 1),
      { ...msg('user', 2), role: 'system' },
      reply(3),
    ])
    expect(turns.map((t) => t.map((m) => m.role))).toEqual([['assistant'], ['user', 'assistant']])
  })

  it('an unloaded older page cuts the first turn only when it starts on a reply', () => {
    const l = buildLedger([page([msg('user', 0), reply(1)], true)], {
      complete: false,
      preview: false,
    })
    expect(l.turns[0]!.partial).toBe(false)
  })

  it('more than 100 messages: the loaded subtotal says so, and the cut turn is marked', () => {
    const all: ChatMessage[] = []
    for (let i = 0; i < 75; i++) all.push(msg('user', i * 2), reply(i * 2 + 1))
    // The newest page starts on a reply: its user message is on the older page.
    const loaded = buildLedger([page(all.slice(-99), true)], { complete: false, preview: false })
    expect(loaded.complete).toBe(false)
    expect(loaded.turns[0]!.partial).toBe(true)
    const whole = buildLedger([page(all.slice(-100), true), page(all.slice(0, -100))], {
      complete: true,
      preview: false,
    })
    expect(whole.totals.turns).toBe(75)
    expect(whole.turns.some((t) => t.partial)).toBe(false)
  })
})

describe('what a turn used', () => {
  it('sums the replies’ tokens and cost, with a running total', () => {
    const l = buildLedger([page([msg('user', 0), reply(1), reply(2), msg('user', 3), reply(4)])], {
      complete: true,
      preview: false,
    })
    expect(l.turns.map((t) => [t.tokensIn, t.tokensOut, t.cost])).toEqual([
      [2000, 200, 0.009],
      [1000, 100, 0.0045],
    ])
    expect(l.turns[1]!.runningCost).toBeCloseTo(0.0135)
    expect(l.turns[1]!.runningFloor).toBe(false)
    expect(l.totals.cost.value).toBeCloseTo(0.0135)
    expect(l.totals.cost.reported).toBe(2)
  })

  it('tokens in count cache reads and writes (the server reports them apart; review: Codex)', () => {
    const l = buildLedger(
      [
        page([
          msg('user', 0),
          reply(
            1,
            {},
            { input_tokens: 10, cache_read_tokens: 90_000, cache_creation_tokens: 10_000 },
          ),
        ]),
      ],
      { complete: true, preview: false },
    )
    expect(l.turns[0]).toMatchObject({ tokensIn: 100_010, cacheRead: 90_000, cacheWrite: 10_000 })
  })

  it('an unpriced turn makes the totals a floor over the turns that reported (review: adversarial)', () => {
    const l = buildLedger(
      [
        page([
          msg('user', 0),
          reply(1),
          msg('user', 2),
          reply(3, {}, { cost_usd: null }),
          msg('user', 4),
          reply(5),
        ]),
      ],
      { complete: true, preview: false },
    )
    expect(l.totals.cost).toMatchObject({ reported: 2 })
    expect(l.totals.turns).toBe(3)
    expect(l.turns.map((t) => t.runningFloor)).toEqual([false, true, true])
  })

  it('tool badges are keyed by message: a harness repeats call ids across messages', () => {
    const same = { id: 'call-1', name: 'grep', status: 'succeeded' }
    const l = buildLedger(
      [page([msg('user', 0), reply(1, { tool_calls: [same] }), reply(2, { tool_calls: [same] })])],
      { complete: true, preview: false },
    )
    const keys = l.turns[0]!.tools.map((t) => t.key)
    expect(new Set(keys).size).toBe(2)
  })

  it('a value no reply reported is null ("not reported"), never 0', () => {
    const l = buildLedger(
      [
        page([
          msg('user', 0),
          reply(1, {}, { input_tokens: null, output_tokens: null, cost_usd: null }),
        ]),
      ],
      { complete: true, preview: false },
    )
    expect([l.turns[0]!.tokensIn, l.turns[0]!.cost, l.totals.cost.value]).toEqual([
      null,
      null,
      null,
    ])
  })

  it('recorded tool calls land on their turn; an unknown association is "Turn not recorded"', () => {
    const l = buildLedger(
      [
        page([
          msg('user', 0),
          reply(1, {
            tool_calls: [
              call('read_file'),
              call('bash', { status: 'failed' }),
              call('x', { association: 'unknown' }),
            ],
          }),
        ]),
      ],
      { complete: true, preview: false },
    )
    expect(l.turns[0]!.tools.map((s) => s.name)).toEqual(['read_file', 'bash'])
    expect(l.unplaced.map((s) => s.name)).toEqual(['x'])
    expect(l.totals).toMatchObject({ toolCalls: 3, failedCalls: 1 })
  })
})

describe('preview records (AH-7)', () => {
  const proposed = {
    context: {
      'nasiko.context.strategy': 'pacms',
      'nasiko.context.pool': 40,
      'nasiko.context.pool_tokens': 20_000,
      'nasiko.context.kept': 10,
      'nasiko.context.kept_tokens_est': 9_000,
    },
    memory: { tokens: 1_500, items: 3 },
    controls: [
      {
        at: at(1),
        kind: 'tool_rule',
        rule: { id: 'r1', name: 'Block rm' },
        outcome: 'blocked',
        tool: 'bash',
      },
    ],
  }

  it('are read only in preview mode: an unexpected live field never draws', () => {
    const pages = [page([msg('user', 0), reply(1, proposed)])]
    const live = buildLedger(pages, { complete: true, preview: false })
    expect([live.turns[0]!.context, live.turns[0]!.memory, live.turns[0]!.controls]).toEqual([
      null,
      null,
      [],
    ])
    expect(live.hasPreview).toBe(false)
    const mock = buildLedger(pages, { complete: true, preview: true })
    expect(mock.turns[0]!.context?.kept.tokens).toBe(9_000)
    expect(mock.turns[0]!.memory).toEqual({ tokens: 1_500, items: 3 })
    expect(mock.turns[0]!.controls).toHaveLength(1)
  })

  it('no control is shown without a recorded decision: time, rule identity and outcome', () => {
    const m = (controls: unknown[]) => reply(1, { controls })
    const good = { at: at(1), kind: 'budget', rule: { id: 'b', name: 'Budget' }, outcome: 'warned' }
    expect(controlsOf(m([good]))).toHaveLength(1)
    for (const bad of [
      { ...good, at: 'yesterday' },
      { ...good, at: undefined },
      { ...good, rule: { id: '', name: 'Budget' } },
      { ...good, rule: { id: 'b' } },
      { ...good, rule: undefined },
      { ...good, outcome: undefined },
      { ...good, outcome: 'maybe' },
      { ...good, kind: 'setting' },
    ])
      expect(controlsOf(m([bad]))).toEqual([])
  })
})

describe('CSV (X4)', () => {
  const labels = () => ({
    ...copy.ledger.csv,
    cost: 'Est. cost (API list price)',
    notReported: 'not reported',
  })
  const l = buildLedger(
    [
      page([
        msg('user', 0, { content: 'Fix the bug' }),
        reply(
          1,
          {
            tool_calls: [call('=HYPERLINK("http://x")'), call('grep', { status: 'denied' })],
            context: {
              'nasiko.context.pool': 4,
              'nasiko.context.pool_tokens': 400,
              'nasiko.context.kept': 2,
              'nasiko.context.kept_tokens_est': 200,
            },
            controls: [
              {
                at: at(1),
                kind: 'tool_rule',
                rule: { id: 'r', name: '=cmd()' },
                outcome: 'blocked',
              },
            ],
          },
          { content: 'REPLY-TEXT' },
        ),
        msg('user', 2),
        reply(3, { tool_calls: [call('lost', { association: 'unknown' })] }, { cost_usd: null }),
      ]),
    ],
    { complete: true, preview: true },
  )
  const csv = ledgerCsv(l, labels())
  const lines = csv.split('\r\n')

  it('has the agreed columns, with the harness cost basis, cache tokens and coverage', () => {
    expect(lines[0]).toBe(
      '"turn","time (UTC)","tool names","tokens in","of which cache reads","of which cache writes","tokens out","Est. cost (API list price)","context tokens","memory tokens","controls applied","source","coverage"',
    )
  })

  it('never exports tool arguments, results, prompts or replies', () => {
    for (const text of ['ARG-TEXT', 'OUTPUT-TEXT', 'Fix the bug', 'REPLY-TEXT'])
      expect(csv).not.toContain(text)
  })

  it('escapes a tool or a rule named like a formula, and names recorded statuses', () => {
    expect(lines[1]).toContain(`"'=HYPERLINK(""http://x"")`)
    expect(lines[1]).toContain('grep (denied)')
    expect(lines[1]).toContain(`"'=cmd(): blocked"`)
  })

  it('writes "not reported" for what the server didn\'t report, and marks preview rows', () => {
    expect(lines[1]).toContain('"200"')
    expect(lines[1]).toContain(`"${copy.ledger.csv.source.preview}"`)
    expect(lines[2]).toContain('"not reported"')
    expect(lines[2]).toContain(`"${copy.ledger.csv.source.recorded}"`)
    // The unplaced call: its own row, turn "not recorded".
    expect(lines[3]).toMatch(/^"not recorded","","lost"/)
  })

  it('every row says whether the file is the whole session or the loaded turns (review: Codex)', () => {
    expect(lines[1]).toMatch(/"whole session"$/)
    const part = buildLedger([page([reply(1), msg('user', 2), reply(3)], true)], {
      complete: false,
      preview: false,
    })
    expect(ledgerCsv(part, labels()).split('\r\n')[1]).toMatch(/"latest 2 turns loaded"$/)
  })

  it('live rows (no per-turn records) carry "not reported"', () => {
    const live = buildLedger([page([msg('user', 0), reply(1)])], { complete: true, preview: false })
    const row = ledgerCsv(live, { ...labels(), cost: 'Cost' }).split('\r\n')[1]!
    // cache reads, cache writes, context, memory, controls
    expect(row.match(/"not reported"/g)).toHaveLength(5)
  })
})

describe('budgets (AH-8, preview)', () => {
  const b = (over: Partial<SessionBudget>): SessionBudget => ({
    budget_id: over.scope ?? 'owner',
    scope: 'owner',
    limit_usd: 100,
    period: '2026-10',
    used_before_usd: 41,
    session_used_usd: 3,
    turns: [
      { turn_id: 't1', used_usd: 1 },
      { turn_id: 't2', used_usd: 2 },
    ],
    ...over,
  })

  it('orders session → agent → owner and picks the highest share used as the tightest', () => {
    const lines = budgetLines([
      b({ scope: 'owner' }),
      b({ scope: 'session', limit_usd: 5, used_before_usd: 0, period: null }),
      b({ scope: 'agent', limit_usd: 50, used_before_usd: 10 }),
    ])
    expect(lines.map((l) => l.budget.scope)).toEqual(['session', 'agent', 'owner'])
    // session 60%, agent 26%, owner 44%.
    expect(tightest(lines)?.budget.scope).toBe('session')
    expect(lines[2]).toMatchObject({ sessionPct: 3, beforePct: 41 })
  })

  it('ties keep precedence; an unrecorded earlier use compares on the session’s share alone', () => {
    const tie = budgetLines([
      b({ scope: 'owner', limit_usd: 100, used_before_usd: 0, session_used_usd: 50 }),
      b({ scope: 'session', limit_usd: 10, used_before_usd: 0, session_used_usd: 5, period: null }),
    ])
    expect(tightest(tie)?.budget.scope).toBe('session')
    const unknown = budgetLines([
      b({ scope: 'owner', used_before_usd: null, session_used_usd: 30 }),
      b({ scope: 'agent', used_before_usd: 10, session_used_usd: 10 }),
    ])
    expect(tightest(unknown)?.budget.scope).toBe('owner')
  })

  it('use before the session is "not recorded" (null), never 0', () => {
    const [line] = budgetLines([b({ used_before_usd: null })])
    expect(line!.beforePct).toBeNull()
  })

  it('the running share adds each turn to what was used before', () => {
    const [line] = budgetLines([b({})])
    expect([...runningShare(line!)]).toEqual([
      ['t1', 42],
      ['t2', 44],
    ])
  })
})
