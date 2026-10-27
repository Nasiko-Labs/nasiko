// The cactus character (reference/cactus-rest.png). Same approach as the
// monster: a flat vector puppet of cubic chains whose control points move
// through the part's own transform → the 2.5D turn → flex → lift. The pot is
// planted and rigid; the column flexes above its rim (squash, bend, lean,
// breathing), the arms sway about their elbows, spines ride the column.
//
// The face is the monster's face unit (eye mask, pupils, smile, teeth,
// blush, effect marks) at 0.717×, placed by a single matrix, so every
// expression, blink and gaze works unchanged on both characters.

import * as A from './cactusArt'
import {
  buildMonsterScene,
  type PropNode,
  type Scene,
  type SceneDef,
  type SceneNode,
  type SceneOptions,
} from './scene'
import {
  arcChain,
  chainD,
  crClosed,
  ellipseChain,
  flatten,
  lineSeg,
  polyArea,
  type Chain,
} from './geom'
import { bbox, clamp01, lerp, smoothstep, unionBox, type Box, type Vec } from './path'
import { mixHex } from './color'
import type { CharacterConfig, PropId } from '../config/schema'
import { FACE_CHANNELS, FX_APPEAR, zeroPose, type Pose } from '../anim/channels'
import {
  capParts,
  glassParts,
  tileParts,
  wrenchParts,
  blueprintParts,
  type PropFill,
  type PropPart,
} from './props'

const S = A.CACTUS_SCALE
const [OX, OY] = A.CACTUS_ORIGIN
const L = (p: A.P): Vec => ({ x: (p[0] - OX) * S, y: (p[1] - OY) * S })
const DEG = Math.PI / 180
const orient = (pts: Vec[]) => (polyArea(pts) < 0 ? pts.slice().reverse() : pts)
const closed = (pts: A.P[]) => crClosed(orient(pts.map(L)))

function capsule([cx, cy, len, w, ang]: A.Spine): Chain {
  // a stadium: two half circles joined by straight sides
  const r = (w / 2) * S
  const h = Math.max(0, (len - w) / 2) * S
  const c = L([cx, cy])
  const a = ang * DEG
  const ux = Math.cos(a)
  const uy = Math.sin(a)
  const end1 = { x: c.x + ux * h, y: c.y + uy * h }
  const end0 = { x: c.x - ux * h, y: c.y - uy * h }
  const cap1 = arcChain(end1, r, a - Math.PI / 2, a + Math.PI / 2)
  const cap0 = arcChain(end0, r, a + Math.PI / 2, a + (3 * Math.PI) / 2)
  return [
    ...cap1,
    ...lineSeg(cap1[cap1.length - 1], cap0[0]),
    ...cap0.slice(1),
    ...lineSeg(cap0[cap0.length - 1], cap1[0]),
  ]
}

function roundedPoly(pts: Vec[], radius: number | number[]): Chain {
  // straight edges, each corner rounded with its own radius (0 = sharp)
  const n = pts.length
  const out: Chain = []
  const cut = (a: Vec, b: Vec, d: number) => {
    const len = Math.hypot(b.x - a.x, b.y - a.y) || 1
    return { x: a.x + ((b.x - a.x) * d) / len, y: a.y + ((b.y - a.y) * d) / len }
  }
  const corners = pts.map((v, i) => {
    const prev = pts[(i - 1 + n) % n]
    const next = pts[(i + 1) % n]
    const r = Array.isArray(radius) ? radius[i] : radius
    return { a: cut(v, prev, r), v, b: cut(v, next, r) }
  })
  corners.forEach((c, i) => {
    if (i === 0) out.push(c.a)
    out.push(
      { x: lerp(c.a.x, c.v.x, 0.55), y: lerp(c.a.y, c.v.y, 0.55) },
      { x: lerp(c.b.x, c.v.x, 0.55), y: lerp(c.b.y, c.v.y, 0.55) },
      c.b,
    )
    const nxt = corners[(i + 1) % n]
    out.push(...lineSeg(c.b, nxt.a))
  })
  return out
}

const R0 = ((869 - 450) / 2) * S // column radius
/**
 * The cactus is a little springier than the monster: the same clips and
 * follow-through squash, stretch, wobble and hop it slightly further.
 */
// toned down (user, round 12) from 1.65 / 1.8 / 1.3 / 1.25 / 1.4 / 0.5 / 34
const BOUNCE = {
  squash: 1.2,
  wobble: 1.25,
  lean: 1.1,
  lift: 1.05,
  breath: 1.15,
  arm: 0.42,
  armMax: 30,
}
/** the most an arm tips in toward the column (°) and how far the excess raises it (art px → lab) */
const ARM_TUCK = 6
const ARM_RAISE = 46 * S
const BASE_Y = (1000 - OY) * S // top of the pot rim: the column flexes above it
const COL_H = (1000 - 199) * S
const TOTAL_H = (A.POT_BOTTOM - 170) * S

export const CACTUS_REST = (() => {
  const rim = A.POT_RIM
  return {
    column: closed(A.COLUMN),
    shadeEdge: A.SHADE_EDGE.map(L),
    armL: closed(A.ARM_L),
    armR: closed(A.ARM_R),
    pivotL: L(A.ARM_L_PIVOT),
    pivotR: L(A.ARM_R_PIVOT),
    tipL: L(A.ARM_L_TIP),
    tipR: L(A.ARM_R_TIP),
    spinesCol: A.SPINES_COLUMN.map(capsule),
    spinesBehind: A.SPINES_BEHIND.map(capsule),
    spinesColC: A.SPINES_COLUMN.map((sp) => L([sp[0], sp[1]])),
    spinesBehindC: A.SPINES_BEHIND.map((sp) => L([sp[0], sp[1]])),
    spinesL: A.SPINES_ARM_L.map(capsule),
    spinesR: A.SPINES_ARM_R.map(capsule),
    rim: roundedPoly(
      [L([rim.x0, rim.y0]), L([rim.x1, rim.y0]), L([rim.x1, rim.y1]), L([rim.x0, rim.y1])],
      rim.r * S,
    ),
    // only the base is rounded: the top runs on, square, under the rim
    pot: roundedPoly(A.POT_BODY.map(L), [0, 0, 42 * S, 42 * S]),
  }
})()

// the monster face unit → cactus rest coordinates (lab units)
const F = A.FACE_FROM_MONSTER
const FA = (F.scale * S) / 0.25
const FB = {
  x: S * (F.cactus[0] - OX + F.scale * (869 - F.monster[0])),
  y: S * (F.cactus[1] - OY + F.scale * (1778 - F.monster[1])),
}
const F0M: Vec = { x: (-61.25 + -3) / 2, y: -226 } // the monster face unit's own centre
const F0C: Vec = { x: FA * F0M.x + FB.x, y: FA * F0M.y + FB.y }

const FACE_IDS = [
  'eyeWhite',
  'pupilL',
  'pupilR',
  'toothL',
  'toothR',
  'mouth',
  'blushL',
  'blushR',
  'blushHatchL',
  'blushHatchR',
  'fx-marksL',
  'fx-marksR',
  'fx-sweat',
  'fx-question',
  'fx-bulb-glow',
  'fx-bulb',
  'fx-bulb-base',
  'fx-sparkle',
]

export const CACTUS_NODE_IDS: string[] = [
  'shadow',
  'spineBehind0',
  'armL',
  'armL-spine0',
  'armL-spine1',
  'armL-spine2',
  'armR',
  'armR-spine0',
  'armR-spine1',
  'armR-spine2',
  'column',
  'columnShade',
  ...A.SPINES_COLUMN.map((_, i) => `spine${i}`),
  'potBody',
  'potRim',
  ...FACE_IDS,
]

export function buildCactusScene(cfg: CharacterConfig, pose: Pose, opts: SceneOptions = {}): Scene {
  const B = cfg.body
  const C = cfg.colors
  const M = cfg.motion
  const drift = M.drift
  const Wd = B.width
  const Hd = B.height
  const nodes = new Map<string, SceneNode>()
  const defs: SceneDef[] = []
  const put = (n: SceneNode) => nodes.set(n.id, n)
  const REST = CACTUS_REST

  // ---- turn (the column is a cylinder; the head end leads) ----
  const tr = B.turnRange * DEG
  const bodyT = Math.max(-1.25, Math.min(1.25, pose.turn + B.restTurn + pose.dTurn * 0.06 * drift))
  const headT = Math.max(-1.25, Math.min(1.25, pose.headTurn + pose.headLag))
  const wHead = (y: number) => smoothstep((BASE_Y - 60 - y) / 120)
  const tA = (y: number) => tr * (bodyT + headT * wHead(y))
  const softLimit = (a: number) => {
    const a0 = 72 * DEG
    const amax = 89 * DEG
    const m = Math.abs(a)
    return m <= a0 ? a : Math.sign(a) * (a0 + (amax - a0) * Math.tanh((m - a0) / (amax - a0)))
  }
  const surface = (p: Vec): Vec => {
    const R = Math.max(R0, Math.abs(p.x) + 1e-6)
    const th = Math.asin(p.x / R)
    return {
      x: p.x + R * (Math.sin(softLimit(th + tA(p.y))) - Math.sin(th)),
      y: p.y - (pose.tip + pose.headTip) * B.tipRange * 0.35 * wHead(p.y),
    }
  }
  const outline = (p: Vec): Vec => {
    const u = Math.max(-1, Math.min(1, p.x / R0))
    return { x: p.x + B.silhouette * 0.5 * R0 * Math.sin(tA(p.y)) * (1 - u * u), y: p.y }
  }

  // ---- head tilt and column flex (the pot stays put) ----
  const tiltDeg = pose.headTilt + pose.dTilt * 2.2 * drift
  const NECK = { x: 0, y: BASE_Y - COL_H * 0.45 }
  const tilt = (p: Vec): Vec => {
    const w = smoothstep((NECK.y + 20 - p.y) / 90)
    if (Math.abs(tiltDeg) < 1e-4 || w <= 0) return p
    const a = tiltDeg * w * DEG
    const dx = p.x - NECK.x
    const dy = p.y - NECK.y
    return {
      x: NECK.x + dx * Math.cos(a) - dy * Math.sin(a),
      y: NECK.y + dx * Math.sin(a) + dy * Math.cos(a),
    }
  }
  const mo = M.morphAmount
  const syT = Math.max(0.5, 1 + (pose.sy + pose.jy) * BOUNCE.squash + pose.dBob * 0.01 * drift)
  const sxT = Math.max(0.55, (1 + pose.sx) / Math.sqrt(syT))
  const bAmt = (M.breathAmount / 100) * clamp01(pose.breath) * BOUNCE.breath
  const leanK = Math.tan(Math.max(-30, Math.min(30, pose.lean * BOUNCE.lean)) * DEG)
  const liftU = pose.lift * TOTAL_H * Hd * BOUNCE.lift
  const flex = (p: Vec): Vec => {
    let x = p.x * Wd
    let y = p.y * Hd
    const base = BASE_Y * Hd
    const h = Math.max(0, base - y)
    const hn = h / (COL_H * Hd)
    if (y < base) {
      x *=
        1 +
        bAmt * Math.exp(-(((hn - 0.4) / 0.3) ** 2)) +
        pose.bulge * mo * Math.sin(Math.PI * clamp01(hn))
      x *= lerp(1, sxT, smoothstep(h / 20))
      y = base - h * syT * (1 + bAmt * 0.2)
      x += (pose.bend + pose.jx) * BOUNCE.wobble * mo * COL_H * Hd * hn * hn + leanK * h
    }
    return { x, y: y - liftU }
  }
  const col = (p: Vec) => flex(tilt(outline(p)))
  const onCol = (p: Vec) => flex(tilt(surface(p)))
  const rigid = (p: Vec) => ({ x: p.x * Wd, y: p.y * Hd - liftU })

  // ---- pot, column, shade band ----
  const columnW = REST.column.map(col)
  const colD = chainD(columnW)
  defs.push({ kind: 'clip', id: 'cactusColumn', d: colD })
  put({ id: 'column', d: colD, fill: C.cactusBody })
  const shadePts = REST.shadeEdge.map(col)
  const far = shadePts.map((p) => ({ x: p.x + 400, y: p.y })).reverse()
  put({
    id: 'columnShade',
    d:
      'M' + [...shadePts, ...far].map((p) => `${p.x.toFixed(2)},${p.y.toFixed(2)}`).join('L') + 'Z',
    fill: C.cactusShade,
    clip: 'cactusColumn',
  })
  put({ id: 'potBody', d: chainD(REST.pot.map(rigid)), fill: C.pot })
  put({ id: 'potRim', d: chainD(REST.rim.map(rigid)), fill: C.potRim })
  // spines ride the column by their centre only: their shape and size stay
  // constant (no cylinder stretch when it twists), turning with the column
  const spineSc = (Wd + Hd) / 2
  const rideCol = (c: Chain, c0: Vec) => {
    const cw = onCol(c0)
    const up = onCol({ x: c0.x, y: c0.y - 6 })
    const a = Math.atan2(up.x - cw.x, cw.y - up.y)
    const ca = Math.cos(a) * spineSc
    const sa = Math.sin(a) * spineSc
    return c.map((p) => ({
      x: cw.x + (p.x - c0.x) * ca - (p.y - c0.y) * sa,
      y: cw.y + (p.x - c0.x) * sa + (p.y - c0.y) * ca,
    }))
  }
  REST.spinesCol.forEach((c, i) =>
    put({ id: `spine${i}`, d: chainD(rideCol(c, REST.spinesColC[i])), fill: C.spine }),
  )
  REST.spinesBehind.forEach((c, i) =>
    put({ id: `spineBehind${i}`, d: chainD(rideCol(c, REST.spinesBehindC[i])), fill: C.spine }),
  )
  if (B.shadow > 0.01) {
    const k = 1 / (1 + Math.max(0, pose.lift) * 3.2)
    put({
      id: 'shadow',
      d: chainD(ellipseChain(0, 1.5, 110 * Wd * k, 8 * k)),
      fill: C.pot,
      opacity: B.shadow * 0.28 * (0.35 + 0.65 * k),
    })
  }

  // ---- arms: sway about the elbow (they stay close to the body) ----
  const hands: Record<'L' | 'R', Vec> = { L: { x: 0, y: 0 }, R: { x: 0, y: 0 } }
  for (const side of ['L', 'R'] as const) {
    const out =
      side === 'L'
        ? pose.armL + pose.sArmL - pose.dArm * 3 * drift
        : pose.armR + pose.sArmR + pose.dArm * 3 * drift
    const sgn = side === 'L' ? -1 : 1
    const raw = Math.max(-BOUNCE.armMax, Math.min(BOUNCE.armMax, out * BOUNCE.arm)) * sgn
    // The arms sit behind the column, so tipping a forearm inward hides it
    // (and swings the flat base out the other side). Inward tips stop at a
    // few degrees; the rest of a "hand to the cheek" becomes a small raise.
    const tuck = Math.max(0, -raw * sgn)
    const tuckSoft = ARM_TUCK * Math.tanh(tuck / ARM_TUCK)
    const ang = tuck > 0 ? -tuckSoft * sgn : raw
    const raise = ARM_RAISE * Math.tanh(Math.max(0, tuck - tuckSoft) / 20)
    const pv = side === 'L' ? REST.pivotL : REST.pivotR
    const dx = R0 * (Math.sin(softLimit((sgn * Math.PI) / 2 + tA(pv.y))) - sgn) * 0.35
    const T = (p: Vec) => {
      const a = ang * DEG
      const qx = p.x - pv.x
      const qy = p.y - pv.y
      return flex(
        tilt({
          x: pv.x + qx * Math.cos(a) - qy * Math.sin(a) + dx,
          y: pv.y + qx * Math.sin(a) + qy * Math.cos(a) - raise,
        }),
      )
    }
    put({
      id: `arm${side}`,
      d: chainD((side === 'L' ? REST.armL : REST.armR).map(T)),
      fill: C.cactusArm,
    })
    ;(side === 'L' ? REST.spinesL : REST.spinesR).forEach((c, i) =>
      put({ id: `arm${side}-spine${i}`, d: chainD(c.map(T)), fill: C.spine }),
    )
    hands[side] = T(side === 'L' ? REST.tipL : REST.tipR)
  }

  // ---- the face: the monster's face unit, placed by one matrix ----
  const fp = zeroPose()
  for (const c of [...FACE_CHANNELS, ...FX_APPEAR, 'blink', 'shutL', 'shutR', 'fxPhase'] as const)
    fp[c] = pose[c]
  const faceScene = buildMonsterScene(
    { ...cfg, character: 'monster', body: { ...cfg.body, width: 1, height: 1 } },
    fp,
    { propsEnabled: { cap: false, blueprint: false, wrench: false, glass: false, tiles: false } },
  )
  const fs = surface(F0C)
  const slide = 12 * Math.tanh((fs.x - F0C.x) / 12)
  const Fw = flex(tilt({ x: F0C.x + slide, y: fs.y }))
  const ta = tiltDeg * DEG
  const sc = (Wd + Hd) / 2
  const ma = FA * sc * Math.cos(ta)
  const mb = FA * sc * Math.sin(ta)
  const tx = Fw.x - (ma * F0M.x - mb * F0M.y)
  const ty = Fw.y - (mb * F0M.x + ma * F0M.y)
  const matrix = `matrix(${ma.toFixed(5)},${mb.toFixed(5)},${(-mb).toFixed(5)},${ma.toFixed(5)},${tx.toFixed(3)},${ty.toFixed(3)})`
  const fm = (p: Vec): Vec => ({ x: ma * p.x - mb * p.y + tx, y: mb * p.x + ma * p.y + ty })
  for (const id of FACE_IDS) {
    const n = faceScene.nodes.find((x) => x.id === id)
    if (n && n.d) put({ ...n, clip: undefined, transform: matrix })
  }
  // the question mark floats clear of the column, up past the right arm, and larger
  const q = cfg.effects.question ? clamp01(pose.fxQuestion) : 0
  if (q > 0.01) {
    const ph = ((pose.fxPhase % 1) + 1) % 1
    const o0 = flex(tilt(L([1010, 250])))
    const o = { x: o0.x, y: o0.y + Math.sin(ph * Math.PI * 2) * 2 }
    const k = 12.5 * cfg.effects.size * (0.5 + 0.5 * q)
    const f = (x: number, y: number) => `${(o.x + x * k).toFixed(2)},${(o.y + y * k).toFixed(2)}`
    const d = `M${f(-1, -0.6)}C${f(-1, -1.9)} ${f(1, -1.9)} ${f(1, -0.6)}C${f(1, 0.3)} ${f(0, 0.1)} ${f(0, 0.9)}M${f(0, 1.75)}l0,0.01`
    put({
      id: 'fx-question',
      d,
      fill: 'none',
      opacity: Math.min(1, q * 1.5),
      stroke: {
        color: C.effects,
        width: 4.6 * cfg.effects.size,
        opacity: 1,
        cap: 'round',
        join: 'round',
      },
    })
  }
  // the idea bulb floats well clear of the column's top (rising a little as it appears)
  const bulb = cfg.effects.bulb ? clamp01(pose.fxBulb) : 0
  if (bulb > 0.01) {
    const ph = ((pose.fxPhase % 1) + 1) % 1
    const o = flex(tilt(L([640, 92 + (1 - bulb) * 40])))
    const k = 10 * cfg.effects.size * (0.4 + 0.6 * bulb)
    const show = Math.min(1, bulb * 1.5)
    put({
      id: 'fx-bulb-glow',
      d: chainD(ellipseChain(o.x, o.y, k * 1.75, k * 1.75)),
      fill: '#FFF2B3',
      opacity: 0.55 * bulb * (0.75 + 0.25 * Math.sin(ph * Math.PI * 4)),
    })
    put({ id: 'fx-bulb', d: chainD(ellipseChain(o.x, o.y, k, k)), fill: '#FFD84D', opacity: show })
    put({
      id: 'fx-bulb-base',
      d: `M${(o.x - k * 0.45).toFixed(2)},${(o.y + k * 0.85).toFixed(2)}h${(k * 0.9).toFixed(2)}v${(k * 0.55).toFixed(2)}h${(-k * 0.9).toFixed(2)}Z`,
      fill: '#A2A2A2',
      opacity: show,
    })
  }

  // ---- tools: held at the top of the right arm ----
  const P = cfg.props
  const propFill: Record<PropFill, string> = {
    accent: C.accessory,
    accentSoft: C.accessoryLight,
    accentDeep: C.accessoryDeep,
    accentMid: C.accessoryMid,
    card: mixHex(C.accessory, C.accessoryMid, 0.12),
    metal: C.metal,
    body: C.cactusBody,
  }
  const props: PropNode[] = []
  const propAnchors: Partial<Record<PropId, Vec>> = {}
  const hand = hands.R
  const scale0 = 0.62 * sc
  const bob = pose.propBob * 2.5 * P.motionAmp
  const sway = pose.propSway * P.motionAmp
  const add = (
    id: PropId,
    appear: number,
    build: () => { parts: PropPart[]; width: number; height: number },
    at: Vec,
    deg: number,
    size: number,
    grip: Vec = { x: 0, y: 0 },
  ) => {
    if (opts.propsEnabled && opts.propsEnabled[id] === false) return
    if (!P[id].enabled) return
    const k = scale0 * size * P[id].size
    const a0 = (deg + P[id].rotation) * DEG
    const c = {
      x: at.x + P[id].offsetX * 60 - (grip.x * Math.cos(a0) - grip.y * Math.sin(a0)) * k,
      y: at.y + P[id].offsetY * 60 - (grip.x * Math.sin(a0) + grip.y * Math.cos(a0)) * k,
    }
    propAnchors[id] = c
    const a = Math.max(0, appear)
    if (a < 0.005) return
    const built = build()
    const hw = (built.width * k) / 2
    const hh = (built.height * k) / 2
    props.push({
      id,
      transform: `translate(${(at.x + P[id].offsetX * 60).toFixed(2)},${(at.y + P[id].offsetY * 60).toFixed(2)}) rotate(${(deg + P[id].rotation).toFixed(2)}) scale(${(k * a).toFixed(4)}) translate(${(-grip.x).toFixed(2)},${(-grip.y).toFixed(2)})`,
      opacity: Math.min(1, a * 1.6),
      parts: built.parts.map((p, i) => ({
        id: `${id}-${i}`,
        d: p.d,
        fill: propFill[p.fill],
        evenodd: p.evenodd,
      })),
      center: c,
      box: { x0: c.x - hw, y0: c.y - hh, x1: c.x + hw, y1: c.y + hh },
    })
  }
  add('wrench', pose.wrench, wrenchParts, hand, 8 + pose.wrenchTurn * 26 * P.motionAmp, 1.2, {
    x: 0,
    y: 40,
  })
  add('glass', pose.glass, glassParts, hand, 30 + pose.glassDrift * 6 * P.motionAmp, 1.35, {
    x: 37,
    y: 37,
  })
  add(
    'tiles',
    pose.tiles,
    () => tileParts(pose.tilesPhase),
    { x: hand.x + 6, y: hand.y - 52 + sway * 2 },
    sway * 3,
    1.5,
  )
  add(
    'blueprint',
    pose.blueprint,
    blueprintParts,
    { x: hand.x + 4, y: hand.y - 60 + bob * 0.5 },
    -6 + sway * 3,
    1,
  )
  const top = col({ x: -10, y: (199 - OY) * S })
  add(
    'cap',
    pose.cap,
    capParts,
    { x: top.x + sway * 2, y: top.y - 4 + bob * 0.6 },
    tiltDeg * 0.8,
    1,
  )

  // ---- assemble ----
  const hull = flatten(columnW, 4)
  let bounds: Box = bbox(hull)
  for (const id of ['armL', 'armR', 'potRim', 'potBody']) {
    const n = nodes.get(id)
    if (n?.d) {
      const nums = (n.d.match(/-?\d+(\.\d+)?/g) ?? []).map(Number)
      const pts: Vec[] = []
      for (let i = 0; i + 1 < nums.length; i += 2) pts.push({ x: nums[i], y: nums[i + 1] })
      bounds = unionBox(bounds, bbox(pts))
    }
  }
  for (const p of props) bounds = unionBox(bounds, p.box)
  const fa = faceScene.anchors
  const eyeMask = faceScene.eyeMask.map(fm)
  const ordered = CACTUS_NODE_IDS.map((id) => nodes.get(id) ?? { id, d: '', fill: 'none' })
  return {
    nodes: ordered,
    defs,
    props,
    bounds,
    hull,
    eyeMask,
    dims: { W: R0 * 2 * Wd, H: COL_H * Hd },
    blinkable: faceScene.blinkable,
    armZ: { L: -1, R: -1 },
    anchors: {
      face: fm(fa.face),
      eyeL: fm(fa.eyeL),
      eyeR: fm(fa.eyeR),
      pupilL: fm(fa.pupilL),
      pupilR: fm(fa.pupilR),
      faceBox: bbox(eyeMask),
      props: propAnchors,
      handL: hands.L,
      handR: hands.R,
      joint: flex(NECK),
      feet: { L: rigid(L([451, A.POT_BOTTOM])), R: rigid(L([868, A.POT_BOTTOM])) },
      bottom: 0,
    },
  }
}

export function cactusRestFrame(cfg: CharacterConfig): {
  cx: number
  cy: number
  w: number
  h: number
} {
  const top = (170 - OY) * S * cfg.body.height - 6
  const bottom = 4
  return { cx: 0, cy: (top + bottom) / 2, w: (1125 - 190) * S * cfg.body.width, h: bottom - top }
}
