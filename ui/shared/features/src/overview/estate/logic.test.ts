/**
 * The estate's pure rules: which problems show (scope, History day, filters, snooze, handled), the one colour a node
 * takes, day tiles, what a selection keeps in focus, the `sel` param, the map layout, the fallback map and OV-2's
 * optimistic patch.
 */
import { describe, expect, it } from 'vitest'
import type { Agent } from '@nasiko/features/agents/types'
import { layoutMap, midpoint, badgeAxis, clip, CAPS } from '../map/layout'
import { anchorOf, focusOf, formatSel, parseSel, type Selection } from '../map/selection'
import { applyPatch } from './api'
import { fallbackEstate, harnessKind } from './fallback'
import {
  dayState,
  flowBadges,
  indexEstate,
  isOpen,
  nodeState,
  pace,
  related,
  sessionState,
  visibleProblems,
  type View,
} from './model'
import type { Estate, Problem } from './types'

const day = (n: number) => `2026-10-${String(n).padStart(2, '0')}`
const DAYS = Array.from({ length: 14 }, (_, i) => day(i + 1))

function problem(over: Partial<Problem> & Pick<Problem, 'id'>): Problem {
  return {
    kind: 'errors',
    also: null,
    tier: 'broken',
    title: over.id,
    why: '',
    metric: { label: 'x', current: 1, target: 0, direction: 'lower', unit: '', min: 0, max: 2 },
    trend: [],
    since: day(10),
    confidence: 'high',
    evidence: [],
    fixed_when: '',
    action: { label: 'Open', target: { page: 'agents' } },
    entities: ['a1'],
    flow_id: 'agents-tools',
    count: 3,
    chain_id: null,
    owner_id: null,
    snoozed_until: null,
    handled_at: null,
    score: 100,
    mine: true,
    ...over,
  }
}

function estate(problems: Problem[] = []): Estate {
  return {
    as_of: `${day(14)}T12:00:00Z`,
    days: DAYS,
    me: 'p1',
    mine: ['p1', 'a1', 't1', 'g1'],
    groups: [
      {
        id: 'g1',
        name: 'Platform',
        budget: { limit_usd: 100, spent_usd: 50, projected_usd: 120, harness_usd: 10, hard: false },
      },
      { id: 'g2', name: 'Research', budget: null },
    ],
    people: [
      {
        id: 'p1',
        name: 'Ada Admin',
        group_id: 'g1',
        spend_usd: 5,
        sessions: 10,
        success_pct: 95,
        avg_context_tokens: 1,
        segment: 'value',
      },
      {
        id: 'p2',
        name: 'Omar Haddad',
        group_id: 'g2',
        spend_usd: 9,
        sessions: 40,
        success_pct: 70,
        avg_context_tokens: 1,
        segment: 'stuck',
      },
    ],
    workflows: [
      {
        id: 'w1',
        name: 'Ticket resolution',
        owner_id: 'p1',
        runs: 9,
        steps: [{ kind: 'agent', ref: 'a1', label: 'Support' }],
      },
    ],
    routers: [
      { id: 'r1', name: 'default', direct: false, note: null },
      { id: 'r0', name: 'No router', direct: true, note: null },
    ],
    providers: [{ id: 'pv1', name: 'OpenAI', brand: 'openai', share_pct: 100 }],
    models: [
      {
        id: 'm1',
        name: 'gpt-4o',
        short: 'GPT-4o',
        provider_id: 'pv1',
        spend_usd: 1,
        calls: 1,
        p95_ms: 1,
      },
    ],
    agents: [
      {
        id: 'a1',
        name: 'Support Bot',
        owner_id: 'p1',
        router_id: 'r1',
        model_ids: ['m1'],
        tool_ids: ['t1'],
        spend_usd: 1,
        runs: 1,
        success_pct: 90,
        p95_ms: 1,
        idle: false,
      },
      {
        id: 'a2',
        name: 'Doc Writer',
        owner_id: 'p2',
        router_id: 'r1',
        model_ids: ['m1'],
        tool_ids: [],
        spend_usd: 1,
        runs: 1,
        success_pct: 99,
        p95_ms: 1,
        idle: false,
      },
      {
        id: 'a3',
        name: 'Summarizer',
        owner_id: 'p2',
        router_id: null,
        model_ids: [],
        tool_ids: [],
        spend_usd: 0,
        runs: 0,
        success_pct: null,
        p95_ms: null,
        idle: true,
      },
    ],
    tools: [{ id: 't1', name: 'jira-cloud', brand: 'jira', connector_id: 't1' }],
    harnesses: [
      {
        id: 'h1',
        name: 'Codex (Ada)',
        harness: 'codex',
        owner_id: 'p1',
        model_ids: ['m1'],
        tool_ids: [],
        spend_usd: 3,
        sessions: ['ok', 'cost', 'failed'],
      },
    ],
    flows: [
      {
        id: 'agents-tools',
        from: 'agents',
        to: 'tools',
        volume: 10,
        unit: 'tool calls',
        spend_usd: null,
        direct: false,
      },
      {
        id: 'people-harnesses',
        from: 'people',
        to: 'harnesses',
        volume: 3,
        unit: 'sessions',
        spend_usd: null,
        direct: false,
      },
    ],
    problems,
    chains: [
      {
        id: 'c1',
        title: 'Why',
        confidence: 'high',
        steps: [
          { entity: 't1', what: 'jira breaks', detail: '' },
          { entity: 'a1', what: 'Support Bot fails', detail: '' },
        ],
        fix: 'Reconnect',
        clears: 2,
        action: { label: 'Open', target: { page: 'mcp' } },
      },
    ],
    opportunities: [],
    allowance: null,
  }
}

const NOW = Date.parse(`${day(14)}T12:00:00Z`)
const view = (over: Partial<View> = {}): View => ({
  scope: 'org',
  day: 13,
  filters: {},
  now: NOW,
  ...over,
})

describe('which problems show', () => {
  const ps = [
    problem({ id: 'p-broken', score: 140 }),
    problem({
      id: 'p-worse',
      tier: 'worse',
      kind: 'slow',
      score: 80,
      since: day(12),
      entities: ['a2'],
      mine: false,
      owner_id: 'p2',
    }),
    problem({ id: 'p-snoozed', snoozed_until: `${day(15)}T09:00:00Z` }),
    problem({ id: 'p-woke', snoozed_until: `${day(13)}T09:00:00Z`, score: 10 }),
    problem({ id: 'p-handled', handled_at: `${day(14)}T10:00:00Z` }),
  ]
  const e = estate(ps)
  const nodes = indexEstate(e)

  it('leaves out snoozed and handled problems, and brings a snooze back once it ends', () => {
    expect(visibleProblems(e, nodes, view()).map((p) => p.id)).toEqual([
      'p-broken',
      'p-worse',
      'p-woke',
    ])
  })

  it('Mine keeps only the viewer’s problems', () => {
    expect(visibleProblems(e, nodes, view({ scope: 'mine' })).map((p) => p.id)).toEqual([
      'p-broken',
      'p-woke',
    ])
    expect(isOpen(ps[1]!, 'mine', NOW)).toBe(false)
  })

  it('a past day shows only problems that had started by then', () => {
    expect(visibleProblems(e, nodes, view({ day: 10 })).map((p) => p.id)).toEqual([
      'p-broken',
      'p-woke',
    ])
    expect(visibleProblems(e, nodes, view({ day: 5 }))).toEqual([])
  })

  it('filters by kind (counting a second kind), tier, owner (Nobody included) and team', () => {
    expect(visibleProblems(e, nodes, view({ filters: { kind: 'slow' } })).map((p) => p.id)).toEqual(
      ['p-worse'],
    )
    expect(
      visibleProblems(e, nodes, view({ filters: { tier: 'worse' } })).map((p) => p.id),
    ).toEqual(['p-worse'])
    expect(visibleProblems(e, nodes, view({ filters: { owner: 'p2' } })).map((p) => p.id)).toEqual([
      'p-worse',
    ])
    expect(
      visibleProblems(e, nodes, view({ filters: { owner: 'none' } })).map((p) => p.id),
    ).toEqual(['p-broken', 'p-woke'])
    // a2 belongs to Omar, who is in Research.
    expect(visibleProblems(e, nodes, view({ filters: { group: 'g2' } })).map((p) => p.id)).toEqual([
      'p-worse',
    ])
  })
})

describe('a node’s color', () => {
  const e = estate([
    problem({ id: 'cost', kind: 'cost', tier: 'worse', score: 50 }),
    problem({ id: 'err', score: 140 }),
  ])
  const nodes = indexEstate(e)
  const ps = visibleProblems(e, nodes, view())
  const mine = new Set(e.mine)

  it('is its highest-scoring problem’s kind', () => {
    expect(nodeState(e, nodes, ps, view(), mine, 'a1')).toBe('errors')
  })

  it('a team takes its members’ problems', () => {
    expect(nodeState(e, nodes, ps, view(), mine, 'g1')).toBe('errors')
  })

  it('idle agents are grey, healthy ones green, and others’ nodes are ghosts in Mine', () => {
    expect(nodeState(e, nodes, ps, view(), mine, 'a3')).toBe('idle')
    expect(nodeState(e, nodes, ps, view(), mine, 'a2')).toBe('ok')
    expect(nodeState(e, nodes, ps, view({ scope: 'mine' }), mine, 'a2')).toBe('ghost')
  })

  it('a node outside the team filter is muted', () => {
    expect(nodeState(e, nodes, ps, view({ filters: { group: 'g1' } }), mine, 'a2')).toBe('muted')
  })
})

describe('day and session tiles', () => {
  const e = estate([problem({ id: 'err', since: day(10) })])
  const nodes = indexEstate(e)
  const mine = new Set(e.mine)

  it('color a day by the problems that had started by it, and the days after the slider as not reached', () => {
    expect(dayState(e, nodes, view(), mine, 'a1', 8)).toBe('ok')
    expect(dayState(e, nodes, view(), mine, 'a1', 9)).toBe('errors')
    expect(dayState(e, nodes, view({ day: 9 }), mine, 'a1', 11)).toBe('future')
  })

  it('a session is quiet unless failed, or explained by a problem on its harness', () => {
    expect(sessionState('failed', 0, 30, view(), 13, false)).toBe('errors')
    expect(sessionState('cost', 1, 30, view(), 13, false)).toBe('ok')
    expect(sessionState('cost', 1, 30, view(), 13, true)).toBe('cost')
    // Halfway back in History, the later half of the sessions hasn't happened yet.
    expect(sessionState('ok', 29, 30, view({ day: 6 }), 13, false)).toBe('future')
  })
})

describe('focus and badges', () => {
  const e = estate([problem({ id: 'err', entities: ['t1', 'a1'], chain_id: 'c1' })])
  const nodes = indexEstate(e)

  it('a node keeps what it connects to and what its problems involve', () => {
    const r = related(e, nodes, 'a1')
    for (const id of ['p1', 'r1', 'm1', 'pv1', 't1', 'w1']) expect(r.has(id)).toBe(true)
    expect(r.has('a2')).toBe(false)
  })

  it('a chain keeps only its steps; a flow keeps its two cards', () => {
    expect([...(focusOf(e, nodes, { type: 'chain', id: 'c1' }) ?? [])].sort()).toEqual(['a1', 't1'])
    const flow = focusOf(e, nodes, { type: 'flow', id: 'agents-tools' })!
    expect(flow.has('a2') && flow.has('t1') && !flow.has('p1')).toBe(true)
  })

  it('a flow’s badges sum what its problems touched, per kind', () => {
    expect(flowBadges(e.problems, 'agents-tools')).toEqual({ errors: 3 })
  })

  it('a team’s pace is its projection over its budget', () => {
    expect(pace(e.groups[0]!.budget)).toBeCloseTo(1.2)
    expect(pace(null)).toBeNull()
  })
})

describe('the sel param', () => {
  it('round-trips every kind, ids with colons included', () => {
    const all: Selection[] = [
      { type: 'node', id: 'model:gpt-4o' },
      { type: 'problem', id: 'ov-p1' },
      { type: 'flow', id: 'agents-tools' },
      { type: 'chain', id: 'ov-c1' },
      { type: 'opp', id: 'ov-o1' },
      { type: 'panel', id: 'agents', kind: 'errors' },
      { type: 'panel', id: 'tools', kind: null },
      { type: 'tile', id: 'model:x', day: 12 },
      { type: 'sess', id: 'h1', index: 0 },
    ]
    for (const s of all) expect(parseSel(formatSel(s))).toEqual(s)
  })

  it('drops junk instead of throwing', () => {
    for (const raw of [
      undefined,
      '',
      'node',
      'node:',
      'bogus:x',
      'panel:nope',
      'tile:a1:-1',
      'tile:a1:x',
    ])
      expect(parseSel(raw)).toBeNull()
  })

  it('a problem is about its first node', () => {
    const e = estate([problem({ id: 'err', entities: ['t1', 'a1'] })])
    expect(anchorOf(e, { type: 'problem', id: 'err' })).toBe('t1')
  })
})

describe('the map layout', () => {
  const e = estate([problem({ id: 'err' })])
  const L = layoutMap(e)

  it('places every node and never overlaps two cards', () => {
    for (const id of indexEstate(e).keys()) expect(L.pos.has(id)).toBe(true)
    const boxes = Object.values(L.panels)
    for (const a of boxes)
      for (const b of boxes)
        if (a !== b)
          expect(a.x + a.w <= b.x || b.x + b.w <= a.x || a.y + a.h <= b.y || b.y + b.h <= a.y).toBe(
            true,
          )
  })

  it('ends the columns on one baseline', () => {
    const P = L.panels
    expect(P.people.y + P.people.h).toBe(P.agents.y + P.agents.h)
    expect(P.agents.y + P.agents.h).toBe(P.harnesses.y + P.harnesses.h)
  })

  it('caps long lists with a "+N" row', () => {
    const big = estate()
    big.agents = Array.from({ length: CAPS.agents + 5 }, (_, i) => ({
      ...big.agents[0]!,
      id: `x${i}`,
      name: `Agent ${i}`,
    }))
    expect(layoutMap(big).agents.more).toBe(5)
  })

  it('draws only the flows the estate has', () => {
    expect(L.flows.map((f) => f.id).sort()).toEqual(['agents-tools', 'people-harnesses'])
  })

  it('puts badges along a long line and across a short one', () => {
    expect(
      badgeAxis([
        [0, 0],
        [200, 0],
      ]),
    ).toBe('x')
    expect(
      badgeAxis([
        [0, 0],
        [40, 0],
      ]),
    ).toBe('y')
    expect(
      badgeAxis([
        [0, 0],
        [0, 40],
      ]),
    ).toBe('x')
    expect(
      midpoint([
        [0, 0],
        [10, 0],
        [10, 10],
      ]),
    ).toEqual([10, 0])
  })

  it('clips long labels with an ellipsis', () => {
    expect(clip('Onboarding Guide for new hires', 80)).toMatch(/…$/)
    expect(clip('SQL', 80)).toBe('SQL')
  })
})

describe('the fallback map (no OV-1)', () => {
  const agent = (id: string, over: Partial<Agent> = {}) =>
    ({
      id,
      name: id,
      display_name: id,
      owner_id: 'me',
      status: 'running',
      tags: [],
      metadata: {},
      ...over,
    }) as Agent

  it('draws agents and harnesses from the directory, problems from the fleet ratings, nothing guessed', () => {
    const e = fallbackEstate({
      me: { sub: 'me', username: 'ada' },
      agents: [
        agent('a'),
        agent('b', { owner_id: 'other' }),
        agent('maya-codex', { tags: ['coding-agent'] }),
      ],
      ratings: {
        counts: { healthy: 1, watch: 0, action: 1, unknown: 0 },
        byId: new Map([
          [
            'a',
            {
              id: 'a',
              name: 'a',
              rating: 'action',
              reasons: [{ dimension: 'reliability', level: 'action', text: 'crashed' }],
            },
          ],
        ]),
        watch: [],
        action: [],
      },
      now: NOW,
    })
    expect(e.agents.map((a) => a.id)).toEqual(['a', 'b'])
    expect(e.harnesses.map((h) => h.harness)).toEqual(['codex'])
    expect(e.problems).toHaveLength(1)
    expect(e.problems[0]).toMatchObject({
      kind: 'errors',
      tier: 'broken',
      entities: ['a'],
      mine: true,
    })
    expect(e.groups).toEqual([])
    expect(e.workflows).toEqual([])
    // Other owners are counted, never named.
    expect(e.people.map((p) => p.name)).toEqual(['ada', 'Person 1'])
    expect(harnessKind('someone-claude-code')).toBe('claude')
  })
})

describe('OV-2’s patch, applied before the server answers', () => {
  it('assigns, snoozes, handles and undoes the way the server will', () => {
    const p = problem({ id: 'x' })
    expect(applyPatch(p, { owner_id: 'p1' }, NOW).owner_id).toBe('p1')
    expect(applyPatch(p, { snoozed_until: 'later' }, NOW).snoozed_until).toBe('later')
    expect(applyPatch(p, { handled: true }, NOW).handled_at).toBe(new Date(NOW).toISOString())
    expect(applyPatch({ ...p, handled_at: 'then' }, { handled: false }, NOW).handled_at).toBeNull()
    expect(applyPatch({ ...p, owner_id: 'p1' }, { owner_id: null }, NOW).owner_id).toBeNull()
  })
})
