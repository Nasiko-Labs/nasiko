/**
 * The Map view's token lines over the OV-1 mock: one line per pair, spend that reconciles with the agents and teams,
 * red only where spend is going up, a selection that lights what the part is wired to (not every team a problem
 * names), and one curve per line from a part's right edge to the next part's own place: a router's row, a model's row
 * in its provider's row, a provider's line on the bill. Nothing reaches the bill but the providers.
 */
import { describe, expect, it } from 'vitest'
import { setupPinnedSeed } from '@nasiko/testing/pinnedSeed'
import { LANES } from './layout'
import {
  buildLines,
  curve,
  focusOf,
  kindFocus,
  kindId,
  litLine,
  ORG,
  BILL,
  billLine,
  pointsOf,
  restLines,
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
  it('draws no line inside a card, and none to the bill but the providers', async () => {
    const { lines } = await setup()
    expect(lines.some((l) => l.from === ORG)).toBe(false)
    expect(lines.filter((l) => l.to === BILL).every((l) => l.route === 'provider-bill')).toBe(true)
  })

  it("lights a harness kind picked whole with every install's own parts", async () => {
    const { v, board } = await setup()
    const c = board.harnesses[0]!
    const f = kindFocus(v, c.key)
    expect(f.has(kindId(c.key))).toBe(true)
    for (const h of v.estate.harnesses.filter((x) => x.harness === c.key))
      expect(f.has(h.id)).toBe(true)
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
      // Coding harnesses routed through it send on what they spend too.
      const harnessUsd = of(lines, 'harness-router')
        .filter((l) => l.to === r.id)
        .reduce((n, l) => n + l.usd, 0)
      // Agents past the twelve shown add to what the router sends on, never take from it.
      expect(outUsd).toBeGreaterThanOrEqual(inUsd - 1e-6)
      expect(outUsd).toBeCloseTo(r.spend + harnessUsd)
      for (const l of of(lines, 'router-model').filter((x) => x.from === r.id))
        expect(l.via).toMatch(/^provider:/)
    }
  })

  it('passes coding harnesses with no router through the No router row to their models, and from their teams', async () => {
    const { board, lines } = await setup()
    const direct = board.direct?.id
    expect(direct).toBeTruthy()
    for (const c of board.harnesses) {
      expect(of(lines, 'harness-router').some((l) => l.from === kindId(c.key))).toBe(true)
      expect(of(lines, 'team-harness').some((l) => l.to === kindId(c.key))).toBe(true)
    }
    expect(of(lines, 'harness-router').some((l) => l.to === direct)).toBe(true)
    expect(of(lines, 'router-model').some((l) => l.from === direct)).toBe(true)
    expect(of(lines, 'agent-router').every((l) => !l.from.startsWith('hk:'))).toBe(true)
  })

  it('draws a workflow to each step it calls, once', async () => {
    const { lines } = await setup()
    const flows = [...of(lines, 'flow-agent'), ...of(lines, 'flow-tool')]
    expect(flows.length).toBeGreaterThan(0)
    expect(new Set(flows.map((l) => l.id)).size).toBe(flows.length)
    expect(flows.every((l) => l.kind === 'calls' && !l.hot)).toBe(true)
  })

  it("marks failing the lines at a part in error, and a workflow's call only where its step fails", async () => {
    const { v, lines } = await setup()
    // A team is never the broken end: the line breaks at its part.
    const broken = (id: string) => v.nodes.get(id)?.kind !== 'group' && v.state.get(id) === 'errors'
    const failing = lines.filter((l) => l.failing)
    expect(failing.length).toBeGreaterThan(0)
    for (const l of lines.filter((x) => !x.from.startsWith('hk:') && !x.to.startsWith('hk:')))
      expect(l.failing).toBe(broken(l.to) || (!l.route.startsWith('flow') && broken(l.from)))
  })
})

describe('the board at rest', () => {
  it('keeps only failing and red lines, none from a team, and one line into each provider and failing tool', async () => {
    const { lines } = await setup()
    const rest = restLines(lines)
    expect(rest.length).toBeGreaterThan(0)
    expect(rest.every((l) => l.failing || l.hot)).toBe(true)
    expect(rest.some((l) => l.route.startsWith('team'))).toBe(false)
    const toProviders = rest.filter((l) => l.route === 'router-model')
    expect(toProviders.every((l) => l.to === l.via)).toBe(true)
    const alerting = lines.filter((l) => l.route === 'router-model' && (l.failing || l.hot))
    expect(toProviders.map((l) => l.to).sort()).toEqual(
      [...new Set(alerting.map((l) => l.via))].sort(),
    )
    // One master calls line into each part in error that is called, and none into a healthy one.
    const calls = rest.filter((l) => l.kind === 'calls')
    expect(calls.every((l) => l.endFails)).toBe(true)
    const into = lines.filter((l) => l.kind === 'calls' && l.endFails).map((l) => l.to)
    expect(calls.map((l) => l.to).sort()).toEqual([...new Set(into)].sort())
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

  const line = (route: Line['route'], from: string, to: string, lane: number): Line => ({
    id: 'x',
    route,
    kind: 'spend',
    from,
    to,
    via: route.endsWith('model') ? 'p' : null,
    lane,
    slot: 0,
    usd: 1,
    hot: false,
    failing: false,
    endFails: false,
  })

  it('runs a line from the right edge of its part to the left edge of the next, at their rows', () => {
    const l = line('team-agent', 't', 'a', 2)
    expect(
      pointsOf(
        l,
        new Map([
          ['t', box(40, 200)],
          ['a', box(589, 300)],
        ]),
      ),
    ).toEqual([
      [140, 210],
      [589, 310],
    ])
    expect(pointsOf(l, new Map([['t', box(40, 200)]]))).toBeNull()
  })

  it("runs a router's line into the model's own row, or its provider's row when the model isn't drawn", () => {
    const l = line('router-model', 'r', 'm', 1)
    const at = new Map([
      ['r', box(1000, 200, 250, 30)],
      ['p', box(1350, 240, 104, 18)],
      ['m', box(1480, 260, 200, 16)],
    ])
    expect(pointsOf(l, at)).toEqual([
      [1250, 210],
      [1480, 268],
    ])
    at.delete('m')
    expect(pointsOf(l, at)?.at(-1)).toEqual([1350, 253])
  })

  it("lands a provider on its own line on the bill, or the bill's edge when the line isn't drawn", () => {
    const at = new Map([
      [BILL, box(1700, 144, 300, 580)],
      ['p', box(1272, 200, 104, 18)],
      [billLine('p'), box(1712, 300, 276, 20)],
    ])
    const pay = line('provider-bill', 'p', BILL, 0)
    expect(pointsOf(pay, at)).toEqual([
      [1376, 209],
      [1712, 310],
    ])
    at.delete(billLine('p'))
    expect(pointsOf(pay, at)?.at(-1)).toEqual([1700, 154])
  })

  it("brackets a workflow's call into an agent in its own column, down the column's left side", () => {
    const l = { ...line('flow-agent', 'w', 'a', 1), kind: 'calls' as const }
    const pts = pointsOf(
      l,
      new Map([
        ['w', box(410, 900, 360, 20)],
        ['a', box(410, 200, 360, 20)],
      ]),
    )
    const x = LANES.g0 - 10 - 6
    expect(pts).toEqual([
      [410, 910],
      [x, 910],
      [x, 210],
      [410, 210],
    ])
  })

  it('draws two points as one smooth S, level where it leaves and lands', () => {
    expect(
      curve([
        [0, 0],
        [100, 50],
      ]),
    ).toBe('M0 0C50 0 50 50 100 50')
    expect(
      curve([
        [0, 10],
        [100, 10],
      ]),
    ).toBe('M0 10L100 10')
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
