/**
 * The token streams (plans/feat-village-overview.md V54, V55): the token lines cut into stretches of way, each carrying
 * the sum of every line that runs over it, so a stretch shared by many lines (the trunk out of the battery) carries
 * their flow together and draws once. A stretch's spend sets how tight its queue of coins is and how fast it runs:
 * the busiest team gets a thick, quick queue, a quiet one a trickle. Pure.
 */
import type { TokenEdge } from './model'
import type { Tile } from './streets'

export interface Stream {
  key: string
  /** From where the coins come in to where they leave, in tiles. */
  from: Tile
  to: Tile
  /** What flows along it in the range, in USD: the sum of the lines over it. */
  spend: number
  /** The kind that carries most of it (the piggy bank's line draws its coins pink). */
  kind: TokenEdge['kind']
  nodes: ReadonlySet<string>
  places: ReadonlySet<string>
}

const EPS = 1e-6
const round = (x: number) => Math.round(x * 1000) / 1000

interface Piece {
  edge: TokenEdge
  lo: number
  hi: number
  /** +1 when the line runs toward `hi`. */
  dir: 1 | -1
}

/** Every token line cut at every point another line joins or leaves, then the equal neighbours joined again. */
export function streamsOf(tokens: readonly TokenEdge[]): Stream[] {
  // Straight pieces along i (constant j) or along j (constant i), by the line they lie on; anything else stands alone.
  const lines = new Map<string, Piece[]>()
  const out: Stream[] = []
  for (const edge of tokens) {
    if (edge.spend <= 0) continue
    for (let k = 1; k < edge.path.length; k++) {
      const p = edge.path[k - 1]
      const q = edge.path[k]
      if (!p || !q || Math.hypot(q[0] - p[0], q[1] - p[1]) < EPS) continue
      const alongI = Math.abs(p[1] - q[1]) < EPS
      const alongJ = Math.abs(p[0] - q[0]) < EPS
      if (!alongI && !alongJ) {
        out.push(streamOf(`${edge.key}#${k}`, p, q, [edge]))
        continue
      }
      const key = alongI ? `j${round(p[1])}` : `i${round(p[0])}`
      const [a, b] = alongI ? [p[0], q[0]] : [p[1], q[1]]
      const list = lines.get(key) ?? []
      list.push({ edge, lo: Math.min(a, b), hi: Math.max(a, b), dir: b > a ? 1 : -1 })
      lines.set(key, list)
    }
  }
  for (const [key, pieces] of lines) {
    const at = Number(key.slice(1))
    const pt = (x: number): Tile => (key.startsWith('j') ? [x, at] : [at, x])
    const cuts = [...new Set(pieces.flatMap((p) => [round(p.lo), round(p.hi)]))].sort(
      (a, b) => a - b,
    )
    let run: { x0: number; x1: number; on: Piece[]; id: string } | null = null
    const flush = (r: NonNullable<typeof run>) => {
      const fwd = r.on.reduce((n, p) => n + p.dir * p.edge.spend, 0) >= 0
      const [a, b] = fwd ? [r.x0, r.x1] : [r.x1, r.x0]
      out.push(
        streamOf(
          `${key}:${r.x0}`,
          pt(a),
          pt(b),
          r.on.map((p) => p.edge),
        ),
      )
    }
    for (let k = 1; k < cuts.length; k++) {
      const x0 = cuts[k - 1] ?? 0
      const x1 = cuts[k] ?? 0
      const on = pieces.filter((p) => p.lo <= x0 + EPS && p.hi >= x1 - EPS)
      const id = on
        .map((p) => `${p.edge.key}${p.dir}`)
        .sort()
        .join('|')
      if (run && run.id === id && Math.abs(run.x1 - x0) < EPS) run.x1 = x1
      else {
        if (run) flush(run)
        run = on.length ? { x0, x1, on, id } : null
      }
    }
    if (run) flush(run)
  }
  return out
}

function streamOf(key: string, from: Tile, to: Tile, edges: readonly TokenEdge[]): Stream {
  const top = [...edges].filter((e) => e.kind !== 'saved').sort((a, b) => b.spend - a.spend)[0]
  return {
    key,
    from,
    to,
    spend: edges.reduce((n, e) => n + e.spend, 0),
    // Pink only where the piggy bank's line runs alone.
    kind: top?.kind ?? 'saved',
    nodes: new Set(edges.flatMap((e) => e.nodes)),
    places: new Set(edges.flatMap((e) => e.places)),
  }
}

/** Which battery a line or stream comes from: the island's (the org's) or the dirt plot's (the coding tools'). */
export const sourceOf = (x: { places: Iterable<string> }) =>
  [...x.places].includes('tank') ? 'island' : 'dirt'

/**
 * What each battery's streams are measured against: its busiest single line. A stretch carrying more than that (the
 * trunk out of the battery) packs as tight as it goes, and so does the line into the biggest user.
 */
export function busiestBySource(tokens: readonly TokenEdge[]): Record<'island' | 'dirt', number> {
  const out = { island: 0, dirt: 0 }
  for (const t of tokens) out[sourceOf(t)] = Math.max(out[sourceOf(t)], t.spend)
  return out
}

/** The tightest and loosest spacing between coins, and their speed, in px and px per second. */
export const GAP = { min: 9, max: 52 } as const
const SPEED = { min: 16, max: 54 } as const

/**
 * How a stream looks, from its spend against its battery's busiest line: the gap between coins shrinks and the queue speeds
 * up as the share grows (on a square-root scale, so a small flow still shows a trickle).
 */
export function cadenceOf(spend: number, max: number): { gap: number; speed: number } {
  const f = max > 0 ? Math.min(1, Math.sqrt(Math.max(0, spend) / max)) : 0
  return {
    gap: Math.round(GAP.max - (GAP.max - GAP.min) * f),
    speed: Math.round(SPEED.min + (SPEED.max - SPEED.min) * f),
  }
}
