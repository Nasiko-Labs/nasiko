import { describe, expect, it } from 'vitest'
import { selectFrom, TIER_DEFAULTS } from '@nasiko/mocks/optimization'
import type { ChatMessage } from '@nasiko/features/chat/types'
import {
  approx,
  contextReport,
  orgPolicyApplied,
  requestSpanOf,
  tierProblems,
  tokenDelta,
  changedFields,
  compressionState,
  numberVisible,
  tierFigure,
  toOwnedAgent,
} from './logic'

describe('numberVisible (design review 1A)', () => {
  it('numbers only the shown sections, in order', () => {
    expect(
      numberVisible([
        { key: 'switch', shown: false },
        { key: 'strategy', shown: true },
        { key: 'budget', shown: true },
        { key: 'compression', shown: true },
      ]),
    ).toEqual([
      { key: 'strategy', n: 1 },
      { key: 'budget', n: 2 },
      { key: 'compression', n: 3 },
    ])
  })
  it('keeps 1..n when everything is shown', () => {
    expect(
      numberVisible([
        { key: 'a', shown: true },
        { key: 'b', shown: true },
      ]).map((s) => s.n),
    ).toEqual([1, 2])
  })
})

describe('compressionState (2E, eng F1/E2: compression_opt_in)', () => {
  const a = (id: string, on: boolean) => ({ id, name: id, compressEnabled: on })
  it('is off with no agents: count(*) > 0 is false (F1)', () => {
    expect(compressionState([])).toEqual({ on: false, reason: 'no-agents' })
  })
  it('is on only when every owned agent allows it and nothing unseen could count', () => {
    const all = [a('x', true), a('y', true)]
    expect(compressionState(all, { serverOn: true })).toEqual({ on: true, reason: 'all-on' })
    // Review finding 1: an unknown server flag, a hidden internal agent (EE superuser) or a capped list is "likely".
    const likely = { on: 'likely', reason: 'likely-on' }
    expect(compressionState(all)).toEqual(likely)
    expect(compressionState(all, { serverOn: true, hiddenMayCount: true })).toEqual(likely)
    expect(compressionState(all, { serverOn: true, capped: true })).toEqual(likely)
  })
  it('names the agents that block it, with the total', () => {
    const s = compressionState([a('x', true), a('y', false), a('z', false)])
    expect(s).toEqual({
      on: false,
      reason: 'some-off',
      off: [a('y', false), a('z', false)],
      total: 3,
    })
  })
  it('the server flag wins over the agents', () => {
    expect(compressionState([a('x', true)], { serverOn: false })).toEqual({
      on: false,
      reason: 'server-off',
    })
    // A visible agent with it off is certain whatever is hidden.
    expect(compressionState([a('x', false)], { hiddenMayCount: true }).on).toBe(false)
  })
  it('reads a list row: compress_enabled true only, display name first (harness rows count too)', () => {
    expect(
      toOwnedAgent({
        id: '1',
        name: 'raw',
        display_name: 'Shown',
        compress_enabled: true,
      } as never),
    ).toEqual({
      id: '1',
      name: 'Shown',
      compressEnabled: true,
    })
    expect(toOwnedAgent({ id: '2', name: 'raw' })).toEqual({
      id: '2',
      name: 'raw',
      compressEnabled: false,
    })
  })
})

describe('tierFigure (2B)', () => {
  it('is null without the server’s tier values', () => {
    expect(tierFigure(undefined, 'pacms', 'medium')).toBeNull()
  })
  it('PACMS reads token budgets; Top-K pairs; Last-K messages', () => {
    expect(tierFigure(TIER_DEFAULTS, 'pacms', 'high')).toEqual({ kind: 'tokens', value: 5000 })
    expect(tierFigure(TIER_DEFAULTS, 'topk', 'low')).toEqual({ kind: 'pairs', value: 1 })
    expect(tierFigure(TIER_DEFAULTS, 'lastk', 'medium')).toEqual({ kind: 'messages', value: 5 })
  })
})

describe('save edits (2C)', () => {
  const saved = { strategy: 'pacms', level: 'medium', enabled: true } as const
  it('sends only what changed, and the switch only when the server has one', () => {
    expect(changedFields(saved, { ...saved, level: 'low' }, true)).toEqual({ level: 'low' })
    expect(changedFields(saved, { ...saved, enabled: false }, false)).toEqual({})
    expect(
      changedFields(saved, { strategy: 'lastk', level: 'high', enabled: false }, true),
    ).toEqual({
      strategy: 'lastk',
      level: 'high',
      enabled: false,
    })
  })
})

describe('approx', () => {
  it('keeps small numbers, rounds larger ones to two significant digits', () => {
    expect(approx(6.4)).toBe(6)
    expect(approx(983)).toBe(980)
    expect(approx(9_812)).toBe(9_800)
  })
})

describe('the preview mock’s selection (optimization.ts)', () => {
  const msg = (i: number, chars: number): ChatMessage => ({
    id: String(i),
    session_id: 's',
    role: i % 2 ? 'assistant' : 'user',
    content: 'x'.repeat(chars),
    timestamp: '2026-10-01T00:00:00Z',
  })
  const pool = Array.from({ length: 40 }, (_, i) => msg(i, 1000)) // 250 tokens each
  it('PACMS keeps the always-kept newest, then fills the budget', () => {
    expect(selectFrom(pool, 'pacms', 'low')).toEqual({ messages: 3, tokens: 750 })
    expect(selectFrom(pool, 'pacms', 'medium')).toEqual({ messages: 4, tokens: 1000 })
    expect(selectFrom(pool, 'pacms', 'high')).toEqual({ messages: 20, tokens: 5000 })
  })
  it('Top-K keeps K pairs; Last-K K messages', () => {
    expect(selectFrom(pool, 'topk', 'medium').messages).toBe(10)
    expect(selectFrom(pool, 'lastk', 'high').messages).toBe(20)
  })
})

describe('contextReport (CX-V1 attributes, eng F5; F2)', () => {
  const full = {
    'nasiko.context.strategy': 'pacms',
    'nasiko.context.level': 'medium',
    'nasiko.context.pool': 40,
    'nasiko.context.pool_tokens': 9800,
    'nasiko.context.kept': 12,
    'nasiko.context.kept_tokens_est': 980,
    'nasiko.context.compressed_bytes_saved': 3174,
  }
  it('reads a full report', () => {
    expect(contextReport(full)).toEqual({
      strategy: 'pacms',
      level: 'medium',
      pool: { messages: 40, tokens: 9800 },
      kept: { messages: 12, tokens: 980 },
      compressedBytesSaved: 3174,
      orgPolicyApplied: false,
    })
  })
  it('is null without the counts, so the trace renders as before (E4)', () => {
    expect(contextReport({})).toBeNull()
    expect(contextReport({ ...full, 'nasiko.context.pool': 'lots' })).toBeNull()
    expect(contextReport({ ...full, 'nasiko.context.kept': -1 })).toBeNull()
  })
  it('takes numbers sent as strings, drops unknown enums and a zero saving', () => {
    const r = contextReport({
      ...full,
      'nasiko.context.pool': '40',
      'nasiko.context.strategy': 'magic',
      'nasiko.context.compressed_bytes_saved': 0,
    })!
    expect(r.pool.messages).toBe(40)
    expect(r.strategy).toBeNull()
    expect(r.compressedBytesSaved).toBeNull()
  })
  it('shows the org policy only when the flag is true (OSS records false)', () => {
    expect(
      contextReport({ ...full, 'nasiko.prompt_context.org_applied': true })!.orgPolicyApplied,
    ).toBe(true)
    expect(
      contextReport({ ...full, 'nasiko.prompt_context.org_applied': false })!.orgPolicyApplied,
    ).toBe(false)
  })
  it('reads the org-policy flag as a boolean or the string an exporter may write, with or without counts', () => {
    expect(
      contextReport({ ...full, 'nasiko.prompt_context.org_applied': 'true' })!.orgPolicyApplied,
    ).toBe(true)
    expect(orgPolicyApplied({ 'nasiko.prompt_context.org_applied': true })).toBe(true)
    expect(orgPolicyApplied({ 'nasiko.prompt_context.org_applied': 'yes' })).toBe(false)
    expect(orgPolicyApplied({})).toBe(false)
  })

  it('tokenDelta is positive when more was sent than the baseline', () => {
    expect(
      tokenDelta(contextReport({ ...full, 'nasiko.context.kept_tokens_est': 30_000 })!),
    ).toBeGreaterThan(0)
  })

  it('tokenDelta is the whole-percent change from the baseline', () => {
    expect(tokenDelta(contextReport(full)!)).toBe(-90)
    expect(tokenDelta(contextReport({ ...full, 'nasiko.context.pool_tokens': 0 })!)).toBeNull()
  })
})

describe('requestSpanOf (design review 1B)', () => {
  const sp = (id: string, name: string, parent_id: string | null = null) => ({
    node: { id, name, parent_id },
  })
  const spans = [
    sp('root', 'planner'),
    sp('p1', 'a2a.proxy', 'root'),
    sp('l1', 'llm.qa_summary', 'p1'),
    sp('l2', 'llm.plan', 'root'),
    sp('d', 'a2a.dispatch'),
  ]
  it('finds the nearest request span at or above', () => {
    expect(requestSpanOf(spans, spans[2])?.node.id).toBe('p1')
    expect(requestSpanOf(spans, spans[1])?.node.id).toBe('p1')
    expect(requestSpanOf(spans, spans[4])?.node.id).toBe('d')
  })
  it('is null outside any request, and survives a parent cycle', () => {
    expect(requestSpanOf(spans, spans[3])).toBeNull()
    const loop = [sp('a', 'x', 'b'), sp('b', 'y', 'a')]
    expect(requestSpanOf(loop, loop[0])).toBeNull()
  })
})

describe('tierProblems (§4)', () => {
  const ok = {
    pacms_budget: { low: 500, medium: 1000, high: 5000 },
    context_k: { low: 1, medium: 5, high: 20 },
    pool_size: 150,
    mandatory_recent: 3,
    compress_history: true,
    compress_min_bytes: 2048,
  }
  it('passes the defaults', () => expect(tierProblems(ok)).toEqual({}))
  it('wants whole numbers of 1 or more', () => {
    expect(tierProblems({ ...ok, pool_size: 0, compress_min_bytes: 1.5 })).toMatchObject({
      pool_size: 'whole',
      compress_min_bytes: 'whole',
    })
    expect(tierProblems({ ...ok, context_k: { low: Number.NaN, medium: 5, high: 20 } })).toEqual({
      'context_k.low': 'whole',
    })
  })
  it('keeps Low ≤ Medium ≤ High, blaming the later tier', () => {
    expect(tierProblems({ ...ok, pacms_budget: { low: 500, medium: 400, high: 5000 } })).toEqual({
      'pacms_budget.medium': 'order',
    })
    expect(tierProblems({ ...ok, context_k: { low: 1, medium: 5, high: 4 } })).toEqual({
      'context_k.high': 'order',
    })
  })
  it('keeps the pool at least the always-kept messages', () => {
    expect(tierProblems({ ...ok, pool_size: 2 })).toEqual({ pool_size: 'pool' })
  })
  it('refuses numbers past safe integers (1e21 is an integer, but not one the server can store)', () => {
    expect(tierProblems({ ...ok, compress_min_bytes: 1e21 })).toEqual({
      compress_min_bytes: 'whole',
    })
  })
})
