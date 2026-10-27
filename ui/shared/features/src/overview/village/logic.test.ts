/**
 * The village's pure rules (plans/feat-village-overview.md §3, V17–V31, V50–V54) over the OV-1 mock: each kind sits
 * only in its own district, on its land, and nothing overlaps; the free plan caps every district without ever hiding
 * something broken; tokens run from the battery to the teams and to the providers under a router or past them (and to
 * the dirt plot's battery only when billed together); ways turn only at corners; broken agents stand in front of the
 * barn; people stand where they work; projects keep their workflows; the HUD's numbers add up; the log sorts what
 * needs you; and the list puts the worst first.
 */
import { describe, expect, it } from 'vitest'
import {
  indexEstate,
  isOpen,
  nodeState,
  visibleProblems,
  type NodeState,
  type View,
} from '../estate/model'
import { estateSchema, type Estate, type Problem } from '../estate/types'
import type { EstateView } from '../estate/useEstateView'
import { layoutMap } from '../map/layout'
import { setupPinnedSeed } from '@nasiko/testing/pinnedSeed'
import { busiestBySource, cadenceOf, GAP, sourceOf, streamsOf } from './coins'
import {
  assignPlots,
  beaconOf,
  buildVillage,
  guideItems,
  lostOf,
  meterScale,
  moodOf,
  poseOf,
  weatherOf,
  type Village,
} from './model'
import { FREE_PLAN } from './plan'
import { LOST_MIN_USD } from './tuning'
import { MAIN, OUTSIDE } from './geo'
import { DIRECT_J, GATE_OUT_I, GATE_STREET_I, LANE_J, passOf, route, SPOUT } from './streets'
import { BAYS, CORE, PLOTS, type Kind, type Plot } from './zones'

setupPinnedSeed()

async function estate(): Promise<Estate> {
  const r = await fetch(new URL('/api/overview/estate?range=30d', globalThis.location.origin))
  return estateSchema.parse(((await r.json()) as { data: unknown }).data)
}

/** The estate view the page would build, without React (org scope unless asked, live, no filters). */
function viewOf(e: Estate, scope: 'org' | 'mine' = 'org'): EstateView {
  const nodes = indexEstate(e)
  const now = Date.parse(e.as_of)
  const view: View = { scope, day: e.days.length - 1, filters: {}, now }
  const mine = new Set(e.mine)
  const problems = visibleProblems(e, nodes, view)
  const state = new Map<string, NodeState>()
  for (const id of nodes.keys()) state.set(id, nodeState(e, nodes, problems, view, mine, id))
  return {
    estate: e,
    fallback: false,
    nodes,
    view,
    live: true,
    mine,
    problems,
    today: e.problems.filter((p) => isOpen(p, scope, now)),
    state,
    day: () => 'ok',
    broken: new Set(),
    inputs: new Map(),
    requests: [],
    layout: layoutMap(e, new Set()),
  }
}

const problem = (over: Partial<Problem>): Problem =>
  ({ kind: 'errors', also: null, tier: 'worse', entities: ['x'], ...over }) as Problem

describe('poses and weather', () => {
  it('knocks out only the subject of a broken error; anything else with errors or slowness limps', () => {
    expect(poseOf(false, 'a', [problem({ tier: 'broken', entities: ['a', 'b'] })])).toBe('down')
    expect(poseOf(false, 'b', [problem({ tier: 'broken', entities: ['a', 'b'] })])).toBe('hurt')
    expect(poseOf(false, 'a', [problem({ kind: 'slow', entities: ['a'] })])).toBe('hurt')
    expect(poseOf(false, 'a', [problem({ kind: 'cost', entities: ['a'] })])).toBe('live')
    expect(poseOf(false, 'a', [problem({ kind: 'cost', also: 'slow', entities: ['a'] })])).toBe(
      'hurt',
    )
    expect(poseOf(true, 'a', [problem({ tier: 'broken', entities: ['a'] })])).toBe('idle')
  })

  it('takes a place’s weather only from problems about it', () => {
    const crash = problem({ tier: 'broken', entities: ['agent', 'model:gpt-4o'] })
    expect(weatherOf([crash], ['model:gpt-4o'])).toBe('ok')
    expect(weatherOf([problem({ tier: 'broken', entities: ['model:x'] })], ['p', 'model:x'])).toBe(
      'down',
    )
    expect(weatherOf([problem({ kind: 'slow', entities: ['model:x'] })], ['model:x'])).toBe('warn')
    expect(weatherOf([problem({ kind: 'cost', entities: ['model:x'] })], ['model:x'])).toBe('ok')
  })
})

/** Every plot something stands on, by kind. */
function placed(v: Village): [Kind | 'core', Plot][] {
  return [
    ...Object.entries(CORE)
      .filter(([k]) => k !== 'depot')
      .map(([, p]) => ['core', p] as const),
    ...v.teams.map((x) => ['team', x] as const),
    ...v.mines.map((x) => ['mine', x] as const),
    ...v.xmines.map((x) => ['xmine', x] as const),
    ...v.gates.map((x) => ['gate', x] as const),
    ...v.mcps.map((x) => ['mcp', x] as const),
    ...v.tools.map((x) => ['tool', x] as const),
    ...v.harnesses.map((x) => ['harness', x] as const),
    ...v.yards.map((x) => ['project', x] as const),
  ] as [Kind | 'core', Plot][]
}

describe('districts and plots', () => {
  it('fills plots in order, and gives a claimed plot to the first newcomer', () => {
    expect([...assignPlots(['a', 'b'], 4).values()]).toEqual([0, 1])
    const claimed = assignPlots(['a', 'b', 'c'], 4, { plot: 3, known: ['a', 'b'] })
    expect(claimed.get('c')).toBe(3)
    expect(claimed.get('a')).toBe(0)
    // A claim past the open plots is ignored.
    expect(assignPlots(['a', 'b'], 2, { plot: 3, known: ['a'] }).get('b')).toBe(1)
  })

  it('puts each kind only on its own district’s plots, on its land, and nothing overlaps', async () => {
    const v = buildVillage(viewOf({ ...(await estate()), plan: 'team' }))
    const all = placed(v)
    const within = (p: Plot, l: typeof MAIN) =>
      p.i >= l.i0 && p.j >= l.j0 && p.i + p.w <= l.i0 + l.ni && p.j + p.d <= l.j0 + l.nj
    for (const [kind, p] of all) {
      if (kind !== 'core')
        expect(
          PLOTS[kind].some((q) => q.i === p.i && q.j === p.j),
          `${kind} at ${p.i},${p.j}`,
        ).toBe(true)
      expect(within(p, MAIN) || within(p, OUTSIDE), `${kind} at ${p.i},${p.j}`).toBe(true)
    }
    // Every plot of every district, and every landmark, keeps clear of every other.
    const plots: [string, Plot][] = [
      ...Object.entries(CORE)
        .filter(([k]) => k !== 'depot')
        .map(([k, p]) => [k, p] as [string, Plot]),
      ...(Object.keys(PLOTS) as Kind[])
        .filter((k) => k !== 'bay')
        .flatMap((k) => PLOTS[k].map((p, n) => [`${k}${n}`, p] as [string, Plot])),
    ]
    for (const [a, p] of plots)
      for (const [b, q] of plots)
        if (a < b)
          expect(
            p.i < q.i + q.w && q.i < p.i + p.w && p.j < q.j + q.d && q.j < p.j + p.d,
            `${a} overlaps ${b}`,
          ).toBe(false)
    // The depot's bays and the plots left empty account for every plot of each district.
    for (const k of Object.keys(PLOTS) as Kind[]) {
      const used = k === 'bay' ? v.cars.length : placed(v).filter(([x]) => x === k).length
      expect(used + v.vacant.filter((x) => x.kind === k).length, k).toBe(PLOTS[k].length)
    }
  })

  it('caps every district at the free plan, says how many more exist, and never hides something broken', async () => {
    const e = await estate()
    const v = buildVillage(viewOf(e))
    expect(v.teams).toHaveLength(Math.min(FREE_PLAN.team, e.groups.length))
    expect(v.cars).toHaveLength(Math.min(FREE_PLAN.bay, e.agents.length))
    expect(v.gates.length).toBeLessThanOrEqual(FREE_PLAN.gate)
    expect(v.mines.length).toBeLessThanOrEqual(FREE_PLAN.mine)
    expect(v.mcps.length).toBeLessThanOrEqual(FREE_PLAN.mcp)
    expect(v.tools.length).toBeLessThanOrEqual(FREE_PLAN.tool)
    expect(v.harnesses).toHaveLength(FREE_PLAN.harness)
    expect(v.districts.team.total).toBe(e.groups.length)
    expect(v.districts.bay.total).toBe(e.agents.length)
    // Broken agents, tools and providers are placed first.
    for (const a of e.agents)
      if (
        poseOf(
          a.idle,
          a.id,
          viewOf(e).problems.filter((p) => p.entities.includes(a.id)),
        ) === 'down'
      )
        expect(
          v.cars.some((c) => c.id === a.id),
          a.name,
        ).toBe(true)
    expect(v.mcps.some((t) => t.name === 'jira-cloud')).toBe(true)
    expect(v.mines.some((m) => m.name === 'Self-hosted')).toBe(true)
    // Platform runs over its budget, so its plot is drawn.
    expect(v.teams.find((t) => t.name === 'Platform')?.pace).toBeGreaterThan(1)
    // The busiest harness kind is the one the free plan shows.
    expect(v.harnesses[0]?.kind).toBe('claude')
  })

  it('opens every plot a district has on a paid plan', async () => {
    const e = await estate()
    const v = buildVillage(viewOf({ ...e, plan: 'team' }))
    expect(v.unlimited).toBe(true)
    expect(v.harnesses).toHaveLength(new Set(e.harnesses.map((h) => h.harness)).size)
    expect(v.cars).toHaveLength(Math.min(BAYS.length, e.agents.length))
    expect(v.vacant.every((p) => p.open)).toBe(true)
  })
})

describe('streets and tokens', () => {
  it('routes along the ways, turning only at corners', () => {
    const path = route([12, 4.9], [5.4, 16])
    expect(path[0]).toEqual([12, 4.9])
    expect(path.at(-1)).toEqual([5.4, 16])
    for (let k = 1; k < path.length; k++) {
      const [a, b] = [path[k - 1]!, path[k]!]
      expect(Math.abs(a[0] - b[0]) < 1e-6 || Math.abs(a[1] - b[1]) < 1e-6, `${a} → ${b}`).toBe(true)
    }
  })

  it('runs tokens from the battery to the teams, and under a router (or past them) to a provider', async () => {
    const e = await estate()
    const v = buildVillage(viewOf(e))
    const alloc = v.tokens.filter((t) => t.kind === 'alloc')
    expect(alloc.length).toBeGreaterThan(0)
    for (const t of alloc) {
      expect(t.path[0]).toEqual(SPOUT)
      expect(t.spend).toBeGreaterThan(0)
      // A team's line serves the team and the agents it uses.
      expect(v.teams.some((x) => t.nodes[0] === x.id)).toBe(true)
    }
    const out = v.tokens.filter((t) => t.kind === 'routed' || t.kind === 'direct')
    expect(out.length).toBeGreaterThan(0)
    // Every dollar an agent spends at a drawn provider runs down exactly one of these lines.
    const providerOf = new Map(e.models.map((m) => [m.id, m.provider_id]))
    const reaching = e.agents
      .filter((a) => a.spend_usd > 0)
      .filter((a) => a.model_ids.some((m) => v.mines.some((x) => x.id === providerOf.get(m))))
    expect(out.reduce((n, t) => n + t.spend, 0)).toBeCloseTo(
      reaching.reduce((n, a) => n + a.spend_usd, 0),
    )
    for (const t of out) {
      const [g, m] = t.key.slice(t.key.indexOf(':') + 1).split('|')
      const gate = v.gates.find((x) => x.id === g)
      const j = gate ? passOf(gate) : DIRECT_J
      expect(t.kind).toBe(gate ? 'routed' : 'direct')
      expect(t.path[0]).toEqual(SPOUT)
      // In at the routers' column, out from under the router (or the direct lane), down to the providers' lane.
      expect(t.path.some(([a, b]) => a === GATE_STREET_I && b === j)).toBe(true)
      expect(t.path.some(([a, b]) => a === GATE_OUT_I && b === j)).toBe(true)
      expect(t.path.some(([a, b]) => a === GATE_OUT_I && b === LANE_J)).toBe(true)
      expect(v.mines.some((x) => x.id === m)).toBe(true)
    }
    // No flow leaves an observatory: nothing starts at either.
    expect(v.tokens.every((t) => t.kind !== 'shared' || v.mines.some((m) => m.shared))).toBe(true)
  })

  it('sends tokens to the dirt plot’s battery only for a provider billed together with the harnesses', async () => {
    const e = await estate()
    const v = buildVillage(viewOf(e))
    const shared = v.tokens.filter((t) => t.kind === 'shared')
    expect(shared.map((t) => t.key)).toEqual(['shared:provider:anthropic'])
    // The dirt plot's battery feeds each coding tool it draws.
    const outside = v.tokens.filter((t) => t.kind === 'outside')
    expect(outside.map((t) => t.key)).toEqual(
      v.harnesses.filter((h) => h.spend > 0).map((h) => `outside:${h.kind}`),
    )
    for (const t of outside) expect(t.path[0]).toEqual(shared[0]?.path.at(-1))
    const apart = buildVillage(
      viewOf({ ...e, providers: e.providers.map((p) => ({ ...p, billed_with_harnesses: false })) }),
    )
    expect(apart.tokens.some((t) => t.kind === 'shared')).toBe(false)
  })
})

describe('token streams', () => {
  const len = (a: readonly number[], b: readonly number[]) =>
    Math.hypot((b[0] ?? 0) - (a[0] ?? 0), (b[1] ?? 0) - (a[1] ?? 0))

  it('carries every line’s flow, once per stretch, the trunk out of the battery the sum of them all', async () => {
    const v = buildVillage(viewOf(await estate()), { saved: 91 })
    const streams = streamsOf(v.tokens)
    // Flow × length is kept: cutting and joining the lines neither loses nor doubles any of it.
    const edges = v.tokens.reduce(
      (n, t) => n + t.spend * t.path.slice(1).reduce((m, q, k) => m + len(t.path[k] ?? q, q), 0),
      0,
    )
    expect(streams.reduce((n, s) => n + s.spend * len(s.from, s.to), 0)).toBeCloseTo(edges)
    // No two stretches lie over each other.
    for (const [k, a] of streams.entries())
      for (const b of streams.slice(k + 1)) {
        const sameJ = a.from[1] === a.to[1] && b.from[1] === b.to[1] && a.from[1] === b.from[1]
        const sameI = a.from[0] === a.to[0] && b.from[0] === b.to[0] && a.from[0] === b.from[0]
        if (!sameJ && !sameI) continue
        const ax = sameJ ? [a.from[0], a.to[0]] : [a.from[1], a.to[1]]
        const bx = sameJ ? [b.from[0], b.to[0]] : [b.from[1], b.to[1]]
        const overlap =
          Math.min(Math.max(...ax), Math.max(...bx)) - Math.max(Math.min(...ax), Math.min(...bx))
        expect(overlap, `${a.key} / ${b.key}`).toBeLessThan(1e-6)
      }
    const fromBattery = v.tokens.filter(
      (t) => t.path[0] === SPOUT || t.path[0]?.join() === SPOUT.join(),
    )
    const trunk = streams.find((s) => s.from.join() === SPOUT.join())
    expect(trunk?.spend).toBeCloseTo(fromBattery.reduce((n, t) => n + t.spend, 0))
    expect(trunk?.places.has('tank')).toBe(true)
    // The piggy bank's line is the saved amount, pink.
    expect(streams.some((s) => s.kind === 'saved' && s.spend === 91)).toBe(true)
  })

  it('packs a busier stream tighter and runs it faster', () => {
    expect(cadenceOf(100, 100)).toEqual({ gap: GAP.min, speed: expect.any(Number) as number })
    const busy = cadenceOf(80, 100)
    const quiet = cadenceOf(5, 100)
    expect(busy.gap).toBeLessThan(quiet.gap)
    expect(busy.speed).toBeGreaterThan(quiet.speed)
    expect(cadenceOf(0, 100).gap).toBe(GAP.max)
    // More than the busiest line (a trunk) packs no tighter than the tightest.
    expect(cadenceOf(300, 100).gap).toBe(GAP.min)
  })

  it('measures each battery’s streams against its own busiest line', async () => {
    const v = buildVillage(viewOf(await estate()), { saved: 91 })
    const max = busiestBySource(v.tokens)
    const island = v.tokens.filter((t) => sourceOf(t) === 'island')
    const dirt = v.tokens.filter((t) => sourceOf(t) === 'dirt')
    expect(max.island).toBe(Math.max(...island.map((t) => t.spend)))
    expect(max.dirt).toBe(Math.max(...dirt.map((t) => t.spend)))
    // The island's busiest user gets the tightest queue however much the coding tools spend.
    const top = island.find((t) => t.spend === max.island)
    expect(top && cadenceOf(top.spend, max.island).gap).toBe(GAP.min)
    // Only the piggy bank's own stretch is pink.
    for (const st of streamsOf(v.tokens).filter((x) => x.kind === 'saved'))
      expect([...st.places].includes('piggy')).toBe(true)
  })
})

describe('agents, people and projects', () => {
  it('stands broken agents in front of the barn, and gives every agent its color', async () => {
    const v = buildVillage(viewOf(await estate()))
    for (const c of v.cars.filter((x) => x.pose === 'down'))
      expect(BAYS[c.bay]?.service, c.name).toBe(true)
    expect(new Set(v.cars.map((c) => c.bay)).size).toBe(v.cars.length)
    expect(beaconOf('down')).toBe('broken')
    expect(beaconOf('idle')).toBe('asleep')
    expect(beaconOf('hurt')).toBe('slow')
    expect(beaconOf('live')).toBe('running')
    for (const c of v.cars.filter((x) => x.pose === 'live' && x.teams.length))
      expect(c.targets.length).toBeGreaterThan(0)
  })

  it('floats what failed runs cost only over a broken or slow piece, and only from LOST_MIN_USD', async () => {
    expect(lostOf('down', 4.11)).toBe(4.11)
    expect(lostOf('hurt', LOST_MIN_USD)).toBe(LOST_MIN_USD)
    expect(lostOf('hurt', LOST_MIN_USD - 0.01)).toBeNull()
    expect(lostOf('live', 40)).toBeNull()
    expect(lostOf('idle', 40)).toBeNull()
    expect(lostOf('down', null)).toBeNull()
    expect(lostOf('down', undefined)).toBeNull()
    // The mock: Pricing Engine fails 28% of runs (its problem), Code Reviewer crashes; a failed deploy costs nothing.
    const v = buildVillage(viewOf(await estate()))
    const lost = (name: string) => v.cars.find((c) => c.name === name)?.lost
    expect(lost('Pricing Engine')).toBeGreaterThan(LOST_MIN_USD)
    expect(lost('Code Reviewer')).toBeGreaterThan(LOST_MIN_USD)
    expect(lost('Doc Writer')).toBeNull()
    for (const c of v.cars) if (c.lost !== null) expect(['down', 'hurt']).toContain(c.pose)
  })

  it('stands people where they work, at most three to a spot, else with their team', async () => {
    const e = await estate()
    const v = buildVillage(viewOf(e))
    expect(v.people.length).toBeGreaterThan(0)
    const spots = new Map<string, number>()
    for (const p of v.people) spots.set(p.at, (spots.get(p.at) ?? 0) + 1)
    for (const n of spots.values()) expect(n).toBeLessThanOrEqual(3)
    for (const p of v.people) {
      const w = e.people.find((x) => x.id === p.id)?.working_on
      const there =
        (w?.kind === 'agent' && v.cars.some((c) => c.id === w.id)) ||
        (w?.kind === 'router' && v.gates.some((g) => g.id === w.id)) ||
        (w?.kind === 'project' && v.yards.some((y) => y.id === w.id))
      if (there) expect(p.at).toBe(w?.id)
      else expect(v.teams.some((t) => t.id === p.at)).toBe(true)
    }
  })

  it('draws projects as plots, the org’s first, with their workflows, and the rest of the workflows unfiled', async () => {
    const e = await estate()
    const v = buildVillage(viewOf(e))
    expect(v.yards[0]?.org).toBe(true)
    expect(v.yards[0]?.name).toBe('Platform operations')
    const unfiled = v.yards.find((y) => y.unfiled)
    expect(unfiled?.rails.map((r) => r.name)).toEqual(['Contract intake'])
    for (const y of v.yards) expect(y.rails.length).toBeLessThanOrEqual(3)
    // Without projects, every workflow is unfiled.
    const none = buildVillage(
      viewOf({
        ...e,
        projects: null,
        workflows: e.workflows.map((w) => ({ ...w, project_id: null })),
      }),
    )
    expect(none.yards).toHaveLength(1)
    expect(none.yards[0]?.rails).toHaveLength(Math.min(3, e.workflows.length))
    // The clock tower keeps every workflow, busiest first.
    expect(v.workflows).toHaveLength(e.workflows.length)
    expect(v.workflows.map((w) => w.runs)).toEqual(
      [...e.workflows].map((w) => w.runs).sort((a, b) => b - a),
    )
  })
})

describe('the mock village', () => {
  it('knocks out the agents whose crash is broken, and storms only the subject of a problem', async () => {
    const e = await estate()
    const v = buildVillage(viewOf({ ...e, plan: 'team' }))
    const pose = (name: string) => v.cars.find((t) => t.name === name)?.pose
    expect(pose('Code Reviewer')).toBe('down')
    expect(pose('Pricing Engine')).toBe('down')
    expect(pose('Research Agent')).toBe('hurt')
    expect(v.mines.find((m) => m.name === 'Self-hosted')?.weather).toBe('down')
    expect(v.mcps.find((h) => h.name === 'jira-cloud')?.weather).toBe('down')
    expect(v.tools.find((h) => h.name === 'github')?.weather).toBe('ok')
  })

  it('adds up the HUD over every agent, not only the drawn cars', async () => {
    const e = await estate()
    const v = buildVillage(viewOf(e))
    const c = v.hud.counts
    expect(c.live + c.idle + c.hurt + c.down).toBe(e.agents.length)
    const limit = e.groups.reduce((n, g) => n + (g.budget?.limit_usd ?? 0), 0)
    const spent = e.groups.reduce((n, g) => n + (g.budget?.spent_usd ?? 0), 0)
    expect(v.hud.budget?.left).toBeCloseTo(limit - spent)
    expect(v.hud.budget?.spent).toBeCloseTo(spent)
    const harness = e.harnesses.reduce((n, h) => n + (h.spend_usd ?? 0), 0)
    expect(v.outpost?.spend).toBeCloseTo(harness)
  })

  it('counts as ungoverned what has no budget and what a budget can only alert on, once each', async () => {
    const e = await estate()
    const { hud } = buildVillage(viewOf(e))
    const uncapped = e.groups.reduce((n, g) => n + (g.budget?.harness_usd ?? 0), 0)
    expect(hud.unbudgeted).toBeGreaterThan(0)
    expect(hud.uncapped).toBeCloseTo(uncapped)
    expect(hud.ungoverned).toBeCloseTo(hud.unbudgeted + uncapped)
  })

  it('draws every meter on the budget’s scale, else the ungoverned spend’s, else none', async () => {
    const { hud } = buildVillage(viewOf(await estate()))
    expect(meterScale(hud)).toBe(hud.budget?.limit)
    expect(meterScale({ ...hud, budget: null })).toBe(hud.ungoverned)
    expect(meterScale({ ...hud, budget: null, ungoverned: 0 })).toBeNull()
  })

  it('trends the org’s spend by day under the meters (V-11), one point per day', async () => {
    const e = await estate()
    const { hud } = buildVillage(viewOf(e))
    const t = hud.trend
    expect(t?.spent).toHaveLength(e.days.length)
    expect(t?.saved).toHaveLength(e.days.length)
    const day = e.daily?.[3]
    expect(t?.ungoverned[3]).toBeCloseTo((day?.unbudgeted_usd ?? 0) + (day?.uncapped_usd ?? 0))
    expect(t?.spent.some((x) => x > 0)).toBe(true)
    expect(buildVillage(viewOf({ ...e, daily: null })).hud.trend).toBeNull()
  })

  it('reads the viewer’s allowance in Mine, and leaves ungoverned and the org’s trends to Org', async () => {
    const e = await estate()
    const v = buildVillage(viewOf(e, 'mine'), { mine: true })
    expect(v.hud.budget?.limit).toBe(e.allowance?.limit_usd)
    expect(v.hud.budget?.spent).toBe(e.allowance?.spent_usd)
    expect(v.hud.unbudgeted).toBe(0)
    expect(v.hud.ungoverned).toBe(0)
    expect(v.hud.trend).toBeNull()
  })

  it('sorts the log: broken to Fix, worse to Watch, opportunities to Upgrades; Kiko flares', async () => {
    const e = await estate()
    const v = buildVillage(viewOf(e))
    const open = viewOf(e).problems
    expect(v.log.fix.map((x) => x.key)).toEqual(
      open.filter((p) => p.tier === 'broken').map((p) => p.id),
    )
    expect(v.log.watch.map((x) => x.key)).toEqual(
      open.filter((p) => p.tier === 'worse').map((p) => p.id),
    )
    expect(v.log.upgrade).toHaveLength(e.opportunities.length)
    expect(v.log.fix[0]?.sel).toBe(`problem:${v.log.fix[0]?.key}`)
    expect(moodOf(v.log)).toBe('flare')
    expect(guideItems(v.log)).toHaveLength(
      v.log.fix.length + v.log.approve.length + Math.min(3, v.log.watch.length),
    )
    expect(moodOf({ fix: [], approve: [], watch: [], upgrade: v.log.upgrade })).toBe('calm')
  })
})
