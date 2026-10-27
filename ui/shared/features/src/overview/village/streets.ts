/**
 * The village's ways (plans/feat-village-overview.md V21, V24, V50): one small graph that every route runs on, laid
 * where the reference design runs its coin trails and sand strips. Tokens take it from the battery's spout east to the
 * routers and south down the sand strips to the teams; agents take it from their bay up the parking's aisle to the team
 * or project that uses them. Past a router a route is its last few points: out from under the router, down to the
 * providers' lane in front of the tents, along it to the tent's door. Pure.
 */
import type { Spot } from './zones'

export type Tile = [number, number]

const NODES = {
  /** Just past the battery's spout, at its front-left, on the line to the crossroads. */
  tank: [9.9, 8.85],
  /** The lane between the project plots' two rows, by the town hall. */
  yards: [0.7, 1.95],
  y1: [4.55, 1.95],
  a1: [8.85, 1.7],
  a2: [8.85, 5.75],
  a3: [8.85, 8.85],
  /** The column west of the routers. */
  g1: [10.6, 1.7],
  g2: [10.6, 4.3],
  g3: [10.6, 5.75],
  /** The direct lane's far end, past the routers: the right-hand project plots' door is on it. */
  g4: [12.35, 5.75],
  /** The sand strips: B along the town hall's front, A down between the teams, C across to the parking. */
  b0: [0.8, 8.85],
  b1: [4.65, 8.85],
  c0: [0.8, 13.25],
  c1: [4.65, 13.25],
  c2: [8.85, 13.25],
  a4: [4.65, 17.4],
  /** The parking's aisle, between its two rows of bays. */
  d0: [8.85, 15.7],
  d1: [17.9, 15.7],
} as const satisfies Record<string, Tile>
type Node = keyof typeof NODES

const EDGES: readonly (readonly [Node, Node])[] = [
  ['tank', 'a3'],
  ['yards', 'y1'],
  ['y1', 'a1'],
  ['a1', 'g1'],
  ['g1', 'g2'],
  ['g2', 'g3'],
  ['g3', 'g4'],
  ['a1', 'a2'],
  ['a2', 'g3'],
  ['a2', 'a3'],
  ['a3', 'b1'],
  ['b0', 'b1'],
  ['b1', 'c1'],
  ['c1', 'a4'],
  ['c0', 'c1'],
  ['c1', 'c2'],
  ['a3', 'c2'],
  ['c2', 'd0'],
  ['d0', 'd1'],
]

/** The ways as segments. */
export const STREETS: readonly (readonly [Tile, Tile])[] = EDGES.map(([a, b]) => [
  [...NODES[a]] as Tile,
  [...NODES[b]] as Tile,
])

/** The battery's spout, where every token starts. */
export const SPOUT: Tile = [...NODES.tank]
/** The column the routers stand east of: a routed token turns east here, at its router's passage. */
export const GATE_STREET_I = 10.6
/** Where a token comes out from under a router, and the line it drops down to the providers' lane on. */
export const GATE_OUT_I = 12.35
/** The providers' lane, along i in front of the tents. */
export const LANE_J = 3.25
/** The direct lane: unrouted spend passes the routers here, under none of them. */
export const DIRECT_J = 5.75

const dist = (a: Tile, b: Tile) => Math.hypot(a[0] - b[0], a[1] - b[1])

/** The nearest point on a segment, and how far along it (0..1). */
function project(p: Tile, a: Tile, b: Tile): { at: Tile; t: number } {
  const dx = b[0] - a[0]
  const dy = b[1] - a[1]
  const len = dx * dx + dy * dy || 1
  const t = Math.max(0, Math.min(1, ((p[0] - a[0]) * dx + (p[1] - a[1]) * dy) / len))
  return { at: [a[0] + dx * t, a[1] + dy * t], t }
}

/** Where a point joins the streets: the nearest point on the nearest segment. */
function attach(p: Tile) {
  let best = { edge: 0, at: [0, 0] as Tile, d: Infinity }
  STREETS.forEach(([a, b], k) => {
    const { at } = project(p, a, b)
    const d = dist(p, at)
    if (d < best.d) best = { edge: k, at, d }
  })
  return best
}

/** The shortest street route from one point to another (both ends included), turning only at corners. */
export function route(from: Tile, to: Tile): Tile[] {
  const a = attach(from)
  const b = attach(to)
  const names = Object.keys(NODES) as Node[]
  const pos = new Map<string, Tile>(names.map((n) => [n, [...NODES[n]] as Tile]))
  const adj = new Map<string, { to: string; w: number }[]>()
  const link = (x: string, y: string) => {
    const w = dist(pos.get(x) ?? [0, 0], pos.get(y) ?? [0, 0])
    adj.set(x, [...(adj.get(x) ?? []), { to: y, w }])
    adj.set(y, [...(adj.get(y) ?? []), { to: x, w }])
  }
  pos.set('A', a.at)
  pos.set('B', b.at)
  EDGES.forEach(([x, y], k) => {
    const onA = k === a.edge
    const onB = k === b.edge
    if (!onA && !onB) return link(x, y)
    if (onA && onB) {
      link('A', 'B')
      link(x, 'A')
      link(x, 'B')
      link(y, 'A')
      link(y, 'B')
      return
    }
    const mid = onA ? 'A' : 'B'
    link(x, mid)
    link(mid, y)
  })
  // Dijkstra over a couple of dozen nodes.
  const best = new Map<string, number>([['A', 0]])
  const prev = new Map<string, string>()
  const open = new Set<string>(['A'])
  while (open.size) {
    let cur = ''
    let cd = Infinity
    for (const n of open) {
      const d = best.get(n) ?? Infinity
      if (d < cd) {
        cd = d
        cur = n
      }
    }
    open.delete(cur)
    if (cur === 'B') break
    for (const e of adj.get(cur) ?? []) {
      const nd = cd + e.w
      if (nd < (best.get(e.to) ?? Infinity)) {
        best.set(e.to, nd)
        prev.set(e.to, cur)
        open.add(e.to)
      }
    }
  }
  const path: Tile[] = []
  for (let n: string | undefined = 'B'; n; n = prev.get(n)) path.unshift(pos.get(n) ?? [0, 0])
  // The ends exactly as asked; drop the street points that repeat them (an end that already sits on its street).
  const inner = path.filter((p) => dist(p, from) > 0.01 && dist(p, to) > 0.01)
  return [from, ...inner.filter((p, k) => k === 0 || dist(p, inner[k - 1] ?? p) > 0.01), to]
}

/** A plot's door on the way beside it: the middle of its east side. */
export const doorOf = (p: Spot & { w: number; d: number }): Tile => [p.i + p.w + 0.4, p.j + p.d / 2]

/**
 * A project plot's door: the left-hand plots open west onto the column from the battery, the right-hand ones north
 * onto the direct lane.
 */
export const yardDoor = (p: Spot & { w: number; d: number }): Tile =>
  p.i < NODES.y1[0]
    ? // the plots by the town hall open onto the lane between their rows
      [p.i + p.w / 2, p.j < NODES.yards[1] ? p.j + p.d + 0.08 : p.j - 0.08]
    : p.i < NODES.g3[0]
      ? [p.i - 0.3, p.j + p.d / 2]
      : [p.i + p.w / 2, p.j - 0.3]

/** A router's passage: the line along i the tokens take under it. */
export const passOf = (g: Spot & { d: number }) => g.j + g.d / 2

/** A tent's door, on the providers' lane side. */
export const tentDoor = (m: Spot & { w: number; d: number }): Tile => [
  m.i + m.w * 0.42,
  m.j + m.d + 0.15,
]

/** From a router's passage (or the direct lane) at `j` to a provider's tent: out, down to the lane, along, in. */
export const toTent = (j: number, m: Spot & { w: number; d: number }): Tile[] => {
  const door = tentDoor(m)
  return [[GATE_OUT_I, j], [GATE_OUT_I, LANE_J], [door[0], LANE_J], door]
}
