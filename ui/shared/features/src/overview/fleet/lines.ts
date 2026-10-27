/**
 * The Map view's token lines (pure, tested in lines.test.ts): every connection the village draws as a coin trail,
 * drawn on the board between the parts it joins, one line per pair so each agent, workflow and tool has its own.
 *
 * - Spend (blue, as wide as what it carries; red where spend is going up: a team over pace, or a cost problem on
 *   the parts carrying more than HOT_SHARE of the line's spend): org → team → agent → its LLM router → each model the
 *   router sends it to, or agent → model when it has no router; team → coding harness → model, and each harness kind
 *   straight to the bill (its spend is billed with no router between); the month's budget → each provider (what it
 *   pays them, the bill's amounts).
 * - Calls (thin, lighter): workflow → each step's agent or tool, agent → tool, coding harness → tool.
 *
 * `focusOf` is what a selection keeps lit: the estate's own `related` rule plus the path a part's spend takes (an
 * agent's teams, a router's models, a team's agents and where they spend). `pointsOf` routes a line through the board's
 * gutters and streets (layout.ts) from the parts' measured boxes. The lines are drawn under the cards, so a line's last
 * leg into its part disappears at the card's edge: the edge is where it lands. Only the org's lines to its teams run
 * inside a card (the Teams card's left margin) and are drawn over it (`OVER`).
 */
import { problemsOf, related } from '../estate/model'
import type { EstateView } from '../estate/useEstateView'
import { LANES } from './layout'
import { modelWeights, SHOWN, teamsOf, type Board } from './model'

/**
 * The anchors that aren't estate nodes: the org's budget, the people in no team, a harness kind's column, and the two
 * TokenOps cards lines end on (the bill and the month's budget).
 */
export const ORG = 'org'
export const TEAMLESS = 'team:none'
export const BILL = 'card:bill'
export const BUDGET = 'card:budget'
export const kindId = (kind: string) => `hk:${kind}`

type Route =
  | 'org-team'
  | 'team-agent'
  | 'team-harness'
  | 'flow-agent'
  | 'flow-tool'
  | 'agent-tool'
  | 'agent-router'
  | 'agent-model'
  | 'router-model'
  | 'harness-model'
  | 'harness-tool'
  | 'harness-bill'
  | 'budget-provider'

/** The routes drawn over the cards: they run inside one. */
export const OVER: ReadonlySet<Route> = new Set(['org-team'])

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
}

export function buildLines(v: EstateView, board: Board): Line[] {
  const e = v.estate
  const costly = (id: string) =>
    problemsOf(v.nodes, v.problems, id).some((p) => p.kind === 'cost' || p.also === 'cost')
  const providerOf = (m: string) => e.models.find((x) => x.id === m)?.provider_id ?? null
  const groupOf = (person: string | null) =>
    person ? (e.people.find((p) => p.id === person)?.group_id ?? null) : null

  const teamLane = new Map(board.teams.map((t, i) => [t.id ?? TEAMLESS, i]))
  const agents = new Map(board.agents.map((a) => [a.id, a]))
  const toolLane = new Map(board.tools.map((t, i) => [t.id, i]))
  const routerLane = new Map(board.routers.map((r, i) => [r.id, i]))
  const models = new Set(board.providers.flatMap((p) => p.models.map((m) => m.id)))
  const slotOf = new Map(board.providers.flatMap((p) => p.models.map((m, i) => [m.id, i] as const)))
  const providers = new Set(board.providers.map((p) => p.id))
  const shownModel = (m: string) => models.has(m) || providers.has(providerOf(m) ?? '')

  const out = new Map<string, Line>()
  // What each line's costly parts carry; a line is red once that passes HOT_SHARE of it (see the end).
  const heat = new Map<string, number>()
  const add = (l: Omit<Line, 'id' | 'via' | 'slot'> & { via?: string | null }) => {
    const id = `${l.route}:${l.from}>${l.to}`
    if (l.hot) heat.set(id, (heat.get(id) ?? 0) + l.usd)
    const was = out.get(id)
    if (was) {
      was.usd += l.usd
      was.hot ||= l.hot
    } else out.set(id, { ...l, id, via: l.via ?? null, slot: slotOf.get(l.to) ?? SHOWN.models })
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

  for (const t of board.teams)
    add({
      route: 'org-team',
      kind: 'spend',
      from: ORG,
      to: t.id ?? TEAMLESS,
      lane: 0,
      usd: t.usd,
      hot: (t.pace ?? 0) > 1,
    })

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
    const r = e.routers.find((x) => x.id === a.router_id)
    if (r && !r.direct) {
      if (routerLane.has(r.id))
        add({
          route: 'agent-router',
          kind: 'spend',
          from: a.id,
          to: r.id,
          lane: routerLane.get(r.id) ?? 0,
          usd: a.spend_usd,
          hot: row.costly,
        })
    } else toModels('agent-model', a.id, 0, a.model_ids, a.spend_usd, row.costly)
  }

  board.routers.forEach((r, i) => {
    for (const a of e.agents)
      if (a.router_id === r.id)
        toModels(
          'router-model',
          r.id,
          i,
          a.model_ids,
          a.spend_usd,
          agents.get(a.id)?.costly ?? costly(a.id),
        )
  })

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
      toModels('harness-model', kind, k, h.model_ids, h.spend_usd ?? 0, hot)
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
  board.harnesses.forEach((c, k) => {
    const installs = e.harnesses.filter((h) => h.harness === c.key)
    add({
      route: 'harness-bill',
      kind: 'spend',
      from: kindId(c.key),
      to: BILL,
      lane: k,
      usd: c.spend,
      hot: installs.some((h) => costly(h.id)),
    })
  })
  board.providers.forEach((p, i) =>
    add({
      route: 'budget-provider',
      kind: 'spend',
      from: BUDGET,
      to: p.id,
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
  const agents = e.agents.filter((a) =>
    kind === 'group'
      ? teamsOf(e, a).includes(id)
      : a.id === id ||
        a.router_id === id ||
        a.tool_ids.includes(id) ||
        steps.has(a.id) ||
        a.model_ids.some((m) => models.has(m)),
  )
  const installs = e.harnesses.filter((h) =>
    kind === 'group'
      ? groupOf(h.owner_id) === id
      : h.id === id || h.tool_ids.includes(id) || h.model_ids.some((m) => models.has(m)),
  )
  for (const a of agents) {
    s.add(a.id)
    if (kind === 'group') {
      if (a.router_id) s.add(a.router_id)
      path(a.model_ids)
      a.tool_ids.forEach((t) => s.add(t))
      continue
    }
    if (kind === 'router') path(a.model_ids)
    if (models.size && a.router_id) s.add(a.router_id)
    const ts = teamsOf(e, a)
    if (ts.length) ts.forEach((t) => s.add(t))
    else s.add(TEAMLESS)
  }
  for (const h of installs) {
    s.add(h.id)
    s.add(kindId(h.harness))
    s.add(groupOf(h.owner_id) ?? TEAMLESS)
    if (kind === 'group') path(h.model_ids)
  }
  if (s.has(TEAMLESS) || [...s].some((x) => v.nodes.get(x)?.kind === 'group')) s.add(ORG)
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
const mid = (b: Anchor) => b.x + b.w / 2
const bottom = (b: Anchor) => b.y + b.h

/**
 * The streets' lanes (y). In s1, under the top row: the agents' lines to their routers, a lane each; the direct lines
 * (no router); the calls to tools. In s3, over the providers: the routers' lines to their models, a lane each, and the
 * direct lines under them. In s4, over the harnesses: the budget's payments to the providers, a lane each, then the
 * harnesses' lines up to their models and tools.
 */
const S1 = {
  router: (lane: number) => LANES.s1 + 3 + lane * 2,
  direct: LANES.s1 + 12,
  calls: LANES.s1 + 16,
}
const S3 = {
  router: (lane: number) => LANES.s3 + 3 + lane * 3,
  direct: LANES.s3 + 16,
}
const S4 = {
  pay: (lane: number) => LANES.s4 + 3 + lane * 2.5,
  model: (lane: number) => LANES.s4 + 13 + lane * 1.5,
  calls: LANES.s4 + 17,
}
/** The gutter lanes (x): g0 a lane per team; g1 the routers', the direct lane, then the tools' and workflows'. */
const G = {
  team: (lane: number) => LANES.g0 + 10 + lane * 7,
  router: (lane: number) => LANES.g1 + 2 + lane * 2,
  direct: LANES.g1 + 10,
  tool: (lane: number) => LANES.g1 + 12 + (lane % 8) * 0.75,
  flow: (lane: number) => LANES.g1 + 18 - lane,
  /** The band left of the routers, where the direct lines go down. */
  band: LANES.band + 10,
  /** Between the bill and the budget: a payment's lane up into the budget. */
  pay: (lane: number) => LANES.g3 + 4 + lane * 3,
}

/**
 * The line's points, routed through the gaps (layout.ts LANES): out of a part's edge, along its gutter lane, across a
 * street when it has to pass a row, and into the other part. Null when either end isn't drawn.
 */
export function pointsOf(l: Line, at: ReadonlyMap<string, Anchor>): Pt[] | null {
  const a = at.get(l.from)
  const b = at.get(l.to)
  // A model's line drops into its own row, in its provider's column (the column when the model isn't drawn).
  const modelX = () => {
    const end = b ?? (l.via ? at.get(l.via) : undefined)
    return end ? { end, x: end.x + 6 + l.slot * 7 } : null
  }
  const fromAbove = (x: number, y: number): Pt[] | null => {
    const m = modelX()
    return m
      ? [
          [x, y],
          [m.x, y],
          [m.x, m.end.y + 2],
        ]
      : null
  }
  const tail = (pts: Pt[], end: Pt[] | null) => (end ? [...pts, ...end] : null)
  // A tool is entered from above, out of s1's calls lane.
  const toTool = (pts: Pt[]): Pt[] | null =>
    b ? [...pts, [mid(b), S1.calls], [mid(b), b.y]] : null
  // A harness kind's port on the harnesses' top edge, apart from the other kinds'.
  const hport = (x: Anchor) => x.x + 24 + l.lane * 10
  switch (l.route) {
    case 'org-team': {
      if (!a || !b) return null
      const x = b.x - 9
      return [
        [x, a.y + a.h],
        [x, cy(b)],
        [b.x - 2, cy(b)],
      ]
    }
    case 'team-agent': {
      if (!a || !b) return null
      const x = G.team(l.lane)
      return [
        [right(a), cy(a)],
        [x, cy(a)],
        [x, cy(b)],
        [b.x, cy(b)],
      ]
    }
    case 'team-harness': {
      if (!a || !b) return null
      const x = G.team(l.lane)
      const y = b.y + 6 + l.lane * 4
      return [
        [right(a), cy(a)],
        [x, cy(a)],
        [x, y],
        [LANES.band, y],
      ]
    }
    case 'flow-agent': {
      if (!a || !b) return null
      const x = G.flow(l.lane)
      return [
        [a.x, cy(a)],
        [x, cy(a)],
        [x, cy(b)],
        [right(b), cy(b)],
      ]
    }
    case 'flow-tool': {
      if (!a) return null
      const x = G.flow(l.lane)
      return toTool([
        [a.x, cy(a)],
        [x, cy(a)],
        [x, S1.calls],
      ])
    }
    case 'agent-tool': {
      if (!a) return null
      const x = G.tool(l.lane)
      return toTool([
        [right(a), cy(a)],
        [x, cy(a)],
        [x, S1.calls],
      ])
    }
    case 'agent-router': {
      if (!a || !b) return null
      const x = G.router(l.lane)
      const y = S1.router(l.lane)
      return [
        [right(a), cy(a)],
        [x, cy(a)],
        [x, y],
        [mid(b), y],
        [mid(b), b.y],
      ]
    }
    case 'agent-model': {
      if (!a) return null
      return tail(
        [
          [right(a), cy(a)],
          [G.direct, cy(a)],
          [G.direct, S1.direct],
          [G.band, S1.direct],
          [G.band, S3.direct],
        ],
        fromAbove(G.band, S3.direct),
      )
    }
    case 'router-model': {
      if (!a) return null
      const y = S3.router(l.lane)
      return tail(
        [
          [mid(a), bottom(a)],
          [mid(a), y],
        ],
        fromAbove(mid(a), y),
      )
    }
    case 'harness-model': {
      if (!a) return null
      const m = modelX()
      if (!m) return null
      const px = hport(a)
      const y = S4.model(l.lane)
      return [
        [px, a.y],
        [px, y],
        [m.x, y],
        [m.x, bottom(m.end)],
      ]
    }
    case 'harness-tool': {
      if (!a || !b) return null
      const px = hport(a)
      return [
        [px, a.y],
        [px, S4.calls],
        [mid(b), S4.calls],
        [mid(b), bottom(b)],
      ]
    }
    case 'harness-bill': {
      if (!a || !b) return null
      const y = a.y + 12 + l.lane * 5
      const x = b.x + 28 + l.lane * 12
      return [
        [right(a), y],
        [x, y],
        [x, bottom(b)],
      ]
    }
    case 'budget-provider': {
      // From the budget's left edge, down the gap beside the bill, along s4 and up into the provider's column.
      if (!a || !b) return null
      const x = G.pay(l.lane)
      const y = S4.pay(l.lane)
      const into = a.y + 44 + l.lane * 10
      return [
        [a.x, into],
        [x, into],
        [x, y],
        [mid(b), y],
        [mid(b), b.y + 2],
      ]
    }
  }
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
