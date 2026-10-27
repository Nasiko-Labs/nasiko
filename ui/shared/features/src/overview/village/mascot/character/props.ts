// Flat task props, drawn in their own local space (centred, ~unit = px at
// default body size). Each returns filled parts only: no strokes, no hands.

import { lerp, smoothstep, type Vec } from './path'

export type PropFill =
  'accent' | 'accentSoft' | 'accentDeep' | 'accentMid' | 'card' | 'metal' | 'body'
export interface PropPart {
  d: string
  fill: PropFill
  evenodd?: boolean
}

const f = (n: number) => (Math.round(n * 100) / 100).toString()

function roundRect(x: number, y: number, w: number, h: number, r: number): string {
  const rr = Math.min(r, w / 2, h / 2)
  return (
    `M${f(x + rr)},${f(y)}H${f(x + w - rr)}A${f(rr)},${f(rr)} 0 0 1 ${f(x + w)},${f(y + rr)}` +
    `V${f(y + h - rr)}A${f(rr)},${f(rr)} 0 0 1 ${f(x + w - rr)},${f(y + h)}` +
    `H${f(x + rr)}A${f(rr)},${f(rr)} 0 0 1 ${f(x)},${f(y + h - rr)}` +
    `V${f(y + rr)}A${f(rr)},${f(rr)} 0 0 1 ${f(x + rr)},${f(y)}Z`
  )
}

function circle(cx: number, cy: number, r: number, ccw = false): string {
  const s = ccw ? 0 : 1
  return `M${f(cx + r)},${f(cy)}A${f(r)},${f(r)} 0 1 ${s} ${f(cx - r)},${f(cy)}A${f(r)},${f(r)} 0 1 ${s} ${f(cx + r)},${f(cy)}Z`
}

/** Engineer's hard hat. Origin = centre of the brim's underside. */
export function capParts(): { parts: PropPart[]; height: number; width: number } {
  const dome = `M-34,-8C-34,-30 -18,-44 0,-44C18,-44 34,-30 34,-8Z`
  const brim = roundRect(-46, -10, 92, 10, 5)
  const ridge = roundRect(-5, -46, 10, 30, 5)
  return {
    parts: [
      { d: dome, fill: 'accent' },
      { d: brim, fill: 'accent' },
      { d: ridge, fill: 'accentSoft' },
    ],
    height: 46,
    width: 92,
  }
}

// Blueprint card, measured in its own (−5.6° rotated) frame from
// reference/figma-blueprint.png: a 154 × 217 px card (→ 94 × 132 local units,
// ÷1.64) with a big block, a light side strip, a wide block and two bars
// (x, y, w, h, radius). The layout is fixed, as in the art.
const CARD_W = 94
const CARD_H = 132
const BLOCKS: [number, number, number, number, number, PropFill][] = [
  [6, 7.3, 57.3, 51.2, 5, 'accent'],
  [68.9, 7.3, 16.5, 51.2, 4, 'accentSoft'],
  [6.7, 64.6, 78.7, 24.4, 5, 'accentDeep'],
  [6.7, 93.9, 78.7, 11, 3.7, 'accentDeep'],
  [6.7, 109.8, 78.7, 11, 3.7, 'accentDeep'],
]

/** Floating blueprint card. */
export function blueprintParts(): { parts: PropPart[]; width: number; height: number } {
  const parts: PropPart[] = [
    { d: roundRect(-CARD_W / 2, -CARD_H / 2, CARD_W, CARD_H, 8), fill: 'card' },
  ]
  for (const [x, y, w, h, r, fill] of BLOCKS)
    parts.push({ d: roundRect(x - CARD_W / 2, y - CARD_H / 2, w, h, r), fill })
  return { parts, width: CARD_W, height: CARD_H }
}

/**
 * Open-end wrench, head up, measured from reference/figma-wrench.png (318 px
 * long → ×0.609 local): ring R 20.7 with a 12.8 hole, handle 26.8 wide, head
 * R 30.5 with a 27.4 wide jaw slot. Origin = centre of the wrench's length.
 */
export function wrenchParts(): { parts: PropPart[]; width: number; height: number } {
  const ry = 76 // ring centre (down)
  const rr = 20.7
  const hole = 12.8
  const hy = -66.4 // head centre (up)
  const R = 30.5
  const sw = 13.7 // slot half-width
  const sb = hy + 2.4 // slot bottom
  const hw = 13.4 // handle half-width
  const fil = 9 // fillet where the handle flares into the head
  const slotTop = hy - Math.sqrt(R * R - sw * sw)
  // the handle meets the head circle; a short flare softens the joint
  const joinY = hy + Math.sqrt(R * R - (hw + fil * 0.6) ** 2)
  const ringJoin = ry - Math.sqrt(rr * rr - hw * hw)
  const d =
    `M${f(sw)},${f(slotTop)}A${R},${R} 0 0 1 ${f(hw + fil * 0.6)},${f(joinY)}` +
    `Q${f(hw)},${f(joinY + fil * 0.8)} ${f(hw)},${f(joinY + fil * 1.6)}L${f(hw)},${f(ringJoin)}` +
    `A${rr},${rr} 0 1 1 ${f(-hw)},${f(ringJoin)}L${f(-hw)},${f(joinY + fil * 1.6)}` +
    `Q${f(-hw)},${f(joinY + fil * 0.8)} ${f(-hw - fil * 0.6)},${f(joinY)}A${R},${R} 0 0 1 ${f(-sw)},${f(slotTop)}` +
    `L${f(-sw)},${f(sb - 4)}Q${f(-sw)},${f(sb)} ${f(-sw + 4)},${f(sb)}L${f(sw - 4)},${f(sb)}Q${f(sw)},${f(sb)} ${f(sw)},${f(sb - 4)}Z` +
    circle(0, ry, hole, true)
  return { parts: [{ d, fill: 'metal', evenodd: true }], width: 2 * R, height: ry + rr - (hy - R) }
}

/** Magnifying glass: open ring (the eye shows through) plus a separate handle. */
export function glassParts(): { parts: PropPart[]; lensR: number; width: number; height: number } {
  const ring = circle(0, 0, 24) + circle(0, 0, 18, true)
  // handle runs down-right at 45°, starting at the ring's outer edge
  const k = Math.SQRT1_2
  const hw = 5.5
  const a = { x: 23 * k - hw * k, y: 23 * k + hw * k }
  const b = { x: 50 * k - hw * k, y: 50 * k + hw * k }
  const c = { x: 50 * k + hw * k, y: 50 * k - hw * k }
  const dd = { x: 23 * k + hw * k, y: 23 * k - hw * k }
  const handle = `M${f(a.x)},${f(a.y)}L${f(b.x)},${f(b.y)}A${hw},${hw} 0 0 0 ${f(c.x)},${f(c.y)}L${f(dd.x)},${f(dd.y)}Z`
  return {
    parts: [
      { d: ring, fill: 'accent', evenodd: true },
      { d: handle, fill: 'accent' },
    ],
    lensR: 24,
    width: 90,
    height: 90,
  }
}

export const TILE_CYCLE = 12

// 2×2 sliding arrangement with one empty cell (bottom left, as in
// reference/figma-tiles.png: 50 px tiles on a 69 px pitch → 30.5 / 42 local).
// Cells run clockwise.
const TP = 21
const CELLS: Vec[] = [
  { x: -TP, y: -TP },
  { x: TP, y: -TP },
  { x: TP, y: TP },
  { x: -TP, y: TP },
]

/**
 * Three tiles in a 2×2 grid; `phase` slides one tile into the empty cell per
 * unit. Tile identities return to the start after 12 moves (TILE_CYCLE).
 */
export function tilePositions(phase: number): { pos: Vec[]; moving: number } {
  const p = ((phase % TILE_CYCLE) + TILE_CYCLE) % TILE_CYCLE
  const step = Math.floor(p)
  const k = smoothstep((p - step - 0.35) / 0.65)
  // initial: tiles in cells 0,1,2, empty cell 3
  const occ = [0, 1, 2]
  let empty = 3
  for (let s = 0; s < step; s++) {
    const from = (empty + 3) % 4
    const t = occ.indexOf(from)
    occ[t] = empty
    empty = from
  }
  const from = (empty + 3) % 4
  const moving = occ.indexOf(from)
  const pos = occ.map((cell, i) => {
    if (i !== moving) return CELLS[cell]
    const a = CELLS[cell]
    const b = CELLS[empty]
    const lift = Math.sin(k * Math.PI) * 6
    return { x: lerp(a.x, b.x, k) + lift * 0.4, y: lerp(a.y, b.y, k) - lift }
  })
  return { pos, moving }
}

export function tileParts(phase: number): {
  parts: PropPart[]
  width: number
  height: number
  focus: Vec
} {
  const { pos, moving } = tilePositions(phase)
  // Figma: top left mid blue, top right blue, bottom right light blue
  const fills: PropFill[] = ['accentMid', 'accent', 'accentSoft']
  const lift = 5
  const parts = pos.map((p, i) => ({
    d: roundRect(p.x - 15.25, p.y - 15.25, 30.5, 30.5, 6.5),
    fill: fills[i],
  }))
  return { parts, width: 2 * TP + 30.5 + lift, height: 2 * TP + 30.5 + lift, focus: pos[moving] }
}
