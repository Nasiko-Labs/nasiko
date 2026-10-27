/**
 * The map's geometry (pure): where the six cards sit, where every square goes inside them, and the path of each flow
 * between cards, in map units (the SVG's viewBox). Cards stack in three columns — people; workflows over agents;
 * routers and models over tools and coding harnesses — and every column ends on one baseline, so the bottoms line up.
 * Long lists are capped with a "+N" row: the map is a picture of the estate, the pages own the full lists.
 */
import { pace } from '../estate/model'
import type { Estate, PanelId } from '../estate/types'

export interface Box {
  x: number
  y: number
  w: number
  h: number
}
export type Point = readonly [number, number]

export interface GroupRow {
  /** Null: people in no team (or every person, when the estate has no teams). */
  id: string | null
  name: string
  y: number
  members: { id: string; x: number; y: number }[]
  more: { count: number; x: number; y: number } | null
  /** The team's budget pace bar, when it has a budget. */
  bar: { y: number; pace: number } | null
}
export interface WorkflowRow {
  id: string
  y: number
  steps: { x: number; index: number }[]
}
export interface ProviderCol {
  id: string
  x: number
  y: number
  models: { id: string; x: number; y: number }[]
}
export interface MapLayout {
  width: number
  height: number
  panels: Record<PanelId, Box>
  people: { rows: GroupRow[] }
  workflows: { rows: WorkflowRow[]; more: number; moreY: number }
  models: {
    routers: { id: string; x: number; y: number }[]
    dividerY: number
    providers: ProviderCol[]
    shareY: number
  }
  agents: {
    rows: { id: string; y: number }[]
    tilesX: number
    more: number
    moreY: number
    labelY: number
  }
  tools: { items: { id: string; x: number; y: number }[]; more: number; moreY: number }
  harnesses: { rows: { id: string; y: number }[]; more: number; moreY: number }
  /** `stack`: the axis a flow's badges line up on — along its line where there is room, across it in a narrow gap. */
  flows: { id: string; d: string; mid: Point; stack: 'x' | 'y'; direct: boolean }[]
  /** Each node's centre (cause-chain arrows run between them). */
  pos: Map<string, Point>
}

export const HEAD = 52
const GAP = 56
const PAD = 20

export const SIZE = {
  group: 24,
  person: 20,
  workflow: 22,
  step: 20,
  router: 32,
  provider: 28,
  model: 22,
  agent: 22,
  tool: 34,
  harness: 32,
  tile: { w: 9, h: 18, step: 13 },
  session: { w: 11, h: 9, stepX: 14, stepY: 12, perRow: 15 },
} as const

export const CAPS = {
  workflows: 5,
  agents: 12,
  tools: 12,
  harnesses: 5,
  people: 40,
} as const

const COLS = { people: { x: 0, w: 250 }, mid: { x: 290, w: 400 }, right: { x: 730, w: 480 } }

/** Coding harnesses worth a row: the ones with problems first, then by spend. */
export function harnessOrder(e: Estate, flagged: ReadonlySet<string>) {
  return [...e.harnesses].sort(
    (a, b) =>
      Number(flagged.has(b.id)) - Number(flagged.has(a.id)) ||
      (b.spend_usd ?? 0) - (a.spend_usd ?? 0) ||
      a.name.localeCompare(b.name),
  )
}

export function layoutMap(e: Estate, flagged: ReadonlySet<string> = new Set()): MapLayout {
  const pos = new Map<string, Point>()
  const put = (id: string, x: number, y: number, size: number) =>
    pos.set(id, [x + size / 2, y + size / 2])

  // People: a line per team (its name, then its budget pace on the right), its members on the line under it.
  const p = COLS.people
  const rows: GroupRow[] = []
  let y = HEAD + 16
  const teams: { id: string | null; name: string; members: string[] }[] = e.groups.length
    ? [
        ...e.groups.map((g) => ({
          id: g.id,
          name: g.name,
          members: e.people.filter((x) => x.group_id === g.id).map((x) => x.id),
        })),
        {
          id: null,
          name: '',
          members: e.people
            .filter((x) => !x.group_id || !e.groups.some((g) => g.id === x.group_id))
            .map((x) => x.id),
        },
      ].filter((t) => t.id !== null || t.members.length)
    : [{ id: null, name: '', members: e.people.map((x) => x.id) }]
  const flat = !e.groups.length
  for (const t of teams) {
    const cols = Math.floor((p.w - PAD * 2 - (flat ? 0 : 34)) / 28)
    const cap = flat ? CAPS.people : cols
    const shown = t.members.slice(0, t.members.length > cap ? cap - 1 : cap)
    const top = y
    if (t.id) put(t.id, p.x + PAD, y, SIZE.group)
    const mx0 = p.x + PAD + (flat ? 0 : 34)
    const my0 = flat ? y : y + 30
    const members = shown.map((id, i) => {
      const mx = mx0 + (i % cols) * 28
      const my = my0 + Math.floor(i / cols) * 28
      put(id, mx, my, SIZE.person)
      return { id, x: mx, y: my }
    })
    const more =
      t.members.length > shown.length
        ? {
            count: t.members.length - shown.length,
            x: mx0 + (shown.length % cols) * 28,
            y: my0 + Math.floor(shown.length / cols) * 28,
          }
        : null
    const slots = shown.length + (more ? 1 : 0)
    y = my0 + Math.max(1, Math.ceil(slots / cols)) * 28
    const g = t.id ? e.groups.find((x) => x.id === t.id) : undefined
    const pc = g ? pace(g.budget) : null
    rows.push({
      id: t.id,
      name: t.name,
      y: top,
      members,
      more,
      bar: pc !== null ? { y: top, pace: pc } : null,
    })
    y += 14
  }
  const peopleBottom = y

  // Workflows: a row each, its steps as a chain.
  const m = COLS.mid
  const wfShown = e.workflows.slice(0, CAPS.workflows)
  const wfRows: WorkflowRow[] = wfShown.map((w, i) => {
    const ry = HEAD + 16 + i * 36
    put(w.id, m.x + PAD, ry, SIZE.workflow)
    return {
      id: w.id,
      y: ry,
      steps: w.steps.slice(0, 4).map((_, k) => ({ x: m.x + 206 + k * 44, index: k })),
    }
  })
  const wfMore = e.workflows.length - wfShown.length
  const wfMoreY = HEAD + 16 + wfShown.length * 36
  const workflowsH = Math.max(HEAD + 60, wfMoreY + (wfMore ? 28 : 0) + 6)

  // Routers and models: routers on top, then each provider's models in a column, then the provider share bar.
  const r = COLS.right
  const routerStep = Math.floor((r.w - PAD * 2) / 4)
  const routers = e.routers.map((x, i) => {
    const rx = r.x + PAD + (i % 4) * routerStep
    const ry = HEAD + 30 + Math.floor(i / 4) * 46
    put(x.id, rx, ry, SIZE.router)
    return { id: x.id, x: rx, y: ry }
  })
  const routerBottom = HEAD + 30 + Math.max(1, Math.ceil(e.routers.length / 4)) * 46
  const dividerY = routerBottom + 6
  const provTop = dividerY + 34
  const provCols = Math.min(6, Math.max(4, e.providers.length))
  const provStep = Math.floor((r.w - PAD * 2) / provCols)
  let provBottom = provTop
  const providers: ProviderCol[] = e.providers.slice(0, provCols).map((pr, i) => {
    const px = r.x + PAD + (i % provCols) * provStep
    const py = provTop
    put(pr.id, px, py, SIZE.provider)
    const ms = e.models.filter((x) => x.provider_id === pr.id)
    const models = ms.map((md, k) => {
      const my = py + 40 + k * 30
      put(md.id, px, my, SIZE.model)
      return { id: md.id, x: px, y: my }
    })
    provBottom = Math.max(provBottom, py + 40 + ms.length * 30)
    return { id: pr.id, x: px, y: py, models }
  })
  const shareY = provBottom + 8
  const modelsH = Math.max(HEAD + 100, shareY + 26)

  // Agents: a row each, then 14 day tiles.
  const agentsY = workflowsH + GAP
  const agShown = e.agents.slice(0, CAPS.agents)
  const tilesX = m.x + m.w - PAD - SIZE.tile.step * 14 + (SIZE.tile.step - SIZE.tile.w)
  const agRows = agShown.map((a, i) => {
    const ry = agentsY + HEAD + 12 + i * 28
    put(a.id, m.x + PAD, ry, SIZE.agent)
    return { id: a.id, y: ry }
  })
  const agMore = e.agents.length - agShown.length
  const agMoreY = agentsY + HEAD + 12 + agShown.length * 28
  const labelY = agMoreY + (agMore ? 28 : 0) + 12

  // Tools: two columns of marks.
  const toolsY = modelsH + GAP
  const tw = 140
  const toolShown = e.tools.slice(0, CAPS.tools)
  const tools = toolShown.map((t, i) => {
    const tx = r.x + 24 + (i % 2) * 52
    const ty = toolsY + HEAD + 14 + Math.floor(i / 2) * 46
    put(t.id, tx, ty, SIZE.tool)
    return { id: t.id, x: tx, y: ty }
  })
  const toolMore = e.tools.length - toolShown.length
  const toolMoreY = toolsY + HEAD + 14 + Math.ceil(toolShown.length / 2) * 46

  // Coding harnesses: a row each with its last sessions.
  const hx = r.x + tw + 36
  const hw = r.x + r.w - hx
  const hShown = harnessOrder(e, flagged).slice(0, CAPS.harnesses)
  const hRows = hShown.map((h, i) => {
    const ry = toolsY + HEAD + 12 + i * 54
    put(h.id, hx + PAD, ry, SIZE.harness)
    return { id: h.id, y: ry }
  })
  const hMore = e.harnesses.length - hShown.length
  const hMoreY = toolsY + HEAD + 12 + hShown.length * 54

  const bottom = Math.max(
    peopleBottom,
    labelY + 22,
    toolMoreY + (toolMore ? 28 : 0) + 6,
    hMoreY + (hMore ? 28 : 0) + 6,
    toolsY + HEAD + 120,
  )
  const panels: Record<PanelId, Box> = {
    people: { x: p.x, y: 0, w: p.w, h: bottom },
    workflows: { x: m.x, y: 0, w: m.w, h: workflowsH },
    agents: { x: m.x, y: agentsY, w: m.w, h: bottom - agentsY },
    models: { x: r.x, y: 0, w: r.w, h: modelsH },
    tools: { x: r.x, y: toolsY, w: tw, h: bottom - toolsY },
    harnesses: { x: hx, y: toolsY, w: hw, h: bottom - toolsY },
  }

  // Flows between cards: orthogonal lines that leave and enter card edges.
  const P = panels
  const below = bottom + 26
  const geo: Record<string, { pts: Point[] }> = {
    'people-workflows': {
      pts: [
        [P.people.x + P.people.w, P.workflows.y + P.workflows.h / 2],
        [P.workflows.x, P.workflows.y + P.workflows.h / 2],
      ],
    },
    'people-agents': {
      pts: [
        [P.people.x + P.people.w, P.agents.y + 80],
        [P.agents.x, P.agents.y + 80],
      ],
    },
    'workflows-agents': {
      pts: [
        [P.workflows.x + 120, P.workflows.y + P.workflows.h],
        [P.workflows.x + 120, P.agents.y],
      ],
    },
    // Straight across when the agents' top sits beside the models card, else an elbow up to it.
    'agents-models':
      P.agents.y + 34 <= P.models.y + P.models.h - 24
        ? {
            pts: [
              [P.agents.x + P.agents.w, P.agents.y + 34],
              [P.models.x, P.agents.y + 34],
            ],
          }
        : {
            pts: [
              [P.agents.x + P.agents.w, P.agents.y + 34],
              [(P.agents.x + P.agents.w + P.models.x) / 2, P.agents.y + 34],
              [(P.agents.x + P.agents.w + P.models.x) / 2, P.models.y + P.models.h - 40],
              [P.models.x, P.models.y + P.models.h - 40],
            ],
          },
    'agents-tools': {
      pts: [
        [P.agents.x + P.agents.w, P.tools.y + 70],
        [P.tools.x, P.tools.y + 70],
      ],
    },
    'harnesses-models': {
      pts: [
        [P.harnesses.x + P.harnesses.w / 2, P.harnesses.y],
        [P.harnesses.x + P.harnesses.w / 2, P.models.y + P.models.h],
      ],
    },
    'harnesses-tools': {
      pts: [
        [P.harnesses.x, P.tools.y + 130],
        [P.tools.x + P.tools.w, P.tools.y + 130],
      ],
    },
    'people-harnesses': {
      pts: [
        [P.people.x + P.people.w / 2, bottom],
        [P.people.x + P.people.w / 2, below],
        [P.harnesses.x + P.harnesses.w / 2, below],
        [P.harnesses.x + P.harnesses.w / 2, bottom],
      ],
    },
  }
  const flows = e.flows.flatMap((f) => {
    const g = geo[f.id] ?? geo[`${f.from}-${f.to}`]
    if (!g) return []
    const d = `M${g.pts.map(([x, py]) => `${x} ${py}`).join(' L')}`
    return [{ id: f.id, d, mid: midpoint(g.pts), stack: badgeAxis(g.pts), direct: f.direct }]
  })

  return {
    width: r.x + r.w,
    height: below + 14,
    panels,
    people: { rows },
    workflows: { rows: wfRows, more: wfMore, moreY: wfMoreY },
    models: { routers, dividerY, providers, shareY },
    agents: { rows: agRows, tilesX, more: agMore, moreY: agMoreY, labelY },
    tools: { items: tools, more: toolMore, moreY: toolMoreY },
    harnesses: { rows: hRows, more: hMore, moreY: hMoreY },
    flows,
    pos,
  }
}

/** Badges line up along the segment holding the midpoint when it is long enough for three, else across it. */
export function badgeAxis(pts: readonly Point[]): 'x' | 'y' {
  const segs = pts.slice(1).map((b, i) => {
    const a = pts[i] ?? b
    return { a, b, len: Math.hypot(b[0] - a[0], b[1] - a[1]) }
  })
  let left = segs.reduce((n, s) => n + s.len, 0) / 2
  for (const s of segs) {
    if (left <= s.len) {
      const horizontal = Math.abs(s.b[0] - s.a[0]) >= Math.abs(s.b[1] - s.a[1])
      const roomy = s.len >= 80
      return horizontal === roomy ? 'x' : 'y'
    }
    left -= s.len
  }
  return 'x'
}

/** The point halfway along a polyline (where a flow's badges sit). */
export function midpoint(pts: readonly Point[]): Point {
  const segs = pts.slice(1).map((b, i) => {
    const a = pts[i] ?? b
    return { a, b, len: Math.hypot(b[0] - a[0], b[1] - a[1]) }
  })
  const total = segs.reduce((n, s) => n + s.len, 0)
  let left = total / 2
  for (const s of segs) {
    if (left <= s.len && s.len > 0) {
      const t = left / s.len
      return [s.a[0] + (s.b[0] - s.a[0]) * t, s.a[1] + (s.b[1] - s.a[1]) * t]
    }
    left -= s.len
  }
  return pts[0] ?? [0, 0]
}

/** Text cut to fit a width at the map's label size (an estimate: the map never measures text). */
export function clip(text: string, px: number, size = 13): string {
  const max = Math.max(3, Math.floor(px / (size * 0.56)))
  return text.length > max ? `${text.slice(0, max - 1)}…` : text
}
