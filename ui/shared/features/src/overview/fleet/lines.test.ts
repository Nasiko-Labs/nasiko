/**
 * The Map view's token lines over the OV-1 mock: one line per pair, spend that reconciles with the agents and teams,
 * red only where spend is going up, a selection that lights what the part is wired to (not every team a problem
 * names), and routes that run through the gutters and streets and land at a part's own place: a router's port, a
 * model's row in its provider's column.
 */
import { describe, expect, it } from 'vitest'
import { setupPinnedSeed } from '@nasiko/testing/pinnedSeed'
import { LANES } from './layout'
import {
  buildLines,
  focusOf,
  kindId,
  litLine,
  ORG,
  BILL,
  BUDGET,
  pointsOf,
  roundPath,
  type Anchor,
  type Line,
} from './lines'
import { buildBoard, teamsOf } from './model'
import { estate, viewOf } from '@nasiko/testing/estateView'

setupPinnedSeed()

async function setup() {
  const e = await estate()
  const v = viewOf(e)
  const board = buildBoard(v)
  return { e, v, board, lines: buildLines(v, board) }
}
const of = (lines: Line[], route: Line['route']) => lines.filter((l) => l.route === route)

describe('the token lines', () => {
  it('joins the org to every team, red where the team is over pace', async () => {
    const { board, lines } = await setup()
    const org = of(lines, 'org-team')
    expect(org).toHaveLength(board.teams.length)
    for (const t of board.teams) {
      const l = org.find((x) => x.to === (t.id ?? 'team:none'))
      expect(l?.usd).toBeCloseTo(t.usd)
      expect(l?.hot).toBe((t.pace ?? 0) > 1)
    }
  })

  it('gives each agent its own line from each of its teams, splitting its spend between them', async () => {
    const { e, board, lines } = await setup()
    for (const row of board.agents) {
      const a = e.agents.find((x) => x.id === row.id)!
      const ins = of(lines, 'team-agent').filter((l) => l.to === a.id)
      expect(ins.map((l) => l.from).sort()).toEqual(
        (teamsOf(e, a).length ? teamsOf(e, a) : ['team:none']).sort(),
      )
      expect(ins.reduce((n, l) => n + l.usd, 0)).toBeCloseTo(a.spend_usd)
      for (const l of ins) expect(l.hot).toBe(row.costly)
      for (const t of a.tool_ids.filter((t) => board.tools.some((x) => x.id === t)))
        expect(lines.some((l) => l.route === 'agent-tool' && l.from === a.id && l.to === t)).toBe(
          true,
        )
    }
  })

  it('carries an agent through its router to each model, and the router passes on what reached it', async () => {
    const { board, lines } = await setup()
    for (const r of board.routers) {
      const inUsd = of(lines, 'agent-router')
        .filter((l) => l.to === r.id)
        .reduce((n, l) => n + l.usd, 0)
      const outUsd = of(lines, 'router-model')
        .filter((l) => l.from === r.id)
        .reduce((n, l) => n + l.usd, 0)
      // Agents past the twelve shown add to what the router sends on, never take from it.
      expect(outUsd).toBeGreaterThanOrEqual(inUsd - 1e-6)
      expect(outUsd).toBeCloseTo(r.spend)
      for (const l of of(lines, 'router-model').filter((x) => x.from === r.id))
        expect(l.via).toMatch(/^provider:/)
    }
  })

  it('sends coding harnesses past the routers, straight to their models, and from their teams', async () => {
    const { board, lines } = await setup()
    for (const c of board.harnesses) {
      expect(of(lines, 'harness-model').some((l) => l.from === kindId(c.key))).toBe(true)
      expect(of(lines, 'team-harness').some((l) => l.to === kindId(c.key))).toBe(true)
    }
    expect(of(lines, 'agent-router').every((l) => !l.from.startsWith('hk:'))).toBe(true)
  })

  it('draws a workflow to each step it calls, once', async () => {
    const { lines } = await setup()
    const flows = [...of(lines, 'flow-agent'), ...of(lines, 'flow-tool')]
    expect(flows.length).toBeGreaterThan(0)
    expect(new Set(flows.map((l) => l.id)).size).toBe(flows.length)
    expect(flows.every((l) => l.kind === 'calls' && !l.hot)).toBe(true)
  })
})

describe('a selection', () => {
  it("lights an agent's own teams, router, models, tools and workflows, and nothing a problem merely names", async () => {
    const { e, v, lines } = await setup()
    const a = e.agents.find((x) => x.name === 'Support Bot')!
    const f = focusOf(v, a.id)
    for (const t of teamsOf(e, a)) expect(f.has(t)).toBe(true)
    expect(f.has(ORG)).toBe(true)
    expect(f.has(a.router_id!)).toBe(true)
    for (const m of a.model_ids) expect(f.has(m)).toBe(true)
    for (const t of a.tool_ids) expect(f.has(t)).toBe(true)
    const others = e.groups.filter((g) => !teamsOf(e, a).includes(g.id))
    expect(others.some((g) => f.has(g.id))).toBe(false)
    // Its lines light; another team's line to another agent stays down.
    const own = lines.filter((l) => litLine(l, a.id, f))
    expect(own.some((l) => l.route === 'team-agent' && l.to === a.id)).toBe(true)
    expect(own.some((l) => l.route === 'agent-router' && l.from === a.id)).toBe(true)
    expect(
      lines.some((l) => l.route === 'team-agent' && l.to !== a.id && litLine(l, a.id, f)),
    ).toBe(false)
  })

  it("lights a team's agents and where their spend goes", async () => {
    const { e, v } = await setup()
    const g = e.groups[0]!
    const f = focusOf(v, g.id)
    for (const a of e.agents.filter((x) => teamsOf(e, x).includes(g.id))) {
      expect(f.has(a.id)).toBe(true)
      if (a.router_id) expect(f.has(a.router_id)).toBe(true)
    }
    expect(f.has(ORG)).toBe(true)
  })
})

describe('routing', () => {
  const box = (x: number, y: number, w = 100, h = 20): Anchor => ({ x, y, w, h })

  it('runs a team line down its own lane in the gutter and straight into the agent', () => {
    const l: Line = {
      id: 'x',
      route: 'team-agent',
      kind: 'spend',
      from: 't',
      to: 'a',
      via: null,
      lane: 2,
      slot: 0,
      usd: 1,
      hot: false,
    }
    const pts = pointsOf(
      l,
      new Map([
        ['t', box(40, 200)],
        ['a', box(589, 300)],
      ]),
    )
    const lane = LANES.g0 + 10 + 2 * 7
    expect(pts).toEqual([
      [140, 210],
      [lane, 210],
      [lane, 310],
      [589, 310],
    ])
    expect(pointsOf(l, new Map([['t', box(40, 200)]]))).toBeNull()
  })

  it("drops a router's line out of its tile, along s3, into the model's own row of its provider's column", () => {
    const l: Line = {
      id: 'x',
      route: 'router-model',
      kind: 'spend',
      from: 'r',
      to: 'm',
      via: 'p',
      lane: 1,
      slot: 2,
      usd: 1,
      hot: false,
    }
    const at = new Map([
      ['r', box(500, 498, 28, 28)],
      ['p', box(370, 660, 90, 18)],
      ['m', box(370, 700, 90, 20)],
    ])
    const pts = pointsOf(l, at)
    // The tile's middle; the street's lane; the model's slot in the column.
    expect(pts?.at(0)).toEqual([514, 526])
    expect(pts?.at(-2)).toEqual([370 + 6 + 2 * 7, LANES.s3 + 3 + 3])
    expect(pts?.at(-1)).toEqual([370 + 6 + 2 * 7, 702])
    // A model the column doesn't draw lands on its provider.
    at.delete('m')
    expect(pointsOf(l, at)?.at(-1)).toEqual([370 + 6 + 2 * 7, 662])
  })

  it('sends a harness kind straight to the bill, and the budget pays each provider along s4', () => {
    const bill: Line = {
      id: 'x',
      route: 'harness-bill',
      kind: 'spend',
      from: 'hk:claude',
      to: BILL,
      via: null,
      lane: 1,
      slot: 0,
      usd: 1,
      hot: false,
    }
    const at = new Map([
      ['hk:claude', box(360, 760, 200, 80)],
      [BILL, box(1116, 144, 300, 576)],
      [BUDGET, box(1436, 540, 280, 180)],
      ['p', box(400, 600, 90, 18)],
    ])
    // Out of the harnesses' right side and up into the bill's bottom edge.
    expect(pointsOf(bill, at)).toEqual([
      [560, 777],
      [1116 + 28 + 12, 777],
      [1116 + 28 + 12, 720],
    ])
    const pay: Line = { ...bill, route: 'budget-provider', from: BUDGET, to: 'p', lane: 0 }
    const pts = pointsOf(pay, at)
    // Out of the budget's left edge, down beside the bill, along s4, up into the provider's column.
    expect(pts?.at(0)).toEqual([1436, 584])
    expect(pts?.at(2)).toEqual([LANES.g3 + 4, LANES.s4 + 3])
    expect(pts?.at(-1)).toEqual([445, 602])
  })

  it('rounds the corners and drops repeated points', () => {
    expect(
      roundPath([
        [0, 0],
        [0, 0],
        [20, 0],
        [20, 20],
      ]),
    ).toBe('M0 0L14 0Q20 0 20 6L20 20')
    expect(roundPath([])).toBe('')
  })
})
