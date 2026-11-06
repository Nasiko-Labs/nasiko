/**
 * The Map view's token lines (pure, tested in lines.test.ts): every connection between the estate's parts,
 * drawn on the board between the parts it joins, one line per pair so each agent, workflow and tool has its own.
 *
 * - Spend (blue, as wide as what it carries; red where spend is going up: a team over pace, or a cost problem on
 *   the parts carrying more than HOT_SHARE of the line's spend): team → agent → its LLM router → each model the
 *   router sends it to; team → coding harness → its router → each model; an agent or a coding harness with no router
 *   passes through the routers' No router row the same way (Ans, 2026-11-04); each provider → its line on the bill
 *   (what it is paid, agents and coding harnesses alike). Nothing reaches the bill past the providers (Ans, 2026-11-05:
 *   "I don't want any of them bypassing providers and models").
 * - Calls (thin, lighter): workflow → each step's agent or tool, agent → tool, coding harness → tool.
 * - Failing (strong red, spend or calls): a line touching a part in error (`failing`). With nothing picked the board
 *   draws only these and the red spend lines, fewer still (`restLines`), so what is breaking shows at rest (Ans,
 *   2026-11-05: "keep all the lines that represent an error or warning by default").
 *
 * `focusOf` is what a selection keeps lit: the estate's own `related` rule plus the path a part's spend takes (an
 * agent's teams, a router's models, a team's agents and where they spend). Every line joins two neighbouring columns
 * and is drawn as one smooth curve from the right edge of its part to the left edge of the next (`pointsOf`, `curve`),
 * so lines sharing a part fan out from one point instead of running in lanes (Ans, 2026-11-05: "organize the lines in
 * such a way that it's much soothing"). The only exception is a workflow's call to an agent in its own column: a
 * bracket out of the workflow's left edge, up the fleet's left gutter and into the agent's left edge. The lines are
 * drawn under the cards, so a line
 * ends at a card's edge, never inside one.
 */
import { problemsOf, related } from '../estate/model'
import type { EstateView } from '../estate/useEstateView'
import { LANES } from './layout'
import { modelWeights, routeOf, SHOWN, teamsOf, type Board } from './model'

/**
 * The anchors that aren't estate nodes: the org's budget, the people in no team, a harness kind's row, the two
 * TokenOps cards (the bill and the month's budget) and a provider's line on the bill, where its line lands.
 */
export const ORG = 'org'
export const TEAMLESS = 'team:none'
export const BILL = 'card:bill'
export const BUDGET = 'card:budget'
export const kindId = (kind: string) => `hk:${kind}`
export const billLine = (id: string) => `bill:${id}`

type Route =
  | 'team-agent'
  | 'team-harness'
  | 'flow-agent'
  | 'flow-tool'
  | 'agent-tool'
  | 'agent-router'
  | 'router-model'
  | 'harness-router'
  | 'harness-tool'
  | 'provider-bill'

/** A line reads red when parts with a cost problem carry more than this share of its spend. */
const HOT_SHARE = 0.25

export interface Line {
  id: string
  route: Route
  kind: 'spend' | 'calls'
  from: string
  to: string
  /** A line to a model lands on its provider's row: the provider's id. */
  via: string | null
  /** Its lane in the gutter it runs down (the team's, the tool's, the workflow's, the router's or the harness kind's place). */
  lane: number
  /** A line to a model: the model's row in its provider's column (the column's own slot when the model isn't drawn). */
  slot: number
  usd: number
  hot: boolean
  /** It touches a part in error, so it is drawn even with nothing picked (Ans, 2026-11-05). */
  failing: boolean
  /** The part it lands on is the one in error (not the part it leaves). */
  endFails: boolean
}

export function buildLines(v: EstateView, board: Board): Line[] {
  const e = v.estate
  const costly = (id: string) =>
    problemsOf(v.nodes, v.problems, id).some((p) => p.kind === 'cost' || p.also === 'cost')
  // A part in error; a coding harness kind is one when any of its installs is. Teams never are: a line breaks at its part.
  const broken = (id: string): boolean =>
    id.startsWith('hk:')
      ? e.harnesses.some((h) => kindId(h.harness) === id && v.state.get(h.id) === 'errors')
      : v.nodes.get(id)?.kind !== 'group' && v.state.get(id) === 'errors'
  const providerOf = (m: string) => e.models.find((x) => x.id === m)?.provider_id ?? null
  const groupOf = (person: string | null) =>
    person ? (e.people.find((p) => p.id === person)?.group_id ?? null) : null

  const teamLane = new Map(board.teams.map((t, i) => [t.id ?? TEAMLESS, i]))
  const agents = new Map(board.agents.map((a) => [a.id, a]))
  const toolLane = new Map(board.tools.map((t, i) => [t.id, i]))
  // The No router row's lane follows the routers' own.
  const routerLane = new Map(board.routers.map((r, i) => [r.id, i]))
  if (board.direct) routerLane.set(board.direct.id, SHOWN.routers)
  const models = new Set(board.providers.flatMap((p) => p.models.map((m) => m.id)))
  const slotOf = new Map(board.providers.flatMap((p) => p.models.map((m, i) => [m.id, i] as const)))
  const providers = new Set(board.providers.map((p) => p.id))
  const shownModel = (m: string) => models.has(m) || providers.has(providerOf(m) ?? '')

  const out = new Map<string, Line>()
  // What each line's costly parts carry; a line is red once that passes HOT_SHARE of it (see the end).
  const heat = new Map<string, number>()
  const add = (
    l: Omit<Line, 'id' | 'via' | 'slot' | 'failing' | 'endFails'> & { via?: string | null },
  ) => {
    const id = `${l.route}:${l.from}>${l.to}`
    if (l.hot) heat.set(id, (heat.get(id) ?? 0) + l.usd)
    const was = out.get(id)
    if (was) {
      was.usd += l.usd
      was.hot ||= l.hot
    } else
      out.set(id, {
        ...l,
        id,
        via: l.via ?? null,
        slot: slotOf.get(l.to) ?? SHOWN.models,
        failing: false,
        endFails: false,
      })
  }
  const toModels = (
    route: Route,
    from: string,
    lane: number,
    ids: readonly string[],
    usd: number,
    hot: boolean,
  ) => {
    for (const [m, w] of modelWeights(e, ids))
      if (shownModel(m))
        add({ route, kind: 'spend', from, to: m, via: providerOf(m), lane, usd: usd * w, hot })
  }

  for (const row of board.agents) {
    const a = e.agents.find((x) => x.id === row.id)
    if (!a) continue
    const ts = teamsOf(e, a)
    for (const t of ts.length ? ts : [TEAMLESS])
      if (teamLane.has(t))
        add({
          route: 'team-agent',
          kind: 'spend',
          from: t,
          to: a.id,
          lane: teamLane.get(t) ?? 0,
          usd: a.spend_usd / Math.max(1, ts.length),
          hot: row.costly,
        })
    for (const t of a.tool_ids)
      if (toolLane.has(t))
        add({
          route: 'agent-tool',
          kind: 'calls',
          from: a.id,
          to: t,
          lane: toolLane.get(t) ?? 0,
          usd: 0,
          hot: false,
        })
    const r = routeOf(e, a.router_id)
    if (routerLane.has(r))
      add({
        route: 'agent-router',
        kind: 'spend',
        from: a.id,
        to: r,
        lane: routerLane.get(r) ?? 0,
        usd: a.spend_usd,
        hot: row.costly,
      })
  }

  // Each router row (the No router one too) passes on to the models what reached it from agents and harnesses.
  for (const a of e.agents) {
    const r = routeOf(e, a.router_id)
    const lane = routerLane.get(r)
    if (lane !== undefined)
      toModels(
        'router-model',
        r,
        lane,
        a.model_ids,
        a.spend_usd,
        agents.get(a.id)?.costly ?? costly(a.id),
      )
  }

  board.workflows.forEach((w, i) => {
    const flow = e.workflows.find((x) => x.id === w.id)
    for (const s of flow?.steps ?? []) {
      if (!s.ref) continue
      if (agents.has(s.ref))
        add({
          route: 'flow-agent',
          kind: 'calls',
          from: w.id,
          to: s.ref,
          lane: i,
          usd: 0,
          hot: false,
        })
      else if (toolLane.has(s.ref))
        add({
          route: 'flow-tool',
          kind: 'calls',
          from: w.id,
          to: s.ref,
          lane: i,
          usd: 0,
          hot: false,
        })
    }
  })

  for (const [k, c] of board.harnesses.entries()) {
    const kind = kindId(c.key)
    const installs = e.harnesses.filter((h) => h.harness === c.key)
    for (const h of installs) {
      const hot = costly(h.id)
      const t = groupOf(h.owner_id) ?? TEAMLESS
      if (teamLane.has(t))
        add({
          route: 'team-harness',
          kind: 'spend',
          from: t,
          to: kind,
          lane: teamLane.get(t) ?? 0,
          usd: h.spend_usd ?? 0,
          hot,
        })
      const r = routeOf(e, h.router_id)
      const lane = routerLane.get(r)
      if (lane !== undefined) {
        add({
          route: 'harness-router',
          kind: 'spend',
          from: kind,
          to: r,
          lane: k,
          usd: h.spend_usd ?? 0,
          hot,
        })
        toModels('router-model', r, lane, h.model_ids, h.spend_usd ?? 0, hot)
      }
      for (const tool of h.tool_ids)
        if (toolLane.has(tool))
          add({
            route: 'harness-tool',
            kind: 'calls',
            from: kind,
            to: tool,
            lane: k,
            usd: 0,
            hot: false,
          })
    }
  }
  board.providers.forEach((p, i) =>
    add({
      route: 'provider-bill',
      kind: 'spend',
      from: p.id,
      to: BILL,
      lane: i,
      usd: p.spend,
      hot: costly(p.id),
    }),
  )
  // A router or model with a cost problem of its own makes all of its lines red.
  for (const l of out.values()) {
    const own = l.route === 'router-model' && (costly(l.from) || costly(l.to))
    const share = l.usd > 0 ? (heat.get(l.id) ?? 0) / l.usd : 0
    l.hot = own || (l.kind === 'spend' && (l.usd > 0 ? share > HOT_SHARE : l.hot))
    // A workflow's call is failing only where its step's part is: the workflow reads failing for any one of them.
    l.endFails = broken(l.to)
    l.failing = l.endFails || (!l.route.startsWith('flow') && broken(l.from))
  }
  return [...out.values()]
}

/**
 * What a selection keeps lit on the board: the estate's `related` rule, plus the way its spend goes. Teams and coding
 * harness kinds follow only from what the part is wired to (an agent's own teams, a router's agents), never from a
 * problem that merely names it, so a pick doesn't light half the org.
 */
export function focusOf(v: EstateView, id: string): Set<string> {
  const e = v.estate
  const s = related(e, v.nodes, id)
  const kind = v.nodes.get(id)?.kind
  const providerOf = (m: string) => e.models.find((x) => x.id === m)?.provider_id
  const path = (ids: readonly string[]) =>
    ids.forEach((m) => {
      s.add(m)
      const p = providerOf(m)
      if (p) s.add(p)
    })
  const groupOf = (person: string | null) =>
    person ? (e.people.find((p) => p.id === person)?.group_id ?? null) : null
  const models = new Set(
    kind === 'model'
      ? [id]
      : kind === 'provider'
        ? e.models.filter((m) => m.provider_id === id).map((m) => m.id)
        : [],
  )
  const steps = new Set(e.workflows.find((w) => w.id === id)?.steps.map((x) => x.ref) ?? [])
  // What the part is wired to: these agents' teams and these installs' kinds light up with it.
  const route = (x: string | null | undefined) => routeOf(e, x)
  const agents = e.agents.filter((a) =>
    kind === 'group'
      ? teamsOf(e, a).includes(id)
      : a.id === id ||
        route(a.router_id) === id ||
        a.tool_ids.includes(id) ||
        steps.has(a.id) ||
        a.model_ids.some((m) => models.has(m)),
  )
  const installs = e.harnesses.filter((h) =>
    kind === 'group'
      ? groupOf(h.owner_id) === id
      : h.id === id ||
        (kind === 'router' && route(h.router_id) === id) ||
        h.tool_ids.includes(id) ||
        h.model_ids.some((m) => models.has(m)),
  )
  // The router row a part passes through lights with it: its own router, or No router.
  const passes = kind === 'group' || kind === 'agent' || kind === 'harness' || models.size > 0
  for (const a of agents) {
    s.add(a.id)
    if (passes) s.add(route(a.router_id))
    if (kind === 'group') {
      path(a.model_ids)
      a.tool_ids.forEach((t) => s.add(t))
      continue
    }
    if (kind === 'router') path(a.model_ids)
    const ts = teamsOf(e, a)
    if (ts.length) ts.forEach((t) => s.add(t))
    else s.add(TEAMLESS)
  }
  for (const h of installs) {
    s.add(h.id)
    s.add(kindId(h.harness))
    s.add(groupOf(h.owner_id) ?? TEAMLESS)
    if (passes) s.add(route(h.router_id))
    if (kind === 'group' || kind === 'router') path(h.model_ids)
  }
  if (s.has(TEAMLESS) || [...s].some((x) => v.nodes.get(x)?.kind === 'group')) s.add(ORG)
  // Spend that reaches a provider reaches the bill: its provider's line into Total spend lights with it.
  if ([...s].some((x) => v.nodes.get(x)?.kind === 'provider')) s.add(BILL)
  return s
}

/** What a harness kind's row keeps lit when picked: the row and everything each of its installs touches. */
export function kindFocus(v: EstateView, key: string): Set<string> {
  const s = new Set([kindId(key)])
  for (const h of v.estate.harnesses)
    if (h.harness === key) for (const x of focusOf(v, h.id)) s.add(x)
  return s
}

/** A line is lit when it touches the selection, or joins two parts the selection keeps lit. */
export const litLine = (l: Line, sel: string, focus: ReadonlySet<string>) =>
  l.from === sel || l.to === sel || (focus.has(l.from) && focus.has(l.to))

/** A part's box on the board, in board units. */
export interface Anchor {
  x: number
  y: number
  w: number
  h: number
}
export type Pt = [number, number]

/** A row's line height: lines meet a row at the middle of its first line, not of the whole row. */
const cy = (b: Anchor) => b.y + Math.min(b.h / 2, 10)
const right = (b: Anchor) => b.x + b.w

/** A workflow's bracket up into an agent in its own column: a lane per workflow in the gutter left of it. */
const flowLane = (lane: number) => LANES.g0 - 10 - lane * 6

/**
 * What the board draws with nothing picked (Ans, 2026-11-05): only failing and red lines, none from a team (a team's
 * lines wait for a pick), and one master line into each part that needs one, since the part's own row already shows
 * what is wrong ("just connect one line to anthropic since the parent itself will surface the error"; "keep a master
 * line" for the tools):
 * - each provider: one line from its worst router (failing, then red, then the most spend), landing on its row;
 * - each tool (and each agent a workflow calls): one calls line, only when that tool is the part in error, from the
 *   first agent that calls it, else a coding harness, else a workflow. A failing agent's calls to healthy tools wait
 *   for a pick.
 */
export function restLines(lines: readonly Line[]): Line[] {
  const out = new Map<string, Line>()
  for (const l of lines) {
    if (!(l.failing || l.hot) || l.route.startsWith('team')) continue
    if (l.kind === 'calls' && !l.endFails) continue
    if (l.route !== 'router-model' || !l.via) {
      out.set(l.id, l)
      continue
    }
    const id = `router-provider:${l.from}>${l.via}`
    const was = out.get(id)
    if (was) {
      was.usd += l.usd
      was.hot ||= l.hot
      was.failing ||= l.failing
    } else out.set(id, { ...l, id, to: l.via })
  }
  // One line into each provider and each called part: the best of those landing on it.
  const master = new Map<string, Line>()
  const better = (a: Line, b: Line) =>
    a.kind === 'calls'
      ? CALLER.indexOf(a.route) < CALLER.indexOf(b.route)
      : (Number(a.failing) - Number(b.failing) || Number(a.hot) - Number(b.hot) || a.usd - b.usd) >
        0
  const merged = (l: Line) => l.kind === 'calls' || l.id.startsWith('router-provider:')
  for (const l of out.values())
    if (merged(l)) {
      const was = master.get(l.to)
      if (!was || better(l, was)) master.set(l.to, l)
    }
  return [...out.values()].filter((l) => !merged(l) || master.get(l.to) === l)
}

/** Which caller's line is a tool's master line at rest: an agent's first, then a coding harness's, then a workflow's. */
const CALLER: readonly Route[] = ['agent-tool', 'harness-tool', 'flow-tool', 'flow-agent']

/**
 * The line's points: out of its part's right edge at its row, into the next part's left edge at its row (the bill's
 * own line for a provider; a provider's row for a model the card doesn't draw). A workflow's call to an agent is a
 * bracket out of the workflow's left edge and into the agent's. Null when either end isn't drawn.
 */
export function pointsOf(l: Line, at: ReadonlyMap<string, Anchor>): Pt[] | null {
  const a = at.get(l.from)
  const b = at.get(l.to)
  if (!a) return null
  if (l.route === 'flow-agent') {
    if (!b) return null
    const x = flowLane(l.lane)
    return [
      [a.x, cy(a)],
      [x, cy(a)],
      [x, cy(b)],
      [b.x, cy(b)],
    ]
  }
  const end =
    l.route === 'provider-bill'
      ? (at.get(billLine(l.from)) ?? b)
      : (b ?? (l.via ? at.get(l.via) : undefined))
  if (!end) return null
  // A model the card doesn't draw: its provider's row, a little under the provider's own line.
  const y = cy(end) + (!b && l.route !== 'provider-bill' ? 4 : 0)
  return [
    [right(a), cy(a)],
    [end.x, y],
  ]
}

/** A line's path: a smooth S between two points (level at both ends), a rounded bracket through more. */
export function curve(pts: readonly Pt[]): string {
  if (pts.length !== 2) return roundPath(pts)
  const [[x0, y0], [x1, y1]] = pts as [Pt, Pt]
  const f = (n: number) => +n.toFixed(1)
  if (Math.abs(y1 - y0) < 0.5) return `M${f(x0)} ${f(y0)}L${f(x1)} ${f(y1)}`
  const m = (x0 + x1) / 2
  return `M${f(x0)} ${f(y0)}C${f(m)} ${f(y0)} ${f(m)} ${f(y1)} ${f(x1)} ${f(y1)}`
}

/** An SVG path through the points, its corners rounded (never past half a leg, so short legs stay square-ish). */
export function roundPath(pts: readonly Pt[], r = 6): string {
  const p = pts.filter((q, i) => i === 0 || q[0] !== pts[i - 1]?.[0] || q[1] !== pts[i - 1]?.[1])
  const first = p[0]
  if (!first) return ''
  const f = (n: number) => +n.toFixed(1)
  let d = `M${f(first[0])} ${f(first[1])}`
  for (let i = 1; i < p.length - 1; i++) {
    const [x0, y0] = p[i - 1] ?? first
    const [x1, y1] = p[i] ?? first
    const [x2, y2] = p[i + 1] ?? first
    const l1 = Math.hypot(x1 - x0, y1 - y0)
    const l2 = Math.hypot(x2 - x1, y2 - y1)
    const k = Math.min(r, l1 / 2, l2 / 2)
    if (!l1 || !l2 || !k) {
      d += `L${f(x1)} ${f(y1)}`
      continue
    }
    d += `L${f(x1 - ((x1 - x0) / l1) * k)} ${f(y1 - ((y1 - y0) / l1) * k)}`
    d += `Q${f(x1)} ${f(y1)} ${f(x1 + ((x2 - x1) / l2) * k)} ${f(y1 + ((y2 - y1) / l2) * k)}`
  }
  const last = p[p.length - 1] ?? first
  if (p.length > 1) d += `L${f(last[0])} ${f(last[1])}`
  return d
}
