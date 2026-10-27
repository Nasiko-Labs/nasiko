// Derives the full drawable scene from the saved design + a composed pose.
// Pure: no DOM. Used by the live renderer, previews and SVG export.
//
// The monster is a flat vector puppet (character/art.ts). Every part is a
// chain of cubic Béziers; a frame moves CONTROL POINTS through:
//   1. the part's own transform (arm swing, ear flick, leaf sway, foot lift),
//   2. the 2.5D turn: surface features ride a virtual curved surface fitted
//      to the body (x = c + R·sin(θ + turn)), so they slide, compress toward
//      the far edge and tuck behind it; appendages shift by depth parallax;
//      the outline bulges toward the turn; a darker band grows on the far side,
//   3. head tip and head tilt (weighted, so the neck bends smoothly),
//   4. body flex: breathing, bulge, volume-preserving squash about the
//      ground (feet stay planted), bend and lean (shear above the feet),
//   5. lift (hops).
// Node ids are stable, so renderers keep one element per id.

import * as ART from './art'
import {
  arcChain,
  chainD,
  crClosed,
  cubicAt,
  cubicSub,
  cubicTAtX,
  ellipseChain,
  flatten,
  lineSeg,
  polyArea,
  polysOverlap,
  taperedStroke,
  type Chain,
} from './geom'
import { bbox, clamp01, lerp, polyPath, smoothstep, unionBox, type Box, type Vec } from './path'
import { mixHex } from './color'
import type { CharacterConfig, EffectId, PropId } from '../config/schema'
import { GLYPH_IDS, type Pose } from '../anim/channels'
import {
  blueprintParts,
  capParts,
  glassParts,
  tileParts,
  wrenchParts,
  type PropFill,
  type PropPart,
} from './props'
import { buildCactusScene, cactusRestFrame } from './cactus'

export interface SceneNode {
  id: string
  d: string
  fill: string // 'none' for pure strokes
  evenodd?: boolean
  opacity?: number
  stroke?: {
    color: string
    width: number
    opacity: number
    cap?: 'round' | 'butt'
    join?: 'round' | 'miter'
  }
  /** logical id of a gradient in Scene.defs (overrides fill) */
  grad?: string
  /** logical id of a blur filter in Scene.defs */
  filter?: string
  /** logical id of a clip path in Scene.defs */
  clip?: string
  /** SVG transform (tools held in the hand are drawn as nodes, behind the fingers) */
  transform?: string
}

export interface GradStop {
  offset: number
  color: string
  opacity: number
}

/**
 * Paint servers, filters and clip paths. Ids are logical: every renderer
 * prefixes them per instance, so previews never collide and an SVG export
 * stays self-contained. The monster only uses clip paths (flat fills).
 */
export type SceneDef =
  | {
      kind: 'linear'
      id: string
      x1: number
      y1: number
      x2: number
      y2: number
      stops: GradStop[]
    }
  | { kind: 'radial'; id: string; cx: number; cy: number; r: number; stops: GradStop[] }
  | { kind: 'blur'; id: string; std: number }
  | { kind: 'clip'; id: string; d: string; evenodd?: boolean; clipBy?: string }

export interface PropNode {
  id: PropId
  transform: string
  opacity: number
  parts: SceneNode[]
  center: Vec
  box: Box
}

export interface Scene {
  nodes: SceneNode[]
  defs: SceneDef[]
  props: PropNode[]
  bounds: Box // includes props
  /** body silhouette polygon (hover hit-testing) */
  hull: Vec[]
  /** eye mask polygon (overlay) */
  eyeMask: Vec[]
  dims: { W: number; H: number }
  /** how much of the face is an open, blinkable eye (0..1) */
  blinkable: number
  /** arms' depth (+ = in front of the body) */
  armZ: { L: number; R: number }
  anchors: {
    face: Vec
    eyeL: Vec
    eyeR: Vec
    pupilL: Vec
    pupilR: Vec
    faceBox: Box
    props: Partial<Record<PropId, Vec>>
    handL: Vec
    handR: Vec
    joint: Vec
    feet: { L: Vec; R: Vec }
    bottom: number
  }
}

export interface SceneOptions {
  /** Which props may render at all (accessory toggles). */
  propsEnabled?: Partial<Record<PropId, boolean>>
}

// ---- rest data (lab units: origin between the feet on the ground, y down) ----

const S = ART.ART_SCALE
const [OX, OY] = ART.ART_ORIGIN
const L = (p: ART.P): Vec => ({ x: (p[0] - OX) * S, y: (p[1] - OY) * S })
const DEG = Math.PI / 180
/** every closed outline runs the same way round, so clip-path unions are true unions */
const orient = (pts: Vec[]) => (polyArea(pts) < 0 ? pts.slice().reverse() : pts)
const closed = (pts: ART.P[]) => crClosed(orient(pts.map(L)))
const mirror = (p: Vec): Vec => ({
  x: -p.x + ART.ARM_L_SHIFT[0] * S,
  y: p.y + ART.ARM_L_SHIFT[1] * S,
})

function toeChain([cx, apex, hw, h]: [number, number, number, number]): Chain {
  // a pointed arch, continued well below the foot (the foot clips it)
  const pts: ART.P[] = [
    [cx - hw, apex + h],
    [cx - hw * 0.78, apex + h * 0.42],
    [cx - hw * 0.34, apex + h * 0.07],
    [cx, apex],
    [cx + hw * 0.34, apex + h * 0.07],
    [cx + hw * 0.78, apex + h * 0.42],
    [cx + hw, apex + h],
    [cx + hw * 1.02, apex + h + 60],
    [cx - hw * 1.02, apex + h + 60],
  ]
  return closed(pts)
}

const ell = (s: [number, number, number, number], rot = 0) =>
  ellipseChain((s[0] - OX) * S, (s[1] - OY) * S, s[2] * S, s[3] * S, rot)

export const REST = (() => {
  const body = closed(ART.BODY)
  const armR = closed(ART.ARM_R)
  const armL = closed(ART.ARM_L)
  const crease = ART.CREASE.p.map(L) as [Vec, Vec, Vec, Vec]
  // the body's half-width and centre at every height: the surface the features ride on
  const poly = flatten(body, 10)
  const top = Math.min(...poly.map((p) => p.y))
  const rows: { c: number; R: number }[] = []
  const y0 = Math.floor(top)
  for (let y = y0; y <= 0; y++) {
    let lo = Infinity
    let hi = -Infinity
    for (let i = 0; i < poly.length; i++) {
      const a = poly[i]
      const b = poly[(i + 1) % poly.length]
      if (a.y > y !== b.y > y) {
        const x = a.x + ((y - a.y) / (b.y - a.y)) * (b.x - a.x)
        lo = Math.min(lo, x)
        hi = Math.max(hi, x)
      }
    }
    rows.push(Number.isFinite(lo) ? { c: (lo + hi) / 2, R: (hi - lo) / 2 } : { c: 0, R: 20 })
  }
  // smooth the profile so features glide (no kinks where the outline has corners)
  const sm = rows.map((_, i) => {
    let c = 0
    let R = 0
    let n = 0
    for (let k = -8; k <= 8; k++) {
      const r = rows[Math.min(rows.length - 1, Math.max(0, i + k))]
      c += r.c
      R += r.R
      n++
    }
    return { c: c / n, R: Math.max(24, R / n) }
  })
  const ears = ART.BODY_EARS.map(([x, y, w]) => ({ p: L([x, y]), w }))
  const earsOrient = polyArea(ears.map((e) => e.p)) < 0 ? ears.slice().reverse() : ears
  return {
    body,
    bodyEars: earsOrient,
    top,
    profile: { y0, rows: sm },
    earL: closed(ART.EAR_L),
    earR: closed(ART.EAR_R),
    earPivotL: L(ART.EAR_PIVOT_L),
    earPivotR: L(ART.EAR_PIVOT_R),
    footL: closed(ART.FOOT_L),
    footR: closed(ART.FOOT_R),
    footPivotL: L(ART.FOOT_PIVOT_L),
    footPivotR: L(ART.FOOT_PIVOT_R),
    toesL: ART.TOES_L.map(toeChain),
    toesR: ART.TOES_R.map(toeChain),
    armR,
    armL,
    armPivotR: L(ART.ARM_PIVOT_R),
    armPivotL: mirror(L(ART.ARM_PIVOT_R)),
    handR: L(ART.HAND_R),
    handL: mirror(L(ART.HAND_R)),
    creaseR: crease,
    creaseL: crease.map(mirror) as [Vec, Vec, Vec, Vec],
    belly: ART.BELLY.p.map(L) as [Vec, Vec, Vec, Vec],
    spotsL: ART.SPOTS_L.map((s, i) => ell(s, [-12, 8, 20][i])),
    spotsR: ART.SPOTS_R.map((s, i) => ell(s, [10, -15, 6, 0][i])),
    face: closed(ART.FACE),
    toothL: closed(ART.TOOTH_L),
    toothR: closed(ART.TOOTH_R),
    eyeCL: L(ART.EYE.cL),
    eyeCR: L(ART.EYE.cR),
    eyeR: ART.EYE.r * S,
    pupilL: L(ART.PUPIL.l),
    pupilR: L(ART.PUPIL.r),
    mouth: ART.MOUTH.p.map(L) as [Vec, Vec, Vec, Vec],
    leafBig: closed(ART.LEAF_BIG),
    leafSmall: closed(ART.LEAF_SMALL),
    leafPivotBig: L(ART.LEAF_PIVOT_BIG),
    leafPivotSmall: L(ART.LEAF_PIVOT_SMALL),
  }
})()

/** Surface profile at height y (rest, unscaled). */
function profile(y: number) {
  const P = REST.profile
  const f = Math.min(P.rows.length - 1.001, Math.max(0, y - P.y0))
  const i = Math.floor(f)
  const t = f - i
  const a = P.rows[i]
  const b = P.rows[i + 1]
  return { c: a.c + (b.c - a.c) * t, R: a.R + (b.R - a.R) * t }
}

/** Rest-pose heights (lab units, y up is negative) the turn and tilt weights use. */
const Y_FACE = -232 // eyes and mouth: fully "head"
const Y_CHEST = -112 // below: fully "body"
const HEAD_CY = -246
const HEAD_RV = 138
const NECK: Vec = { x: 0, y: -150 }
const BODY_H = -REST.top // ~314

/** a soft limit: identity up to a0, then eases toward amax (never past it, no kink) */
function softLimit(a: number, a0 = 72 * DEG, amax = 89 * DEG) {
  const m = Math.abs(a)
  if (m <= a0) return a
  return Math.sign(a) * (a0 + (amax - a0) * Math.tanh((m - a0) / (amax - a0)))
}

// ---- pupil glyphs: a stroke through A → M → B (round caps) --------------------

interface Glyph {
  ax: number
  ay: number
  mx: number
  my: number
  bx: number
  by: number
  w: number
  round: number // 1 = smooth curve through M, 0 = sharp corner at M
}
const g = (
  ax: number,
  ay: number,
  mx: number,
  my: number,
  bx: number,
  by: number,
  w: number,
  round = 1,
): Glyph => ({ ax, ay, mx, my, bx, by, w, round })
/** the art's pupil: a vertical pill 47 × 91 px (left eye shapes; the right eye mirrors them) */
export const PILL = g(0, -5.5, 0, 0, 0, 5.5, 11.75)
export const GLYPHS: Record<(typeof GLYPH_IDS)[number], Glyph> = {
  happy: g(-10, 4, 0, -6, 10, 4, 4.6), // ^ (the waving frame)
  closed: g(-9.6, 2, 0, -2.6, 9.6, 2, 4.2), // ︵ (the content frame: a soft arch)
  squeeze: g(-7.6, -6.6, 7, 0, -7.6, 6.6, 4.6, 0), // > < (the excited frame)
  dot: g(0, -0.6, 0, 0, 0, 0.6, 15), // a big round pupil
  wide: g(0, -8, 0, 0, 0, 8, 12.5), // taller pill
  line: g(-7.5, 0, 0, 0.8, 7.5, 0, 3.6), // — (sleepy)
  half: g(-2.6, 2.4, 0, 2.6, 2.6, 2.4, 10), // a low, squat oval: heavy-lidded, without lids
  slant: g(2, -5.2, 0, 0, -2, 5.2, 11.75), // tops lean inward: soft, worried
  scowl: g(-2, -5.2, 0, 0, 2, 5.2, 11.75), // tops lean outward: cross (sheet-only grumpy)
}
const BLINK = g(-8, 0.8, 0, -0.8, 8, 0.8, 4)

function mixGlyph(a: Glyph, b: Glyph, t: number): Glyph {
  const o = {} as Glyph
  for (const k of Object.keys(a) as (keyof Glyph)[]) o[k] = a[k] + (b[k] - a[k]) * t
  return o
}

function glyphFor(pose: Pose, side: 'L' | 'R', scale: number): Glyph {
  let w = 0
  for (const id of GLYPH_IDS) w += Math.max(0, pose[`g_${id}`])
  const k = w > 1 ? 1 / w : 1
  let out = { ...PILL }
  for (const id of GLYPH_IDS) {
    const wi = Math.max(0, pose[`g_${id}`]) * k
    if (wi <= 0) continue
    const G = GLYPHS[id]
    for (const key of Object.keys(out) as (keyof Glyph)[]) out[key] += (G[key] - PILL[key]) * wi
  }
  const shut = clamp01(side === 'L' ? pose.shutL : pose.shutR)
  if (shut > 0) out = mixGlyph(out, GLYPHS.closed, shut)
  const b = clamp01(pose.blink)
  if (b > 0) out = mixGlyph(out, BLINK, b)
  const s = scale
  if (side === 'R') {
    out.ax = -out.ax
    out.mx = -out.mx
    out.bx = -out.bx
  }
  return {
    ...out,
    ax: out.ax * s,
    ay: out.ay * s,
    mx: out.mx * s,
    my: out.my * s,
    bx: out.bx * s,
    by: out.by * s,
    w: out.w * s,
  }
}

/** sample points along a glyph's centre line (relative to its centre) */
function glyphSamples(G: Glyph): Vec[] {
  const A = { x: G.ax, y: G.ay }
  const M = { x: G.mx, y: G.my }
  const B = { x: G.bx, y: G.by }
  const [q1, q2] = glyphCtrl(G)
  const out: Vec[] = []
  const quad = (p0: Vec, c: Vec, p1: Vec, t: number) => ({
    x: (1 - t) * (1 - t) * p0.x + 2 * (1 - t) * t * c.x + t * t * p1.x,
    y: (1 - t) * (1 - t) * p0.y + 2 * (1 - t) * t * c.y + t * t * p1.y,
  })
  for (let i = 0; i <= 4; i++) out.push(quad(A, q1, M, i / 4))
  for (let i = 1; i <= 4; i++) out.push(quad(M, q2, B, i / 4))
  return out
}

function glyphCtrl(G: Glyph): [Vec, Vec] {
  const tx = (G.bx - G.ax) / 4
  const ty = (G.by - G.ay) / 4
  const s1 = { x: G.mx - tx, y: G.my - ty }
  const s2 = { x: G.mx + tx, y: G.my + ty }
  const h1 = { x: (G.ax + G.mx) / 2, y: (G.ay + G.my) / 2 }
  const h2 = { x: (G.mx + G.bx) / 2, y: (G.my + G.by) / 2 }
  return [
    { x: lerp(h1.x, s1.x, G.round), y: lerp(h1.y, s1.y, G.round) },
    { x: lerp(h2.x, s2.x, G.round), y: lerp(h2.y, s2.y, G.round) },
  ]
}

/** Ids every scene emits, in painter order (some may have an empty path). */
export const NODE_IDS: string[] = [
  'shadow',
  'filletL',
  'filletR',
  'armL-back',
  'armR-back',
  'leafBig',
  'leafSmall',
  'body',
  'footL',
  'footR',
  'spotL0',
  'spotL1',
  'spotL2',
  'spotR0',
  'spotR1',
  'spotR2',
  'spotR3',
  'face',
  'toeL0',
  'toeL1',
  'toeR0',
  'toeR1',
  'soleL',
  'solePadsL',
  'soleR',
  'solePadsR',
  'belly',
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
  'armL-front',
  'armL-line',
  'armL-crease',
  'held-glass-0',
  'held-glass-1',
  'armR-front',
  'armR-line',
  'armR-crease',
  // the wrench is held in front of the hand (user, round 5); the magnifier's handle sits behind the fingers
  'held-wrench-0',
  'fx-marksL',
  'fx-marksR',
  'fx-sweat',
  'fx-question',
  'fx-bulb-glow',
  'fx-bulb',
  'fx-bulb-base',
  'fx-sparkle',
]

/** shoulder angles on the surface: the near arm is in front (cos > 0), the far one behind */
const ARM_THETA = { R: 75 * DEG, L: -115 * DEG }
/** the swap band: an arm passing round the side crossfades its occlusion over this depth range */
const SWAP = 0.16

export function buildMonsterScene(
  cfg: CharacterConfig,
  pose: Pose,
  opts: SceneOptions = {},
): Scene {
  const B = cfg.body
  const C = cfg.colors
  const M = cfg.motion
  const nodes = new Map<string, SceneNode>()
  const defs: SceneDef[] = []
  const put = (n: SceneNode) => nodes.set(n.id, n)
  const drift = M.drift
  const Wd = B.width
  const Hd = B.height
  const HT = BODY_H * Hd

  // ---- turn, tip and tilt fields ------------------------------------------------
  const tr = B.turnRange * DEG
  const lim = (v: number) => Math.max(-1.25, Math.min(1.25, v))
  const bodyT = lim(pose.turn + B.restTurn + pose.dTurn * 0.06 * drift)
  const headT = lim(pose.headTurn + pose.headLag)
  /** head weight: 0 below the chest, 1 at the face and above */
  const wHead = (y: number) => smoothstep((Y_CHEST - y) / (Y_CHEST - Y_FACE))
  /** turn angle at a height: the feet turn least, the head leads */
  const tA = (y: number) => tr * (bodyT * (0.35 + 0.65 * smoothstep(-y / 70)) + headT * wHead(y))
  const tipR = B.tipRange * DEG
  const tipA = (y: number) =>
    tipR * (pose.tip * smoothstep((-60 - y) / 140) + pose.headTip * wHead(y))
  const tiltDeg = pose.headTilt + pose.dTilt * 2.2 * drift

  /** turn + tip displacement of a point on the surface (rest coordinates) */
  const surface = (p: Vec, gain = 1): Vec => {
    const pr = profile(p.y)
    const dx = p.x - pr.c
    const R = Math.max(pr.R, Math.abs(dx) + 1e-6)
    const th = Math.asin(dx / R)
    const a = softLimit(th + tA(p.y) * gain)
    const x = p.x + R * (Math.sin(a) - Math.sin(th))
    let y = p.y
    const tp = tipA(p.y) * gain
    if (tp !== 0) {
      const v = Math.max(-0.995, Math.min(0.995, (p.y - HEAD_CY) / HEAD_RV))
      const ph = Math.asin(v)
      y += HEAD_RV * (Math.sin(softLimit(ph - tp)) - Math.sin(ph))
    }
    return { x, y }
  }
  /** the outline only bulges toward the turn (a solid of revolution keeps its silhouette) */
  const outline = (p: Vec): Vec => {
    const pr = profile(p.y)
    const dx = p.x - pr.c
    const R = Math.max(pr.R, Math.abs(dx) + 1e-6)
    // 1 − u² (not cos θ, whose slope is infinite at the edge): x′ stays monotonic, so the outline never folds
    const u = dx / R
    return { x: p.x + B.silhouette * 0.5 * R * Math.sin(tA(p.y)) * (1 - u * u), y: p.y }
  }
  const tilt = (p: Vec): Vec => {
    const w = smoothstep((Y_CHEST + 10 - p.y) / 120)
    if (Math.abs(tiltDeg) < 1e-4 || w <= 0) return p
    const a = tiltDeg * w * DEG
    const c = Math.cos(a)
    const s = Math.sin(a)
    const dx = p.x - NECK.x
    const dy = p.y - NECK.y
    return { x: NECK.x + dx * c - dy * s, y: NECK.y + dx * s + dy * c }
  }

  // ---- body flex ---------------------------------------------------------------
  const mo = M.morphAmount
  const syT = Math.max(0.55, 1 + pose.sy + pose.jy + pose.dBob * 0.006 * drift)
  const sxT = Math.max(0.55, (1 + pose.sx) / Math.sqrt(syT))
  const bulgeT = (pose.bulge + pose.breathShift * 0.004) * mo
  const bendT = (pose.bend + pose.jx) * mo
  const leanK = Math.tan(Math.max(-30, Math.min(30, pose.lean)) * DEG)
  const bAmt = (M.breathAmount / 100) * clamp01(pose.breath)
  const bCen = 0.36 - pose.breathShift * 0.06
  const liftU = pose.lift * HT
  const flex = (p: Vec): Vec => {
    let x = p.x * Wd
    let y = p.y * Hd
    const h = -y
    const hn = h / HT
    x *=
      1 + bAmt * Math.exp(-(((hn - bCen) / 0.24) ** 2)) + bulgeT * Math.sin(Math.PI * clamp01(hn))
    y *= syT * (1 + bAmt * 0.2)
    x *= lerp(1, sxT, smoothstep(h / 26))
    x += bendT * HT * hn * hn + leanK * h
    y -= liftU
    return { x, y }
  }

  const head = (p: Vec) => flex(tilt(surface(p)))

  // The face (patch, eye mask, pupils, lids, smile, blush) is one rigid 2D
  // unit attached to the body: it slides with the turn, rides the head's tip,
  // tilt, squash and hops, but never warps, foreshortens or stretches.
  const F0: Vec = { x: (REST.eyeCL.x + REST.eyeCR.x) / 2, y: -226 }
  const fs = surface(F0)
  const slide = 20 * Math.tanh(((fs.x - F0.x) * 0.7) / 20)
  const Fw = flex(tilt({ x: F0.x + slide, y: fs.y }))
  const fa = tiltDeg * DEG
  const fc = Math.cos(fa)
  const fsn = Math.sin(fa)
  const faceU = (p: Vec): Vec => {
    const dx = (p.x - F0.x) * Wd
    const dy = (p.y - F0.y) * Hd
    return { x: Fw.x + dx * fc - dy * fsn, y: Fw.y + dx * fsn + dy * fc }
  }

  // ---- appendage helpers ---------------------------------------------------------
  const depth = B.depth
  /** parallax shift of an anchor that sits at surface angle θ, radius R */
  const anchorShift = (pivot: Vec, theta: number, gain = 1) => {
    const pr = profile(pivot.y)
    const R = Math.abs(pivot.x - pr.c) / Math.max(0.2, Math.abs(Math.sin(theta)))
    const a = tA(pivot.y) * gain
    return { dx: R * (Math.sin(theta + a) - Math.sin(theta)) * depth, z: Math.cos(theta + a) }
  }
  const rot = (p: Vec, c: Vec, deg: number, sx = 1, sy = 1): Vec => {
    const a = deg * DEG
    const cs = Math.cos(a)
    const sn = Math.sin(a)
    const x = (p.x - c.x) * sx
    const y = (p.y - c.y) * sy
    return { x: c.x + x * cs - y * sn, y: c.y + x * sn + y * cs }
  }

  // ---- body and feet -------------------------------------------------------------
  // the ears are part of the body outline: flicking them bends the outline's
  // ear points about their base (weight 0 at the base → 1 at the tip)
  const earAng = {
    L: -(pose.earL + pose.sEarL + pose.dEar * 4 * drift),
    R: pose.earR + pose.sEarR - pose.dEar * 4 * drift,
  }
  const bodyChain = B.ears
    ? crClosed(
        REST.bodyEars.map(({ p, w }) => {
          if (w <= 0) return p
          const side = p.x < 0 ? 'L' : 'R'
          const pv = side === 'L' ? REST.earPivotL : REST.earPivotR
          const k = 1 + (B.earSize - 1) * w
          return rot(p, pv, earAng[side] * w, k, k)
        }),
      )
    : REST.body
  const bodyRest = bodyChain.map((p) => flex(tilt(outline(p))))
  const bodyD = chainD(bodyRest)
  put({ id: 'body', d: bodyD, fill: C.body })

  const footSpread = pose.footSpread * 60
  const foot = (side: 'L' | 'R') => {
    const pv = side === 'L' ? REST.footPivotL : REST.footPivotR
    const s = surface(pv)
    const dx = s.x - pv.x + (side === 'L' ? -footSpread : footSpread)
    const dy = -(side === 'L' ? pose.footL : pose.footR) * BODY_H
    const lifted = Math.max(0, -dy)
    // a lifted foot tips its sole toward us a little
    const tipDeg = Math.min(18, lifted * 0.5) * (side === 'L' ? -1 : 1)
    const T = (p: Vec) =>
      flex(rot({ x: p.x + dx, y: p.y + dy }, { x: pv.x + dx, y: pv.y + dy }, tipDeg))
    return T
  }
  const footT = { L: foot('L'), R: foot('R') }
  const footL = REST.footL.map(footT.L)
  const footR = REST.footR.map(footT.R)
  // a kicked-up foot shows its (shaded) sole
  put({
    id: 'footL',
    d: chainD(footL),
    fill: mixHex(C.body, C.dark, smoothstep(pose.footL / 0.1) * 0.5),
  })
  put({
    id: 'footR',
    d: chainD(footR),
    fill: mixHex(C.body, C.dark, smoothstep(pose.footR / 0.1) * 0.5),
  })
  defs.push({ kind: 'clip', id: 'body', d: bodyD + chainD(footL) + chainD(footR) })
  defs.push({ kind: 'clip', id: 'bodyOnly', d: bodyD })
  defs.push({ kind: 'clip', id: 'footL', d: chainD(footL) })
  defs.push({ kind: 'clip', id: 'footR', d: chainD(footR) })

  if (B.toes) {
    REST.toesL.forEach((t, i) =>
      put({ id: `toeL${i}`, d: chainD(t.map(footT.L)), fill: C.toe, clip: 'footL' }),
    )
    REST.toesR.forEach((t, i) =>
      put({ id: `toeR${i}`, d: chainD(t.map(footT.R)), fill: C.toe, clip: 'footR' }),
    )
  }

  // ---- soles (sitting: the soles turn to face us, toe pads showing) ----
  const soles = clamp01(pose.soles)
  if (soles > 0.01) {
    for (const side of ['L', 'R'] as const) {
      const pv = side === 'L' ? REST.footPivotL : REST.footPivotR
      const T = footT[side]
      const sg = side === 'L' ? -1 : 1
      const k = Math.max(0.01, Math.min(1.1, pose.soles))
      const c = { x: pv.x + sg * 4, y: pv.y + 4 }
      const E = (dx: number, dy: number, rx: number, ry: number, rot: number) =>
        ellipseChain(c.x + dx * k, c.y + dy * k, rx * k, ry * k, rot).map(T)
      put({
        id: `sole${side}`,
        d: chainD(E(0, 0, 25, 21, sg * 14)),
        fill: C.body,
        stroke: {
          color: C.dark,
          width: 1.3 * Wd,
          opacity: Math.min(1, soles * 1.5),
          join: 'round',
        },
      })
      const pads = [
        E(0, 5, 10.5, 8, 0),
        E(-13, -9, 5, 6, -18),
        E(0, -14, 5, 6, 0),
        E(13, -9, 5, 6, 18),
      ]
      put({ id: `solePads${side}`, d: pads.map((p) => chainD(p)).join(''), fill: C.toe })
    }
  }

  // ---- shadow --------------------------------------------------------------------
  if (B.shadow > 0.01) {
    const up = Math.max(0, pose.lift)
    const k = 1 / (1 + up * 3.2)
    const fl = flex({ x: 0, y: 0 })
    const cx = (footL[6].x + footR[6].x) / 2
    put({
      id: 'shadow',
      d: chainD(ellipseChain(lerp(cx, fl.x, 0.5), 1.5, 128 * Wd * k, 10 * k)),
      fill: C.dark,
      opacity: B.shadow * 0.32 * (0.35 + 0.65 * k),
    })
  }

  // ---- surface features ----------------------------------------------------------
  const spot = (c: Chain) => c.map((p) => flex(tilt(surface(p))))
  if (B.spots) {
    REST.spotsL.forEach((c, i) =>
      put({ id: `spotL${i}`, d: chainD(spot(c)), fill: C.dark, clip: 'body' }),
    )
    REST.spotsR.forEach((c, i) =>
      put({ id: `spotR${i}`, d: chainD(spot(c)), fill: C.spotLight, clip: 'body' }),
    )
  }
  const facePatch = REST.face.map(faceU)
  put({ id: 'face', d: chainD(facePatch), fill: C.face, clip: 'body' })
  if (B.creases) {
    const bl = REST.belly.map((p) => flex(surface(p))) as [Vec, Vec, Vec, Vec]
    put({
      id: 'belly',
      d: polyPath(taperedStroke(bl, ART.BELLY.w.map((w) => w * S) as [number, number, number])),
      fill: C.dark,
    })
  }

  // ---- mouth -----------------------------------------------------------------------
  // The smile is the eye mask's bottom edge, so between the mask's sides it
  // stays put (the white never changes shape; it may only flatten a hair).
  // Expressions bend the corners outside the mask: up for joy, down for sad.
  const [m0, m1, m2, m3] = REST.mouth
  const tJL = cubicTAtX(m0, m1, m2, m3, REST.eyeCL.x - REST.eyeR)
  const tJR = cubicTAtX(m0, m1, m2, m3, REST.eyeCR.x + REST.eyeR)
  const flat = Math.max(0, Math.min(1, -pose.mouthSmile)) * 4
  const mid0 = cubicSub(m0, m1, m2, m3, tJL, tJR)
  const upper: [Vec, Vec, Vec, Vec] = [
    mid0[0],
    { x: mid0[1].x, y: mid0[1].y - flat },
    { x: mid0[2].x, y: mid0[2].y - flat },
    mid0[3],
  ]
  const limC = (v: number) => Math.max(-13, Math.min(17, v))
  const dyL = limC(
    -pose.mouthSmile * 6 - pose.mouthEnds * 4 + pose.mouthTilt * 3 + pose.mouthWave * 6,
  )
  const dyR = limC(
    -pose.mouthSmile * 6 - pose.mouthEnds * 4 - pose.mouthTilt * 6 - pose.mouthWave * 6,
  )
  const left = cubicSub(m0, m1, m2, m3, 0, tJL)
  const right = cubicSub(m0, m1, m2, m3, tJR, 1)
  left[0] = { x: left[0].x + Math.max(0, dyL) * 0.15, y: left[0].y + dyL }
  left[1] = { x: left[1].x, y: left[1].y + dyL * 0.55 }
  right[3] = { x: right[3].x - Math.max(0, dyR) * 0.15, y: right[3].y + dyR }
  right[2] = { x: right[2].x, y: right[2].y + dyR * 0.55 }
  const mouthW = ART.MOUTH.w * S * cfg.eyes.mouthWidth
  const mapped = (c: Vec[]) => c.map(faceU)
  const mp = mapped([...left, upper[1], upper[2], upper[3], right[1], right[2], right[3]])
  const n2m = (v: Vec) => `${v.x.toFixed(2)},${v.y.toFixed(2)}`
  put({
    id: 'mouth',
    d: `M${n2m(mp[0])}C${n2m(mp[1])} ${n2m(mp[2])} ${n2m(mp[3])}C${n2m(mp[4])} ${n2m(mp[5])} ${n2m(mp[6])}C${n2m(mp[7])} ${n2m(mp[8])} ${n2m(mp[9])}`,
    fill: 'none',
    stroke: { color: C.ink, width: mouthW, opacity: 1, cap: 'round' },
    clip: 'body',
  })
  // the two fangs ride the smile's corners (they move with them when it bends)
  if (cfg.eyes.teeth) {
    const xc = { L: m0.x, R: m3.x }
    const xj = { L: mid0[0].x, R: mid0[3].x }
    for (const side of ['L', 'R'] as const) {
      const dy = side === 'L' ? dyL : dyR
      const ch = (side === 'L' ? REST.toothL : REST.toothR).map((p) => {
        const w = clamp01((p.x - xj[side]) / (xc[side] - xj[side]))
        return faceU({ x: p.x, y: p.y + dy * w * 0.9 })
      })
      put({ id: `tooth${side}`, d: chainD(ch), fill: C.eyeWhite, clip: 'body' })
    }
  }
  // ---- eye mask (two lobes over the smile) -------------------------------------------
  const cL = REST.eyeCL
  const cR = REST.eyeCR
  const r = REST.eyeR
  const xLs = cL.x - r
  const xRs = cR.x + r
  const mouthAt = (x: number) => {
    const t = cubicTAtX(upper[0], upper[1], upper[2], upper[3], x)
    return { t, y: cubicAt(upper[0], upper[1], upper[2], upper[3], t).y }
  }
  const mL = mouthAt(xLs)
  const mR = mouthAt(xRs)
  const yL = Math.max(mL.y, cL.y + 1)
  const yR = Math.max(mR.y, cR.y + 1)
  const notchX = (cL.x + cR.x) / 2
  const notchDy = Math.sqrt(Math.max(0, r * r - (notchX - cL.x) ** 2))
  const aL = Math.atan2(-notchDy, notchX - cL.x)
  const aR = Math.atan2(-notchDy, notchX - cR.x) + 2 * Math.PI // (π, 3π/2): the arc runs over the top to 2π
  const sub = cubicSub(upper[0], upper[1], upper[2], upper[3], mL.t, mR.t)
  const mask: Chain = [
    { x: xLs, y: yL },
    ...lineSeg({ x: xLs, y: yL }, { x: xLs, y: cL.y }),
    ...arcChain(cL, r, Math.PI, 2 * Math.PI + aL).slice(1),
    ...arcChain(cR, r, aR, 2 * Math.PI).slice(1),
    ...lineSeg({ x: xRs, y: cR.y }, { x: xRs, y: yR }),
    ...lineSeg({ x: xRs, y: yR }, sub[3]),
    sub[2],
    sub[1],
    sub[0],
    ...lineSeg(sub[0], { x: xLs, y: yL }),
  ]
  const maskW = mapped(mask)
  const maskD = chainD(maskW)
  put({ id: 'eyeWhite', d: maskD, fill: C.eyeWhite })

  // ---- pupils: placed with true-distance padding inside their lobe, above the smile ----
  const pupilScale = cfg.eyes.pupilSize * (1 + pose.pupil)
  const pad = cfg.eyes.edgePadding * 0.25
  const mouthPoly = Array.from({ length: 25 }, (_, i) =>
    cubicAt(upper[0], upper[1], upper[2], upper[3], i / 24),
  )
  const mouthClear = (p: Vec, rho: number) => {
    let d = Infinity
    for (let i = 0; i < mouthPoly.length - 1; i++) {
      const a = mouthPoly[i]
      const b = mouthPoly[i + 1]
      const dx = b.x - a.x
      const dy = b.y - a.y
      const t = Math.max(
        0,
        Math.min(1, ((p.x - a.x) * dx + (p.y - a.y) * dy) / (dx * dx + dy * dy)),
      )
      d = Math.min(d, Math.hypot(p.x - a.x - dx * t, p.y - a.y - dy * t))
    }
    const below = p.y > mouthAt(Math.max(upper[0].x, Math.min(upper[3].x, p.x))).y
    return !below && d >= rho + mouthW / 2
  }
  const fits = (c: Vec, center: Vec, G: Glyph) => {
    const rho = G.w / 2 + pad
    for (const s of glyphSamples(G)) {
      const p = { x: center.x + s.x, y: center.y + s.y }
      if (p.y < c.y ? Math.hypot(p.x - c.x, p.y - c.y) > r - rho : Math.abs(p.x - c.x) > r - rho)
        return false
      if (!mouthClear(p, rho)) return false
    }
    return true
  }
  const mob = cfg.eyes.mobility
  const placePupil = (side: 'L' | 'R') => {
    const c = side === 'L' ? cL : cR
    let base = side === 'L' ? REST.pupilL : REST.pupilR
    let G = glyphFor(pose, side, pupilScale)
    for (let i = 0; i < 14 && !fits(c, base, G); i++) base = { x: base.x, y: base.y - 1 }
    // the glyph must fit at rest; shrink it if a design makes it too big
    let gs = 1
    for (let i = 0; i < 12 && !fits(c, base, G); i++) {
      gs *= 0.92
      G = {
        ...G,
        ax: G.ax * 0.92,
        ay: G.ay * 0.92,
        mx: G.mx * 0.92,
        my: G.my * 0.92,
        bx: G.bx * 0.92,
        by: G.by * 0.92,
        w: G.w * 0.92,
      }
    }
    const want = {
      x: base.x + pose.gazeX * r * 0.82 * mob,
      y: base.y + pose.gazeY * r * 0.62 * mob,
    }
    if (fits(c, want, G)) return { G, at: want, gs }
    let lo = 0
    let hi = 1
    for (let i = 0; i < 14; i++) {
      const m = (lo + hi) / 2
      if (fits(c, { x: lerp(base.x, want.x, m), y: lerp(base.y, want.y, m) }, G)) lo = m
      else hi = m
    }
    return { G, at: { x: lerp(base.x, want.x, lo), y: lerp(base.y, want.y, lo) }, gs }
  }
  const pup = { L: placePupil('L'), R: placePupil('R') }
  const glyphD = (G: Glyph, at: Vec) => {
    const [q1, q2] = glyphCtrl(G)
    const P = (x: number, y: number) => faceU({ x: at.x + x, y: at.y + y })
    const A = P(G.ax, G.ay)
    const Q1 = P(q1.x, q1.y)
    const Mm = P(G.mx, G.my)
    const Q2 = P(q2.x, q2.y)
    const Bb = P(G.bx, G.by)
    const n = (v: Vec) => `${v.x.toFixed(2)},${v.y.toFixed(2)}`
    return `M${n(A)}Q${n(Q1)} ${n(Mm)}Q${n(Q2)} ${n(Bb)}`
  }
  for (const side of ['L', 'R'] as const) {
    const p = pup[side]
    const k = (Wd + Hd) / 2
    put({
      id: `pupil${side}`,
      d: glyphD(p.G, p.at),
      fill: 'none',
      stroke: { color: C.ink, width: p.G.w * k, opacity: 1, cap: 'round', join: 'round' },
    })
  }

  // ---- lids: they cut the mask itself (and the pupils), so above a lid you
  // simply see what is under the mask: no painted lid, no two-tone seams ----
  const lidY = (side: 'L' | 'R') => {
    const amt = clamp01(side === 'L' ? pose.lidL : pose.lidR)
    const c = side === 'L' ? cL : cR
    const tiltD = (side === 'L' ? pose.lidTiltL : pose.lidTiltR) * DEG
    const inward = side === 'L' ? 1 : -1 // + tilt = inner corner up
    const y = c.y - r - 3 + amt * (r + 13)
    return { amt, at: (x: number) => y - Math.tan(tiltD) * (x - c.x) * inward }
  }
  const lL = lidY('L')
  const lR = lidY('R')
  let lidClip: string | undefined
  if (lL.amt > 0.004 || lR.amt > 0.004) {
    const xa = xLs - 20
    const xb = xRs + 20
    const bot = cL.y + 120
    const pts: Vec[] = [
      { x: xa, y: lL.at(xa) },
      { x: notchX, y: lL.at(notchX) },
      { x: notchX, y: lR.at(notchX) },
      { x: xb, y: lR.at(xb) },
      { x: xb, y: bot },
      { x: xa, y: bot },
    ]
    const ch: Chain = [pts[0]]
    for (let i = 0; i < pts.length; i++) ch.push(...lineSeg(pts[i], pts[(i + 1) % pts.length]))
    defs.push({ kind: 'clip', id: 'lids', d: chainD(mapped(ch)) })
    lidClip = 'lids'
    for (const id of ['eyeWhite', 'pupilL', 'pupilR']) {
      const node = nodes.get(id)
      if (node) node.clip = lidClip
    }
  }

  // ---- blush -----------------------------------------------------------------------
  const blush = cfg.eyes.blush ? clamp01(pose.blush) : 0
  if (blush > 0.01) {
    const spotsB: [Vec, number][] = [
      [{ x: -87, y: -197.5 }, 1],
      [{ x: 41, y: -203 }, -1],
    ]
    spotsB.forEach(([c, s], i) => {
      const id = i === 0 ? 'L' : 'R'
      put({
        id: `blush${id}`,
        d: chainD(ellipseChain(c.x, c.y, 10.5, 6, s * 6).map(faceU)),
        fill: C.blush,
        opacity: 0.85 * blush,
      })
      let hd = ''
      for (let k = -1; k <= 1; k++) {
        const a = faceU({ x: c.x + k * 4.6 - 1.4, y: c.y + 2.6 })
        const b = faceU({ x: c.x + k * 4.6 + 1.4, y: c.y - 2.6 })
        hd += `M${a.x.toFixed(2)},${a.y.toFixed(2)}L${b.x.toFixed(2)},${b.y.toFixed(2)}`
      }
      put({
        id: `blushHatch${id}`,
        d: hd,
        fill: 'none',
        stroke: {
          color: mixHex(C.blush, '#B02A2A', 0.45),
          width: 1.1,
          opacity: blush,
          cap: 'round',
        },
      })
    })
  }

  // ---- ears and leaves (behind the head) ---------------------------------------------
  if (B.leaves) {
    // rooted in the head: the leaves ride the head's own outline (top of the
    // dome), tilt with it and sway about a pivot buried inside it
    const top = { x: REST.leafPivotBig.x, y: REST.top + 2 }
    const anchor = flex(tilt(outline(top)))
    const at = { x: anchor.x - top.x * Wd, y: anchor.y - top.y * Hd }
    const tTop = tA(top.y)
    const fore = 0.7 + 0.3 * Math.cos(tTop)
    // the liveliest part: their own sway, a breath, follow-through
    const base = pose.leaf + pose.sLeaf + pose.dLeaf * 9 * drift + pose.breath * 2.5
    const small = pose.dTilt * 6 * drift - pose.breath * 1.5
    const tiltHere = tiltDeg + (Math.atan(leanK) / DEG) * 0.6
    const leafChain = (c: Chain, pv: Vec, k: number, extra = 0) =>
      c.map((p) => {
        const q = rot(p, pv, base * k + extra + tiltHere, fore * B.leafSize, B.leafSize)
        return { x: q.x * Wd + at.x, y: q.y * Hd + at.y }
      })
    put({ id: 'leafBig', d: chainD(leafChain(REST.leafBig, REST.leafPivotBig, 1)), fill: C.leaf })
    put({
      id: 'leafSmall',
      d: chainD(leafChain(REST.leafSmall, REST.leafPivotSmall, 1.3, small)),
      fill: C.leafLight,
    })
  }

  // ---- arms ---------------------------------------------------------------------------
  const armData = (side: 'L' | 'R') => {
    const pv = side === 'L' ? REST.armPivotL : REST.armPivotR
    const rest = side === 'L' ? REST.armL : REST.armR
    const out =
      side === 'L'
        ? pose.armL + pose.sArmL - pose.dArm * 3 * drift
        : pose.armR + pose.sArmR + pose.dArm * 3 * drift
    const screen = side === 'L' ? out : -out // outward is anticlockwise on the right
    const len = Math.max(0.6, 1 + (side === 'L' ? pose.armLLen : pose.armRLen)) * B.armSize
    const sh = anchorShift(pv, ARM_THETA[side])
    const fwd = clamp01(side === 'L' ? pose.armLFwd : pose.armRFwd)
    const z = sh.z + fwd * 1.3
    const sy = surface(pv).y - pv.y
    const T = (p: Vec) => {
      const q = rot(p, pv, screen, B.armSize, len)
      // a little depth shift only: the arm stays attached to the body's edge
      return flex(tilt({ x: q.x + sh.dx * 0.35, y: q.y + sy }))
    }
    return { T, rest, z, screen, side, pv, out, fwd }
  }
  const arms = { L: armData('L'), R: armData('R') }

  // Shoulder fillets: where the top of a hanging arm meets the body, the two
  // outlines would form a notch and read as separate shapes. A body-coloured
  // fillet bridges the body's edge smoothly into the arm's outer edge, so the
  // silhouette is one continuous curve. It shrinks away as the arm is raised
  // or brought to the front (no pop).
  const bodyPoly = flatten(bodyRest, 6)
  const edgeX = (poly: Vec[], y: number, side: 'L' | 'R'): number | null => {
    let best = side === 'R' ? -Infinity : Infinity
    for (let i = 0; i < poly.length; i++) {
      const a = poly[i]
      const b = poly[(i + 1) % poly.length]
      if (a.y > y !== b.y > y) {
        const x = a.x + ((y - a.y) / (b.y - a.y)) * (b.x - a.x)
        best = side === 'R' ? Math.max(best, x) : Math.min(best, x)
      }
    }
    return Number.isFinite(best) ? best : null
  }
  const shoulderFillet = (side: 'L' | 'R', ch: Chain, shoulder: Vec, out: number, fwd: number) => {
    const k = (1 - smoothstep((Math.abs(out) - 18) / 20)) * (1 - smoothstep(fwd / 0.5))
    if (k <= 0.001) return
    const armPoly = flatten(ch, 6)
    const sgn = side === 'R' ? 1 : -1
    let yJ: number | null = null
    for (let y = shoulder.y - 34; y <= shoulder.y + 70; y += 1) {
      const ax = edgeX(armPoly, y, side)
      const bx = edgeX(bodyPoly, y, side)
      if (ax !== null && bx !== null && (ax - bx) * sgn > 0.4) {
        yJ = y
        break
      }
    }
    if (yJ === null) return
    // the real silhouette along the joint, then its convex outer envelope:
    // the envelope bridges every notch and corner, so the side reads as one curve
    const sc = (Wd + Hd) / 2
    const ys: number[] = []
    for (let y = yJ - 36 * sc; y <= yJ + 52 * sc; y += 1.5) ys.push(y)
    const actual: Vec[] = []
    for (const y of ys) {
      const bx = edgeX(bodyPoly, y, side)
      const ax = edgeX(armPoly, y, side)
      if (bx === null && ax === null) continue
      const u = Math.max(bx === null ? -Infinity : sgn * bx, ax === null ? -Infinity : sgn * ax)
      actual.push({ x: y, y: u }) // (v = height, u = outwardness)
    }
    if (actual.length < 4) return
    const hull: Vec[] = []
    for (const p of actual) {
      while (hull.length >= 2) {
        const o = hull[hull.length - 2]
        const q = hull[hull.length - 1]
        if ((q.x - o.x) * (p.y - o.y) - (q.y - o.y) * (p.x - o.x) >= 0) hull.pop()
        else break
      }
      hull.push(p)
    }
    const hullAt = (v: number) => {
      for (let i = 1; i < hull.length; i++)
        if (v <= hull[i].x) {
          const t = (v - hull[i - 1].x) / (hull[i].x - hull[i - 1].x || 1)
          return hull[i - 1].y + (hull[i].y - hull[i - 1].y) * t
        }
      return hull[hull.length - 1].y
    }
    const pts = actual.map((p) => ({ x: sgn * lerp(p.y, hullAt(p.x), k), y: p.x }))
    const n = (v: Vec) => `${v.x.toFixed(2)},${v.y.toFixed(2)}`
    // back through the shoulder (inside both shapes) to close it
    put({ id: `fillet${side}`, d: `M${pts.map(n).join('L')}L${n(shoulder)}Z`, fill: C.body })
  }
  const moved = (c: Chain, dx: number) => chainD(c.map((p) => ({ x: p.x + dx, y: p.y })))
  const bodyBox = bbox(bodyRest)
  const hands: Record<'L' | 'R', Vec> = { L: { x: 0, y: 0 }, R: { x: 0, y: 0 } }
  for (const side of ['L', 'R'] as const) {
    const A = arms[side]
    const ch = A.rest.map(A.T)
    hands[side] = A.T(side === 'L' ? REST.handL : REST.handR)
    shoulderFillet(side, ch, A.T(A.pv), A.out, A.fwd)
    const d = chainD(ch)
    // the far arm sits in shade; raised out to the side it catches the light
    const raised = smoothstep(((side === 'L' ? pose.armL : pose.armR) - 45) / 60)
    // behind: in shade; in the swap band the colour turns to the body's while
    // the body still covers the overlap, and only then does the cover slide
    // off, so nothing semi-dark ever shows over the body
    const band = clamp01((A.z + SWAP) / (2 * SWAP))
    const lit = A.z <= -SWAP ? 0 : smoothstep(band / 0.5)
    const color = mixHex(C.body, C.dark, B.armShade * (1 - Math.max(lit, raised)))
    if (A.z <= -SWAP) {
      put({ id: `arm${side}-back`, d, fill: color })
      continue
    }
    // in front; inside the swap band the body still hides the part of the arm
    // behind it, and that cover slides off continuously (no pop when the order flips)
    let clip: string | undefined
    if (A.z < SWAP) {
      const ab = bbox(ch)
      const overlap = side === 'R' ? bodyBox.x1 - ab.x0 : ab.x1 - bodyBox.x0
      const k = smoothstep((band - 0.5) / 0.5)
      const slide = Math.max(0, overlap + 4) * k * (side === 'R' ? -1 : 1)
      const big = `M${bodyBox.x0 - 600},${bodyBox.y0 - 600}H${bodyBox.x1 + 600}V${bodyBox.y1 + 600}H${bodyBox.x0 - 600}Z`
      // "outside the body" ∩ "outside each foot" as nested clips: a single
      // even-odd path would count body∩foot twice and let the arm show through
      defs.push({
        kind: 'clip',
        id: `arm${side}Body`,
        d: big + moved(bodyRest, slide),
        evenodd: true,
      })
      defs.push({
        kind: 'clip',
        id: `arm${side}FootL`,
        d: big + moved(footL, slide),
        evenodd: true,
        clipBy: `arm${side}Body`,
      })
      defs.push({
        kind: 'clip',
        id: `arm${side}`,
        d: big + moved(footR, slide),
        evenodd: true,
        clipBy: `arm${side}FootL`,
      })
      clip = `arm${side}`
    }
    put({ id: `arm${side}-front`, d, fill: color, clip })
    // brought round in front of the body (hand to cheek, hands together), the
    // arm gets a thin outline in the shade colour, as in the pose sheet; hidden
    // at the shoulder, where the arm grows out of the body
    const fwd = clamp01(side === 'L' ? pose.armLFwd : pose.armRFwd)
    const lineOp = smoothstep((fwd - 0.3) / 0.5) * smoothstep((A.z - 0.05) / 0.3)
    if (lineOp > 0.01 && !clip) {
      const pw = A.T(A.pv)
      const r0 = 27 * B.armSize
      const big = `M${bodyBox.x0 - 600},${bodyBox.y0 - 600}H${bodyBox.x1 + 600}V${bodyBox.y1 + 600}H${bodyBox.x0 - 600}Z`
      defs.push({
        kind: 'clip',
        id: `armCap${side}`,
        d: big + chainD(ellipseChain(pw.x, pw.y, r0, r0)),
        evenodd: true,
      })
      put({
        id: `arm${side}-line`,
        d,
        fill: 'none',
        stroke: { color: C.dark, width: 1.5 * Wd, opacity: lineOp, join: 'round' },
        clip: `armCap${side}`,
      })
    }
    if (B.creases) {
      const full = (side === 'L' ? REST.creaseL : REST.creaseR).map(A.T) as [Vec, Vec, Vec, Vec]
      // only the lower part (toward the hand), fading in: the shoulder blends into the body
      const crease = cubicSub(full[0], full[1], full[2], full[3], 0.38, 1)
      const op = A.z >= SWAP ? 1 : smoothstep((band - 0.5) / 0.5)
      if (op > 0.01)
        put({
          id: `arm${side}-crease`,
          d: polyPath(taperedStroke(crease, [0.2, 10 * S, 3 * S])),
          fill: C.dark,
          opacity: op,
          clip,
        })
    }
  }

  const lowest = (c: Chain) => c.reduce((a, b) => (b.y > a.y ? b : a))
  // ---- anchors ---------------------------------------------------------------------------
  const pupilL = faceU(pup.L.at)
  const pupilR = faceU(pup.R.at)
  const eyeL = faceU(cL)
  const eyeR = faceU(cR)
  const maskPoly = flatten(maskW, 4)
  const faceBox = bbox(maskPoly)
  const faceC = { x: (pupilL.x + pupilR.x) / 2, y: (pupilL.y + pupilR.y) / 2 }
  const hull = flatten(bodyRest, 4)
  const headTop = head({ x: -10, y: -300 })

  // ---- effects (flat marks that follow the head) ------------------------------------------
  const fxScale = cfg.effects.size
  const fxOn = (id: EffectId, v: number) => (cfg.effects[id] ? clamp01(v) : 0)
  const ph = ((pose.fxPhase % 1) + 1) % 1
  const n2 = (v: Vec) => `${v.x.toFixed(2)},${v.y.toFixed(2)}`
  const marks = (id: 'L' | 'R', v: number) => {
    if (v < 0.01) return
    const s = id === 'L' ? -1 : 1
    const o = head({ x: s * 104 - 8, y: -306 })
    let d = ''
    ;[-62, -36, -10].forEach((deg, i) => {
      const a = deg * DEG * 1
      const dir = { x: Math.cos(a) * s, y: Math.sin(a) }
      const r0 = (12 + i * 1) * fxScale
      const len = (17 - i * 2.2) * fxScale * (0.35 + 0.65 * v)
      d += `M${n2({ x: o.x + dir.x * r0, y: o.y + dir.y * r0 })}L${n2({ x: o.x + dir.x * (r0 + len), y: o.y + dir.y * (r0 + len) })}`
    })
    put({
      id: `fx-marks${id}`,
      d,
      fill: 'none',
      opacity: Math.min(1, v * 1.5),
      stroke: { color: C.effects, width: 4.6 * fxScale, opacity: 1, cap: 'round' },
    })
  }
  // 0..1 = marks by the right side; past 1 they appear on the left too (cheering)
  marks('R', fxOn('marks', pose.fxMarks))
  marks('L', fxOn('marks', pose.fxMarks - 1))

  const sweat = fxOn('sweat', pose.fxSweat)
  if (sweat > 0.01) {
    const o = head({ x: -74, y: -288 })
    const k = 7 * fxScale * (0.4 + 0.6 * sweat)
    const cy = o.y + Math.sin(ph * Math.PI * 2) * 1.5
    const d = `M${o.x.toFixed(2)},${(cy - k * 1.6).toFixed(2)}C${(o.x + k * 0.5).toFixed(2)},${(cy - k * 0.6).toFixed(2)} ${(o.x + k).toFixed(2)},${(cy - k * 0.1).toFixed(2)} ${(o.x + k).toFixed(2)},${(cy + k * 0.35).toFixed(2)}A${k.toFixed(2)},${k.toFixed(2)} 0 1 1 ${(o.x - k).toFixed(2)},${(cy + k * 0.35).toFixed(2)}C${(o.x - k).toFixed(2)},${(cy - k * 0.1).toFixed(2)} ${(o.x - k * 0.5).toFixed(2)},${(cy - k * 0.6).toFixed(2)} ${o.x.toFixed(2)},${(cy - k * 1.6).toFixed(2)}Z`
    put({ id: 'fx-sweat', d, fill: '#8FD3F4', opacity: Math.min(1, sweat * 1.4) })
  }
  const q = fxOn('question', pose.fxQuestion)
  if (q > 0.01) {
    const o = {
      x: headTop.x + 64 * fxScale,
      y: headTop.y - 26 * fxScale + Math.sin(ph * Math.PI * 2) * 2,
    }
    const k = 9 * fxScale * (0.5 + 0.5 * q)
    const d = `M${(o.x - k).toFixed(2)},${(o.y - k * 0.6).toFixed(2)}C${(o.x - k).toFixed(2)},${(o.y - k * 1.9).toFixed(2)} ${(o.x + k).toFixed(2)},${(o.y - k * 1.9).toFixed(2)} ${(o.x + k).toFixed(2)},${(o.y - k * 0.6).toFixed(2)}C${(o.x + k).toFixed(2)},${(o.y + k * 0.3).toFixed(2)} ${o.x.toFixed(2)},${(o.y + k * 0.1).toFixed(2)} ${o.x.toFixed(2)},${(o.y + k * 0.9).toFixed(2)}M${o.x.toFixed(2)},${(o.y + k * 1.75).toFixed(2)}l0,0.01`
    put({
      id: 'fx-question',
      d,
      fill: 'none',
      opacity: Math.min(1, q * 1.5),
      stroke: { color: C.effects, width: 3.6 * fxScale, opacity: 1, cap: 'round', join: 'round' },
    })
  }
  const bulb = fxOn('bulb', pose.fxBulb)
  if (bulb > 0.01) {
    const o = { x: headTop.x + 6, y: headTop.y - 58 * fxScale * (0.7 + 0.3 * bulb) }
    const k = 11 * fxScale * (0.4 + 0.6 * bulb)
    put({
      id: 'fx-bulb-glow',
      d: chainD(ellipseChain(o.x, o.y, k * 1.75, k * 1.75)),
      fill: '#FFF2B3',
      opacity: 0.55 * bulb * (0.75 + 0.25 * Math.sin(ph * Math.PI * 4)),
    })
    put({
      id: 'fx-bulb',
      d: chainD(ellipseChain(o.x, o.y, k, k)),
      fill: '#FFD84D',
      opacity: Math.min(1, bulb * 1.5),
    })
    put({
      id: 'fx-bulb-base',
      d: `M${(o.x - k * 0.45).toFixed(2)},${(o.y + k * 0.85).toFixed(2)}h${(k * 0.9).toFixed(2)}v${(k * 0.55).toFixed(2)}h${(-k * 0.9).toFixed(2)}Z`,
      fill: '#A2A2A2',
      opacity: Math.min(1, bulb * 1.5),
    })
  }
  const sp = fxOn('sparkle', pose.fxSparkle)
  if (sp > 0.01) {
    let d = ''
    const spots: Vec[] = [
      head({ x: -120, y: -290 }),
      head({ x: 112, y: -270 }),
      head({ x: 128, y: -170 }),
    ]
    spots.forEach((o, i) => {
      const t = (ph * 2 + i / 3) % 1
      const k = (5 + i) * fxScale * sp * (0.55 + 0.45 * Math.sin(t * Math.PI))
      const w = k * 0.28
      d += `M${o.x.toFixed(2)},${(o.y - k).toFixed(2)}Q${(o.x + w).toFixed(2)},${(o.y - w).toFixed(2)} ${(o.x + k).toFixed(2)},${o.y.toFixed(2)}Q${(o.x + w).toFixed(2)},${(o.y + w).toFixed(2)} ${o.x.toFixed(2)},${(o.y + k).toFixed(2)}Q${(o.x - w).toFixed(2)},${(o.y + w).toFixed(2)} ${(o.x - k).toFixed(2)},${o.y.toFixed(2)}Q${(o.x - w).toFixed(2)},${(o.y - w).toFixed(2)} ${o.x.toFixed(2)},${(o.y - k).toFixed(2)}Z`
    })
    put({ id: 'fx-sparkle', d, fill: '#FFD34E' })
  }

  // ---- props -----------------------------------------------------------------------------
  const P = cfg.props
  const accent = C.accessory
  const propFill: Record<PropFill, string> = {
    accent,
    accentSoft: C.accessoryLight,
    accentDeep: C.accessoryDeep,
    accentMid: C.accessoryMid,
    card: mixHex(accent, C.accessoryMid, 0.12),
    metal: C.metal,
    body: C.body,
  }
  const amp = P.motionAmp
  const bob = pose.propBob * 2.5 * amp
  const sway = pose.propSway * amp
  const props: PropNode[] = []
  const propAnchors: Partial<Record<PropId, Vec>> = {}
  const eyePoly = maskPoly
  const rectAt = (c: Vec, w: number, h: number, deg: number) => {
    const a = deg * DEG
    return [
      [-w / 2, -h / 2],
      [w / 2, -h / 2],
      [w / 2, h / 2],
      [-w / 2, h / 2],
    ].map(([x, y]) => ({
      x: c.x + x * Math.cos(a) - y * Math.sin(a),
      y: c.y + x * Math.sin(a) + y * Math.cos(a),
    }))
  }
  const scale0 = 0.62 * ((Wd + Hd) / 2)
  const addProp = (
    id: PropId,
    appear: number,
    build: () => { parts: PropPart[]; width: number; height: number },
    place: (k: number) => { c: Vec; rot: number },
    /** held in the fist: this local point sits in the hand, drawn behind the fingers */
    grip?: Vec,
    gripScale = 1,
  ) => {
    if (opts.propsEnabled && opts.propsEnabled[id] === false) return
    const pc = P[id]
    if (!pc.enabled) return
    const a = Math.max(0, appear)
    const k = scale0 * pc.size * gripScale
    const built = build()
    const { c: at, rot: rr } = place(k)
    let c = { x: at.x + pc.offsetX * 150 * Wd, y: at.y + pc.offsetY * 157 * Hd }
    const deg = rr + pc.rotation
    if (grip) {
      // c is the hand: the tool hangs from its grip, scaling in about it
      const a0 = deg * DEG
      const center = {
        x: c.x - (grip.x * Math.cos(a0) - grip.y * Math.sin(a0)) * k,
        y: c.y - (grip.x * Math.sin(a0) + grip.y * Math.cos(a0)) * k,
      }
      propAnchors[id] = center
      const a = Math.max(0, appear)
      if (a < 0.005) return
      const hw = (built.width * k) / 2
      const hh = (built.height * k) / 2
      const tf = `translate(${c.x.toFixed(2)},${c.y.toFixed(2)}) rotate(${deg.toFixed(2)}) scale(${(k * a).toFixed(4)}) translate(${(-grip.x).toFixed(2)},${(-grip.y).toFixed(2)})`
      built.parts.forEach((pp, i) =>
        put({
          id: `held-${id}-${i}`,
          d: pp.d,
          fill: propFill[pp.fill],
          evenodd: pp.evenodd,
          transform: tf,
          opacity: Math.min(1, a * 1.6),
        }),
      )
      props.push({
        id,
        transform: tf,
        opacity: Math.min(1, a * 1.6),
        parts: [],
        center,
        box: { x0: center.x - hw, y0: center.y - hh, x1: center.x + hw, y1: center.y + hh },
      })
      return
    }
    // never cover the eyes: push straight away from the face, continuously
    // (the eye mask's own unsnapped outline, so the push never jumps)
    const hw = (built.width * k) / 2 + 3
    const hh = (built.height * k) / 2 + 3
    if (polysOverlap(rectAt(c, hw * 2, hh * 2, deg), eyePoly)) {
      const dx = c.x - faceC.x
      const dy = c.y - faceC.y
      const n = Math.hypot(dx, dy) || 1
      const dir = { x: dx / n, y: dy / n }
      let lo = 0
      let hi = 260
      for (let i = 0; i < 16; i++) {
        const m = (lo + hi) / 2
        if (
          polysOverlap(
            rectAt({ x: c.x + dir.x * m, y: c.y + dir.y * m }, hw * 2, hh * 2, deg),
            eyePoly,
          )
        )
          lo = m
        else hi = m
      }
      c = { x: c.x + dir.x * hi, y: c.y + dir.y * hi }
    }
    propAnchors[id] = c
    if (a < 0.005) return
    const s = k * a
    props.push({
      id,
      transform: `translate(${c.x.toFixed(2)},${c.y.toFixed(2)}) rotate(${deg.toFixed(2)}) scale(${s.toFixed(4)})`,
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
  const hand = hands.R
  const handAng = arms.R.screen
  const held = P.placement === 'held'
  const fromHand = (gx: number, gy: number, follow: number) => {
    const a = handAng * follow * DEG
    return {
      x: hand.x + gx * Math.cos(a) - gy * Math.sin(a),
      y: hand.y + gx * Math.sin(a) + gy * Math.cos(a),
    }
  }
  const hb = bbox(hull)
  const side = (k: number, extra: number) => hb.x1 + 26 * k + extra
  const midY = (hb.y0 + hb.y1) / 2
  addProp('blueprint', pose.blueprint, blueprintParts, (k) =>
    held
      ? {
          c: fromHand(-6 + sway * 2, -66 * k - 6 + bob * 0.5, 0.45),
          rot: handAng * 0.45 - 6 + sway * 3,
        }
      : { c: { x: side(k, 62 * k) + sway * 2, y: midY - 30 + bob - sway * 2 }, rot: -8 + sway * 3 },
  )
  // gripped tools: the handle runs through the fist (the ring end pokes out
  // below it), the head stands up and turns about the hand
  if (held)
    addProp(
      'wrench',
      pose.wrench,
      wrenchParts,
      () => ({ c: hand, rot: handAng * 0.3 + 8 + pose.wrenchTurn * 26 * amp }),
      { x: 0, y: 40 },
      1.35,
    )
  else
    addProp('wrench', pose.wrench, wrenchParts, (k) => ({
      c: { x: side(k, 48 * k) - sway * 2, y: midY - 20 - bob * 0.8 + sway * 2 },
      rot: 28 + pose.wrenchTurn * 22 * amp + sway * 2,
    }))
  // the magnifier is held by the end of its handle, lens up toward the face
  if (held)
    addProp(
      'glass',
      pose.glass,
      glassParts,
      () => ({ c: hand, rot: 32 + handAng * 0.12 + pose.glassDrift * 6 * amp }),
      { x: 37, y: 37 },
      1.6,
    )
  else
    addProp('glass', pose.glass, glassParts, (k) => ({
      c: { x: side(k, 40 * k) + pose.glassDrift * 4 * amp, y: midY - 70 + bob * 0.7 + sway * 2 },
      rot: -14 + sway * 5 + pose.glassDrift * 6 * amp,
    }))
  // the building blocks are bigger (user, round 5)
  addProp(
    'tiles',
    pose.tiles,
    () => tileParts(pose.tilesPhase),
    (k) =>
      held
        ? { c: fromHand(4 + bob * 0.3, -46 * k + sway * 2, 0.2), rot: sway * 3 }
        : { c: { x: side(k, 46 * k) + bob * 0.4, y: midY - 10 + sway * 3 }, rot: sway * 3 },
    undefined,
    1.65,
  )
  // the cap (off by default): hovers on the head between the ears
  addProp('cap', pose.cap, capParts, () => ({
    c: { x: headTop.x + sway * 2, y: headTop.y - 4 + bob * 0.6 },
    rot: tiltDeg * 0.8 + sway * 3,
  }))

  // ---- assemble -------------------------------------------------------------------------
  let bounds = hb
  for (const id of [
    'leafBig',
    'leafSmall',
    'armL-back',
    'armR-back',
    'armL-front',
    'armR-front',
    'footL',
    'footR',
  ]) {
    const n = nodes.get(id)
    if (n && n.d) {
      const nums = (n.d.match(/-?\d+(\.\d+)?/g) ?? []).map(Number)
      const pts: Vec[] = []
      for (let i = 0; i + 1 < nums.length; i += 2) pts.push({ x: nums[i], y: nums[i + 1] })
      bounds = unionBox(bounds, bbox(pts))
    }
  }
  for (const p of props) bounds = unionBox(bounds, p.box)

  let shut = 0
  for (const id of ['happy', 'closed', 'squeeze', 'line'] as const)
    shut += Math.max(0, pose[`g_${id}`])
  const ordered: SceneNode[] = NODE_IDS.map((id) => nodes.get(id) ?? { id, d: '', fill: 'none' })
  return {
    nodes: ordered,
    defs,
    props,
    bounds,
    hull,
    eyeMask: maskPoly,
    dims: { W: 300 * Wd, H: BODY_H * Hd },
    blinkable: clamp01(1 - shut),
    armZ: { L: arms.L.z, R: arms.R.z },
    anchors: {
      face: faceC,
      eyeL,
      eyeR,
      pupilL,
      pupilR,
      faceBox,
      props: propAnchors,
      handL: hands.L,
      handR: hands.R,
      joint: flex(NECK),
      feet: { L: lowest(footL), R: lowest(footR) },
      bottom: 0,
    },
  }
}

/** The scene for whichever character the design shows. */
export function buildScene(cfg: CharacterConfig, pose: Pose, opts: SceneOptions = {}): Scene {
  return cfg.character === 'cactus'
    ? buildCactusScene(cfg, pose, opts)
    : buildMonsterScene(cfg, pose, opts)
}

/** Rest framing of a design: the box the stage, previews and sheets centre on. */
export function restFrame(cfg: CharacterConfig): { cx: number; cy: number; w: number; h: number } {
  if (cfg.character === 'cactus') return cactusRestFrame(cfg)
  const top = -362 * cfg.body.height - 6
  const bottom = 4
  const w = 312 * cfg.body.width
  return { cx: 0, cy: (top + bottom) / 2, w, h: bottom - top }
}
