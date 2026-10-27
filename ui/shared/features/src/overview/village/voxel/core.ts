// eslint-disable-next-line @typescript-eslint/ban-ts-comment -- the lab's plain-JS rasterizer, kept untyped as copied (see below)
// @ts-nocheck
/*
 * The voxel rasterizer from the Nasiko village lab (`website/nasiko-village/js/engine.js`), copied below this header
 * (only its unused helpers removed): a software 2:1 isometric rasterizer writing colour, depth and place-id buffers. The painted world for the
 * village (three themes: medieval desert, ice age, steampunk) follows it in this same file: it writes the rasterizer's draw state, which a separate
 * module could not. Plain JS as in the lab, so the file is not type-checked.
 */

/* ---------- colour ---------- */
const RGB = (h) => {
  h = h.replace('#', '')
  return [parseInt(h.slice(0, 2), 16), parseInt(h.slice(2, 4), 16), parseInt(h.slice(4, 6), 16)]
}
const PACK = (r, g, b) => ((255 << 24) | ((b & 255) << 16) | ((g & 255) << 8) | (r & 255)) >>> 0
const UNR = (c) => c & 255,
  UNG = (c) => (c >>> 8) & 255,
  UNB = (c) => (c >>> 16) & 255
const TINT = [46, 36, 84] // shadows lean cool purple, like the reference cliffs
let FL = 1 // light of the face being drawn
function lit(c, L = FL) {
  const t = Math.max(0, 1 - L) * 0.42
  const r = c[0] * L * (1 - t) + TINT[0] * t,
    g = c[1] * L * (1 - t) + TINT[1] * t,
    b = c[2] * L * (1 - t) + TINT[2] * t
  return PACK(r > 255 ? 255 : r, g > 255 ? 255 : g, b > 255 ? 255 : b)
}
function mixP(c, d, t) {
  return PACK(
    UNR(c) + (UNR(d) - UNR(c)) * t,
    UNG(c) + (UNG(d) - UNG(c)) * t,
    UNB(c) + (UNB(d) - UNB(c)) * t,
  )
}

/* ---------- noise ---------- */
function h2(a, b) {
  let n = (Math.imul(a | 0, 374761393) + Math.imul(b | 0, 668265263)) | 0
  n = Math.imul(n ^ (n >>> 13), 1274126177)
  return ((n ^ (n >>> 16)) >>> 0) / 4294967296
}
function h3(a, b, c) {
  return h2(a + Math.imul(c | 0, 1013), b - Math.imul(c | 0, 7919))
}
const sm = (t) => t * t * (3 - 2 * t)
function vn(x, y) {
  const i = Math.floor(x),
    j = Math.floor(y),
    fx = sm(x - i),
    fy = sm(y - j)
  const a = h2(i, j),
    b = h2(i + 1, j),
    c = h2(i, j + 1),
    d = h2(i + 1, j + 1)
  return a + (b - a) * fx + (c - a) * fy + (a - b - c + d) * fx * fy
}
let VD1 = 0,
  VD2 = 0,
  VX = 0,
  VY = 0,
  VID = 0 // voronoi results (no allocation)
function vor(x, y, seed = 0) {
  const i = Math.floor(x),
    j = Math.floor(y)
  VD1 = 9
  VD2 = 9
  for (let a = -1; a <= 1; a++)
    for (let b = -1; b <= 1; b++) {
      const ci = i + a,
        cj = j + b
      const cx = ci + 0.15 + h3(ci, cj, seed) * 0.7,
        cy = cj + 0.15 + h3(cj, ci, seed + 7) * 0.7
      const d = (x - cx) * (x - cx) + (y - cy) * (y - cy)
      if (d < VD1) {
        VD2 = VD1
        VD1 = d
        VX = cx
        VY = cy
        VID = ci * 977 + cj
      } else if (d < VD2) VD2 = d
    }
  VD1 = Math.sqrt(VD1)
  VD2 = Math.sqrt(VD2)
}

/* ---------- buffer ---------- */
const K_VOID = 0,
  K_TERRAIN = 1,
  K_DECOR = 2,
  K_OBJ = 3
const RB = { W: 0, H: 0, OX: 0, OY: 0, col: null, z: null, id: null, kind: null }
let TW = 8,
  TH = 4,
  ZH = 8,
  RES = 1
function setRes(r) {
  RES = r
  TW = 8 * r
  TH = 4 * r
  ZH = 8 * r
}
function rbInit(W, H, OX, OY) {
  Object.assign(RB, { W, H, OX, OY })
  RB.col = new Uint32Array(W * H)
  RB.z = new Float32Array(W * H).fill(-1e9)
  RB.id = new Int32Array(W * H)
  RB.kind = new Uint8Array(W * H)
}
const sx = (x, y) => (x - y) * TW + RB.OX
const sy = (x, y, z) => (x + y) * TH - z * ZH + RB.OY

/* ---------- current draw state ---------- */
const CUR_BIAS = 0
let CUR_ID = 0,
  CUR_KIND = K_OBJ,
  FT = 'T',
  FTOP = 0
let BLEND = false,
  SA = 1,
  CYL_U = 0 // BLEND: translucent faces tint what is behind them (no depth write); shaders may set SA
function withId(id, kind, fn) {
  const a = CUR_ID,
    b = CUR_KIND
  CUR_ID = id
  CUR_KIND = kind
  fn()
  CUR_ID = a
  CUR_KIND = b
}

/* triangle with screen-space barycentrics (exact for an orthographic camera) */
function tri(a, b, c, ua, va, ub, vb, uc, vc, sh) {
  const W = RB.W,
    H = RB.H,
    Z = RB.z,
    COL = RB.col,
    ID = RB.id,
    KD = RB.kind
  const Ax = sx(a[0], a[1]),
    Ay = sy(a[0], a[1], a[2]),
    Bx = sx(b[0], b[1]),
    By = sy(b[0], b[1], b[2]),
    Cx = sx(c[0], c[1]),
    Cy = sy(c[0], c[1], c[2])
  const area = (Bx - Ax) * (Cy - Ay) - (By - Ay) * (Cx - Ax)
  if (Math.abs(area) < 1e-6) return
  const x0 = Math.max(0, Math.floor(Math.min(Ax, Bx, Cx))),
    x1 = Math.min(W - 1, Math.ceil(Math.max(Ax, Bx, Cx)))
  const y0 = Math.max(0, Math.floor(Math.min(Ay, By, Cy))),
    y1 = Math.min(H - 1, Math.ceil(Math.max(Ay, By, Cy)))
  const E = -1e-4,
    ia = 1 / area
  for (let py = y0; py <= y1; py++) {
    const cy = py + 0.5
    for (let px = x0; px <= x1; px++) {
      const cx = px + 0.5
      const w0 = ((Bx - cx) * (Cy - cy) - (By - cy) * (Cx - cx)) * ia
      if (w0 < E) continue
      const w1 = ((Cx - cx) * (Ay - cy) - (Cy - cy) * (Ax - cx)) * ia
      if (w1 < E) continue
      const w2 = 1 - w0 - w1
      if (w2 < E) continue
      const x = w0 * a[0] + w1 * b[0] + w2 * c[0],
        y = w0 * a[1] + w1 * b[1] + w2 * c[1],
        z = w0 * a[2] + w1 * b[2] + w2 * c[2]
      const d = x + y + z + CUR_BIAS,
        k = py * W + px
      if (d < Z[k] - 1e-4) continue
      const col = sh(x, y, z, w0 * ua + w1 * ub + w2 * uc, w0 * va + w1 * vb + w2 * vc, px, py)
      if (!col) continue
      if (BLEND) {
        COL[k] = mixP(COL[k], col, SA)
        ID[k] = CUR_ID
        KD[k] = CUR_KIND
        continue
      }
      COL[k] = col
      Z[k] = d
      ID[k] = CUR_ID
      KD[k] = CUR_KIND
    }
  }
}
const LIGHT = [0.2, 0.5, 0.8]
function faceLight(n) {
  return 0.52 + 0.6 * Math.max(0, n[0] * LIGHT[0] + n[1] * LIGHT[1] + n[2] * LIGHT[2])
}
function normalOf(A, B, D) {
  const ux = B[0] - A[0],
    uy = B[1] - A[1],
    uz = B[2] - A[2],
    vx = D[0] - A[0],
    vy = D[1] - A[1],
    vz = D[2] - A[2]
  let n = [uy * vz - uz * vy, uz * vx - ux * vz, ux * vy - uy * vx]
  const l = Math.hypot(n[0], n[1], n[2]) || 1
  n = n.map((q) => q / l)
  if (n[0] + n[1] + n[2] < 0) n = n.map((q) => -q)
  return n
}
/* planar quad A B C D, u runs A→B, v runs A→D, both in world units */
function quad(A, B, C, D, sh, ft = 'S') {
  const lu = Math.hypot(B[0] - A[0], B[1] - A[1], B[2] - A[2]),
    lv = Math.hypot(D[0] - A[0], D[1] - A[1], D[2] - A[2])
  const pFL = FL,
    pFT = FT
  FL = faceLight(normalOf(A, B, D))
  FT = ft
  tri(A, B, C, 0, 0, lu, 0, lu, lv, sh)
  tri(A, C, D, 0, 0, lu, lv, 0, lv, sh)
  FL = pFL
  FT = pFT
}
function triangle(A, B, C, sh, ft = 'S') {
  const lu = Math.hypot(B[0] - A[0], B[1] - A[1], B[2] - A[2])
  const pFL = FL,
    pFT = FT
  FL = faceLight(normalOf(A, B, C))
  FT = ft
  tri(
    A,
    B,
    C,
    0,
    0,
    lu,
    0,
    lu * 0.5,
    Math.hypot(C[0] - (A[0] + B[0]) / 2, C[1] - (A[1] + B[1]) / 2, C[2] - (A[2] + B[2]) / 2),
    sh,
  )
  FL = pFL
  FT = pFT
}

/* ---------- solids ---------- */
function box(x, y, z, w, d, h, m, o) {
  const top = m.top || m,
    L = m.left || m.side || m,
    R = m.right || m.side || m
  if (!o || !o.noTop)
    quad([x, y, z + h], [x + w, y, z + h], [x + w, y + d, z + h], [x, y + d, z + h], top, 'T')
  if (!o || !o.noL) {
    FTOP = z + h
    quad([x, y + d, z], [x + w, y + d, z], [x + w, y + d, z + h], [x, y + d, z + h], L, 'L')
  }
  if (!o || !o.noR) {
    FTOP = z + h
    quad([x + w, y + d, z], [x + w, y, z], [x + w, y, z + h], [x + w, y + d, z + h], R, 'R')
  }
}
/* hip roof: rectangle at z, ridge at z+h */
function hip(x, y, z, w, d, h, m, over = 0.18) {
  x -= over
  y -= over
  w += over * 2
  d += over * 2
  const A = [x, y, z],
    B = [x + w, y, z],
    C = [x + w, y + d, z],
    D = [x, y + d, z]
  if (Math.abs(w - d) < 0.01) {
    const ap = [x + w / 2, y + d / 2, z + h]
    triangle(A, B, ap, m)
    triangle(D, A, ap, m)
    triangle(C, D, ap, m)
    triangle(B, C, ap, m)
    return
  }
  if (w > d) {
    const r0 = [x + d / 2, y + d / 2, z + h],
      r1 = [x + w - d / 2, y + d / 2, z + h]
    quad(A, B, r1, r0, m)
    quad(D, C, r1, r0, m)
    triangle(D, A, r0, m)
    triangle(B, C, r1, m)
  } else {
    const r0 = [x + w / 2, y + w / 2, z + h],
      r1 = [x + w / 2, y + d - w / 2, z + h]
    quad(A, D, r1, r0, m)
    quad(B, C, r1, r0, m)
    triangle(A, B, r0, m)
    triangle(C, D, r1, m)
  }
}
/* gable roof; ridge runs along x ('x') or y ('y'); gable ends use wall material */
function gable(x, y, z, w, d, h, m, wall, axis = 'x', over = 0.16) {
  if (axis === 'x') {
    const ym = y + d / 2
    quad(
      [x - over, y - over, z],
      [x + w + over, y - over, z],
      [x + w + over, ym, z + h],
      [x - over, ym, z + h],
      m,
    )
    quad(
      [x - over, y + d + over, z],
      [x + w + over, y + d + over, z],
      [x + w + over, ym, z + h],
      [x - over, ym, z + h],
      m,
    )
    FTOP = z + h
    triangle([x + w, y + d, z], [x + w, y, z], [x + w, ym, z + h], wall, 'R')
  } else {
    const xm = x + w / 2
    quad(
      [x - over, y - over, z],
      [x - over, y + d + over, z],
      [xm, y + d + over, z + h],
      [xm, y - over, z + h],
      m,
    )
    quad(
      [x + w + over, y - over, z],
      [x + w + over, y + d + over, z],
      [xm, y + d + over, z + h],
      [xm, y - over, z + h],
      m,
    )
    FTOP = z + h
    triangle([x, y + d, z], [x + w, y + d, z], [xm, y + d, z + h], wall, 'L')
  }
}
function pyr(x, y, z, w, d, h, m) {
  hip(x, y, z, w, d, h, m, 0)
}
/* square beam between two points */
function beam(p, q, r, m) {
  const dx = q[0] - p[0],
    dy = q[1] - p[1],
    dz = q[2] - p[2]
  const l = Math.hypot(dx, dy, dz) || 1,
    t = [dx / l, dy / l, dz / l]
  const a = Math.abs(t[2]) < 0.9 ? [0, 0, 1] : [1, 0, 0]
  let u = [t[1] * a[2] - t[2] * a[1], t[2] * a[0] - t[0] * a[2], t[0] * a[1] - t[1] * a[0]]
  const ul = Math.hypot(...u)
  u = u.map((k) => (k / ul) * r)
  let v = [t[1] * u[2] - t[2] * u[1], t[2] * u[0] - t[0] * u[2], t[0] * u[1] - t[1] * u[0]]
  const vl = Math.hypot(...v)
  v = v.map((k) => (k / vl) * r)
  const c = [
    [1, 1],
    [-1, 1],
    [-1, -1],
    [1, -1],
  ].map(([s1, s2]) => [u[0] * s1 + v[0] * s2, u[1] * s1 + v[1] * s2, u[2] * s1 + v[2] * s2])
  for (let k = 0; k < 4; k++) {
    const c0 = c[k],
      c1 = c[(k + 1) % 4]
    quad(
      [p[0] + c0[0], p[1] + c0[1], p[2] + c0[2]],
      [q[0] + c0[0], q[1] + c0[1], q[2] + c0[2]],
      [q[0] + c1[0], q[1] + c1[1], q[2] + c1[2]],
      [p[0] + c1[0], p[1] + c1[1], p[2] + c1[2]],
      m,
    )
  }
}
/* smooth cylinder; side shaders get CYL_U = arc position of the segment start, so u + CYL_U runs around */
function cyl(cx, cy, z, r, h, m, seg = 32, o) {
  const side = m.side || m,
    top = m.top || m
  for (let k = 0; k < seg; k++) {
    const a0 = (k / seg) * 6.2832,
      a1 = ((k + 1) / seg) * 6.2832
    const p0 = [cx + Math.cos(a0) * r, cy + Math.sin(a0) * r],
      p1 = [cx + Math.cos(a1) * r, cy + Math.sin(a1) * r]
    if (o && o.front && Math.cos((a0 + a1) / 2) + Math.sin((a0 + a1) / 2) < -0.05) continue
    CYL_U = r * a0
    FTOP = z + h
    quad(
      [p0[0], p0[1], z],
      [p1[0], p1[1], z],
      [p1[0], p1[1], z + h],
      [p0[0], p0[1], z + h],
      side,
      'C',
    )
  }
  CYL_U = 0
  if (!(o && o.noTop))
    for (let k = 0; k < seg; k++) {
      const a0 = (k / seg) * 6.2832,
        a1 = ((k + 1) / seg) * 6.2832
      triangle(
        [cx + Math.cos(a0) * r, cy + Math.sin(a0) * r, z + h],
        [cx + Math.cos(a1) * r, cy + Math.sin(a1) * r, z + h],
        [cx, cy, z + h],
        top,
        'T',
      )
    }
}
function cone(cx, cy, z, r, h, m, seg = 32) {
  for (let k = 0; k < seg; k++) {
    const a0 = (k / seg) * 6.2832,
      a1 = ((k + 1) / seg) * 6.2832
    triangle(
      [cx + Math.cos(a0) * r, cy + Math.sin(a0) * r, z],
      [cx + Math.cos(a1) * r, cy + Math.sin(a1) * r, z],
      [cx, cy, z + h],
      m,
    )
  }
}
/* octagonal prism */
function oct(cx, cy, z, r, h, m, topM) {
  const pts = []
  for (let k = 0; k < 8; k++) {
    const a = ((k + 0.5) * Math.PI) / 4
    pts.push([cx + Math.cos(a) * r, cy + Math.sin(a) * r])
  }
  for (let k = 0; k < 8; k++) {
    const a = pts[k],
      b = pts[(k + 1) % 8]
    FTOP = z + h
    quad(
      [a[0], a[1], z],
      [b[0], b[1], z],
      [b[0], b[1], z + h],
      [a[0], a[1], z + h],
      m.side || m,
      'L',
    )
  }
  const tm = topM || m.top || m
  for (let k = 0; k < 8; k++) {
    const a = pts[k],
      b = pts[(k + 1) % 8]
    triangle([a[0], a[1], z + h], [b[0], b[1], z + h], [cx, cy, z + h], tm, 'T')
  }
}
/* stepped dome made from shrinking octagons */
function dome(cx, cy, z, r, h, m, steps = 5) {
  for (let k = 0; k < steps; k++) {
    const t0 = k / steps,
      t1 = (k + 1) / steps
    const rr = r * Math.cos((t0 * Math.PI) / 2)
    oct(
      cx,
      cy,
      z + h * Math.sin((t0 * Math.PI) / 2),
      rr,
      h * (Math.sin((t1 * Math.PI) / 2) - Math.sin((t0 * Math.PI) / 2)) + 0.01,
      m,
    )
  }
}

/* ---------- billboards: pixel sprites standing in the world ---------- */
/* fn(ix, iy) → packed colour or 0. Anchored bottom-centre at world (x,y,z). */

/* ---------- post: dark silhouette outline inside objects ---------- */
function outlinePass(ink = PACK(34, 26, 52), amt = 0.55, terrain = false) {
  const { W, H, col, z, id, kind } = RB,
    out = new Uint32Array(col)
  for (let py = 1; py < H - 1; py++)
    for (let px = 1; px < W - 1; px++) {
      const k = py * W + px
      if (kind[k] !== K_OBJ && kind[k] !== K_DECOR && !(terrain && kind[k] === K_TERRAIN)) continue
      const me = id[k],
        d = z[k]
      let edge = false
      for (const n of [k - 1, k + 1, k - W, k + W]) {
        if (kind[n] === K_VOID || ((id[n] !== me || kind[k] === K_TERRAIN) && z[n] < d - 0.6)) {
          edge = true
          break
        }
      }
      if (edge) out[k] = mixP(col[k], ink, amt)
    }
  RB.col = out
}

/* ======================================================================================================== */
/* The village's painted voxel worlds, drawn with the lab's rasterizer above, one tile = one voxel tile at   */
/* RES 3 (the SVG's 48 × 24 px). Three themes share the terrain and the places; each brings its palette and  */
/* its own drawings where the place should look different: medieval desert, ice age, steampunk factory.    */
/* ======================================================================================================== */

const shadeC = (c, a) => (a < 0 ? c.map((v) => v * (1 + a)) : c.map((v) => v + (255 - v) * a))
/** A colour pulled toward another by k (team colours toned to sit in a world). */
const mixC = (c, d, k) => c.map((v, n) => v + (d[n] - v) * k)
const solid = (c) => () => lit(c)
const C3 = (h) => RGB(h)
const P3 = (a) => a.map(RGB)
function shingles(c) {
  const c1 = shadeC(c, -0.1)
  return (x, y, z, u, v) => lit(v % 0.9 < 0.09 ? c1 : c)
}
/** Brick courses on a wall (u, v in world units on the face). */
function bricks(c, mortar) {
  const c1 = shadeC(c, -0.08)
  return (x, y, z, u, v) => {
    const r = Math.floor(v / 0.18),
      off = (r & 1) * 0.16,
      q = Math.floor((u + off) / 0.32)
    if (v / 0.18 - r < 0.16 || (u + off) / 0.32 - q < 0.07) return lit(mortar)
    return lit(h2(r, q) < 0.3 ? c1 : c)
  }
}
/** Vertical planks. */
function planks(c) {
  const c1 = shadeC(c, -0.14)
  return (x, y, z, u) => lit(u % 0.22 < 0.035 ? c1 : c)
}
/** Riveted metal panels. */
function panels(c) {
  const c1 = shadeC(c, -0.18),
    c2 = shadeC(c, 0.25)
  return (x, y, z, u, v) => {
    const fu = u % 0.6,
      fv = v % 0.5
    if (fu < 0.03 || fv < 0.03) return lit(c1)
    if (
      (fu < 0.1 || fu > 0.52) &&
      (fv < 0.09 || fv > 0.42) &&
      h2(Math.floor(u * 20), Math.floor(v * 20)) < 0.5
    )
      return lit(c2)
    return lit(c)
  }
}
/** A band of colour round a wall, between heights v0 and v1 above its base. */

/* ---------- the themes ---------- */
const THEMES = {
  desert: {
    ground: P3(['#ddd3bd', '#d6cbb3', '#e3dac6', '#d0c5ac']),
    groundLine: C3('#c7bba1'),
    side: C3('#b8ab92'),
    patch: [C3('#f2b48c'), C3('#eb9b66'), C3('#d07f4f'), C3('#e08a57')],
    patchAt: 0.5,
    path: C3('#cdbf9f'),
    slab: P3(['#ebe5d5', '#e3dccb', '#f1ecdf']),
    slabLine: C3('#c9bfa9'),
    stone: P3(['#7b828c', '#868e98', '#737a85', '#9098a2', '#a2a9b2']),
    stoneLine: C3('#5f6670'),
    cap: P3(['#8a6a4f', '#9a7a5c', '#7a5c44']),
    pebble: P3(['#8b8f97', '#a9aab0', '#f2efe6', '#6f7480']),
    yard: true,
    shadow: [70, 78, 92],
    ink: [38, 52, 56],
    wall: C3('#e4cca2'),
    wallD: C3('#cbb085'),
    roof: C3('#cf6a40'),
    wood: C3('#8a5a36'),
    woodD: C3('#64402a'),
    woodL: C3('#a8744a'),
    metal: C3('#4b5466'),
    metalL: C3('#6b7588'),
    trim: C3('#f3c444'),
    plinth: C3('#c9c2b0'),
    plinthD: C3('#a39c8b'),
    water: C3('#4fa9d1'),
    waterL: C3('#9fdcf0'),
    waterD: C3('#357fb0'),
    leaf: P3(['#a3a86b', '#e39a5c', '#b9b178']),
    mesas: [],
  },
  ice: {
    ground: P3(['#eef3f8', '#e7eef5', '#f6f9fc', '#e1e9f1']),
    groundLine: C3('#d2dce8'),
    side: C3('#a9bccf'),
    patch: [C3('#d4e5f2'), C3('#bcd7ec'), C3('#a3c6e0'), C3('#93b9d8')],
    patchAt: 0.56,
    path: C3('#a8927d'),
    slab: P3(['#c9b9a8', '#bfae9c', '#d3c4b4']),
    slabLine: C3('#9c8a78'),
    stone: P3(['#a7c2d9', '#b6cfe3', '#98b4cf', '#c6daea', '#d5e6f2']),
    stoneLine: C3('#7d9ab6'),
    cap: P3(['#f4f7fa', '#e9eff6', '#dde6ef']),
    pebble: P3(['#7f8894', '#a3acb6', '#ffffff', '#5f6873']),
    yard: false,
    shadow: [70, 96, 130],
    ink: [40, 56, 78],
    wall: C3('#e6d8bc'),
    wallD: C3('#cdbd9e'),
    roof: C3('#3f6fb2'),
    wood: C3('#7b5a3e'),
    woodD: C3('#4a3424'),
    woodL: C3('#9c7652'),
    metal: C3('#3b3b3d'),
    metalL: C3('#57585c'),
    trim: C3('#c9a34a'),
    plinth: C3('#9aa7b6'),
    plinthD: C3('#7d8a99'),
    water: C3('#bfe2f2'),
    waterL: C3('#ffffff'),
    waterD: C3('#8cc3dd'),
    leaf: P3(['#2f5e4b', '#3c6b55', '#28503f']),
    mesas: [],
  },
  steam: {
    // A Victorian works on a steel deck: grey plates, brass walkways, red brick with cream stone, slate, verdigris.
    ground: P3(['#8a9099', '#838a93', '#9097a0', '#7d848d']),
    groundLine: C3('#636a73'),
    side: C3('#454b54'),
    patch: [C3('#7b828b'), C3('#737a83'), C3('#6a7179'), C3('#61676f')],
    patchAt: 0.64,
    path: C3('#c9a15a'),
    slab: P3(['#575d65', '#5f656d', '#51575f']),
    slabLine: C3('#353a40'),
    stone: P3(['#a4553a', '#ae5f43', '#98502f', '#b36848', '#9c5034']),
    stoneLine: C3('#5a2c1d'),
    cap: P3(['#3a3f46', '#454b53', '#33383e']),
    pebble: P3(['#d4a24c', '#e6bf6e', '#9a7638', '#7a5c2c']),
    yard: true,
    plates: true,
    shadow: [26, 30, 40],
    ink: [34, 38, 48],
    wall: C3('#a4553a'),
    wallD: C3('#86432d'),
    roof: C3('#56657a'),
    wood: C3('#6b4a33'),
    woodD: C3('#3f2a1d'),
    woodL: C3('#8a6444'),
    metal: C3('#3d434b'),
    metalL: C3('#5a616b'),
    trim: C3('#d4a24c'),
    plinth: C3('#8d949d'),
    plinthD: C3('#6a717a'),
    brass: C3('#d4a24c'),
    copper: C3('#c8794a'),
    verdigris: C3('#6fae96'),
    amber: C3('#ffc96e'),
    cream: C3('#e3d8c3'),
    water: C3('#6fae96'),
    waterL: C3('#a6d4c2'),
    waterD: C3('#4f8a74'),
    leaf: P3(['#5d8a77', '#6fae96', '#4f7d6b']),
    mesas: [],
  },
}
let THM = THEMES.desert

/* windows, doors and signs on thin boxes (the lab's) */
let WIN_W = 0.5,
  WIN_D = 0.5,
  WIN_H = 0.8
function winM(frame, glass = C3('#3d5d8c'), glint = C3('#a8cdee')) {
  return (x, y, z, u, v) => {
    if (FT === 'T') return lit(frame)
    const w = FT === 'L' ? WIN_W : WIN_D
    if (u < 0.08 || v < 0.08 || u > w - 0.08 || v > WIN_H - 0.08) return lit(frame)
    return lit(
      u + (WIN_H - v) * 0.6 < w * 0.55 && u + (WIN_H - v) * 0.6 > w * 0.3 ? glint : glass,
      glass === AMBER_GLASS ? 1.15 : FL,
    )
  }
}
const AMBER_GLASS = C3('#ffc96e')
function windowL(x, y, z, w, h, m) {
  WIN_W = w
  WIN_H = h
  box(x, y - 0.02, z, w, 0.06, h, m || winM(THM.woodD), { noR: true })
}
function windowR(x, y, z, d, h, m) {
  WIN_D = d
  WIN_H = h
  box(x - 0.02, y, z, 0.06, d, h, m || winM(THM.woodD), { noL: true })
}
function doorM(c) {
  const c1 = shadeC(c, -0.25)
  return (x, y, z, u, v) => {
    const w = FT === 'L' ? WIN_W : WIN_D
    if (u < 0.07 || u > w - 0.07 || v > WIN_H - 0.08) return lit(THM.woodD)
    return lit(Math.abs(u - w / 2) < 0.04 ? c1 : c)
  }
}
function doorL(x, y, z, w, h, c) {
  WIN_W = w
  WIN_H = h
  box(x, y - 0.02, z, w, 0.06, h, doorM(c || THM.wood), { noR: true })
}
const GLYPH = {
  A: ['01110', '10001', '11111', '10001', '10001'],
  O: ['01110', '10001', '10001', '10001', '01110'],
  G: ['01111', '10000', '10011', '10001', '01110'],
  M: ['10001', '11011', '10101', '10001', '10001'],
  C: ['01111', '10000', '10000', '10000', '01111'],
  X: ['10001', '01010', '00100', '01010', '10001'],
  plus: ['00100', '00100', '11111', '00100', '00100'],
  gear: ['10101', '01110', '11011', '01110', '10101'],
  clock: ['01110', '10101', '10111', '10001', '01110'],
  coin: ['01110', '11011', '11011', '11011', '01110'],
  lens: ['01110', '11011', '10101', '11011', '01110'],
}
function signM(bg, fg, glyph, frame) {
  const g = GLYPH[glyph]
  return (x, y, z, u, v) => {
    if (FT === 'T') return lit(frame || THM.woodD)
    const w = FT === 'L' ? WIN_W : WIN_D
    if (u < 0.07 || v < 0.07 || u > w - 0.07 || v > WIN_H - 0.07) return lit(frame || THM.woodD)
    if (g) {
      const s = Math.min(w, WIN_H) * 0.16
      const gx = Math.floor((u - (w - s * 5) / 2) / s),
        gy = 4 - Math.floor((v - (WIN_H - s * 5) / 2) / s)
      if (gx >= 0 && gx < 5 && gy >= 0 && gy < 5 && g[gy][gx] === '1') return lit(fg)
    }
    return lit(bg)
  }
}
function signL(x, y, z, w, h, bg, fg, glyph, frame) {
  WIN_W = w
  WIN_H = h
  box(x, y - 0.02, z, w, 0.08, h, signM(bg, fg, glyph, frame), { noR: true })
}
function signR(x, y, z, d, h, bg, fg, glyph, frame) {
  WIN_D = d
  WIN_H = h
  box(x - 0.02, y, z, 0.08, d, h, signM(bg, fg, glyph, frame), { noL: true })
}

/* ---------- layout: this village's land, an organic coast round it, pillars behind ---------- */
const GI = -12,
  GJ = -12,
  NI = 54,
  NJ = 44
const HT_ = new Float32Array(NI * NJ),
  MAT_ = new Uint8Array(NI * NJ)
const M_SAND = 1,
  M_COBBLE = 2,
  M_ROCK = 3
const inG = (i, j) => i >= GI && j >= GJ && i < GI + NI && j < GJ + NJ
const HT = (i, j) => (inG(i, j) ? HT_[(j - GJ) * NI + (i - GI)] : 0)
const MT = (i, j) => (inG(i, j) ? MAT_[(j - GJ) * NI + (i - GI)] : 0)
let L = null
function rectDist(x, y, r) {
  return Math.hypot(Math.max(r.i - x, 0, x - (r.i + r.w)), Math.max(r.j - y, 0, y - (r.j + r.d)))
}
function rectBox(x, y, r) {
  return Math.max(r.i - x, 0, x - (r.i + r.w), r.j - y, y - (r.j + r.d))
}
let LM = 0
function landAt(x, y) {
  LM = 0
  let h = 0
  // The steampunk deck is a straight-edged platform (square distance); the other worlds have an organic coast.
  const dist = THM.plates ? rectBox : rectDist
  const d = Math.min(dist(x, y, L.main), dist(x, y, L.outside))
  const margin = THM.plates
    ? 1.25
    : 1.5 + vn(x * 0.17 + 3.1, y * 0.17 - 2.3) * 2.6 + vn(x * 0.55, y * 0.55 + 7) * 0.6
  if (d < margin) {
    h = 1
    LM = M_SAND
  }
  if (!THM.plates)
    for (const [lx, ly, lr] of L.lobes) {
      const a = Math.atan2(y - ly, x - lx),
        r = lr * (1 + 0.16 * Math.sin(3 * a + lx) + 0.08 * Math.sin(7 * a))
      if (Math.hypot(x - lx, y - ly) < r) {
        h = 1
        LM = M_SAND
      }
    }
  if (!THM.plates)
    for (const [hx, hy, hr] of L.holes)
      if (
        Math.hypot(x - hx, (y - hy) * 1.2) <
        hr * (1 + 0.15 * Math.sin(5 * Math.atan2(y - hy, x - hx)))
      ) {
        h = 0
        LM = 0
      }
  for (const c of L.cobble)
    if (h && x >= c.i && x < c.i + c.w && y >= c.j && y < c.j + c.d) LM = M_COBBLE
  for (const [mx, my, mr, mh] of THM.mesas) {
    const a = Math.atan2(y - my, x - mx),
      r = mr * (1 + 0.12 * Math.sin(4 * a + mx) + 0.06 * Math.sin(9 * a))
    if (Math.hypot(x - mx, y - my) < r) {
      h = mh
      LM = M_ROCK
    }
  }
  return h
}
function genTerrain() {
  HT_.fill(0)
  MAT_.fill(0)
  for (let j = GJ; j < GJ + NJ; j++)
    for (let i = GI; i < GI + NI; i++) {
      const h = landAt(i + 0.5, j + 0.5)
      if (h) {
        HT_[(j - GJ) * NI + (i - GI)] = h
        MAT_[(j - GJ) * NI + (i - GI)] = LM
      }
    }
}
const SEGS = []
function bakeSegs() {
  SEGS.length = 0
  for (const [a, b, w] of L.roads)
    SEGS.push({
      ax: a[0],
      ay: a[1],
      bx: b[0],
      by: b[1],
      w,
      x0: Math.min(a[0], b[0]) - w - 0.3,
      x1: Math.max(a[0], b[0]) + w + 0.3,
      y0: Math.min(a[1], b[1]) - w - 0.3,
      y1: Math.max(a[1], b[1]) + w + 0.3,
    })
}
function roadDist(x, y) {
  let best = 9
  for (const s of SEGS) {
    if (x < s.x0 || x > s.x1 || y < s.y0 || y > s.y1) continue
    const vx = s.bx - s.ax,
      vy = s.by - s.ay,
      l2 = vx * vx + vy * vy || 1
    const t = Math.max(0, Math.min(1, ((x - s.ax) * vx + (y - s.ay) * vy) / l2))
    best = Math.min(best, Math.hypot(x - s.ax - vx * t, y - s.ay - vy * t) - s.w)
  }
  return best
}

/* ---------- terrain shaders ---------- */
function blobT(x, y) {
  return (
    vn(x * 0.13 + 3.7, y * 0.13 - 1.9) * 0.72 +
    vn(x * 0.42, y * 0.42 + 5) * 0.2 +
    vn(x * 1.6, y * 1.6) * 0.08
  )
}
let FMAT = 0,
  FVOID = false
function paintCap(x, y) {
  vor(x * 0.9, y * 0.9, 31)
  if (VD2 - VD1 < 0.035) return lit(THM.cap[2])
  return lit(THM.cap[vn(x * 0.5, y * 0.5) > 0.55 ? 1 : h2(VID, 1) < 0.5 ? 0 : 2])
}
function paintStone(x, y, z) {
  const hc = FT === 'L' ? x : FT === 'R' ? y : x + y
  vor(hc * 0.95 + 11, z * 0.75, 17)
  if (VD2 - VD1 < 0.025) return lit(THM.stoneLine)
  return lit(THM.stone[Math.floor(h2(VID, 8) * 4.99)])
}
function groundCell(x, y) {
  if (THM.plates) {
    const fx = x - Math.floor(x),
      fy = y - Math.floor(y)
    if (fx < 0.035 || fy < 0.035) return lit(THM.groundLine)
    const cx = fx < 0.5 ? 0.1 : 0.9,
      cy = fy < 0.5 ? 0.1 : 0.9
    if (Math.hypot(fx - cx, fy - cy) < 0.035) return lit(THM.pebble[1])
    return lit(THM.ground[Math.floor(h2(Math.floor(x), Math.floor(y)) * 3.99)])
  }
  vor(x * 0.75, y * 0.75, 23)
  if (VD2 - VD1 < 0.03) return lit(THM.groundLine)
  return lit(THM.ground[Math.floor(h2(VID, 3) * 3.99)])
}
function terrTop(x, y, z) {
  const i = Math.floor(x - 1e-6),
    j = Math.floor(y - 1e-6),
    m = FMAT || MT(i, j),
    h = FMAT ? z : HT(i, j)
  if (m === M_COBBLE) {
    if (THM.plates) {
      // Steel grating: a fine grid.
      const fx = (x * 4) % 1,
        fy = (y * 4) % 1
      return lit(
        fx < 0.18 || fy < 0.18
          ? THM.slabLine
          : THM.slab[Math.floor(h2(Math.floor(x * 2), Math.floor(y * 2)) * 2.99)],
      )
    }
    vor(x * 1.15, y * 1.15, 11)
    if (VD2 - VD1 < 0.05) return lit(THM.slabLine)
    return lit(THM.slab[Math.floor(h2(VID, 5) * 2.99)])
  }
  if (m === M_ROCK || h >= 2) return paintCap(x, y)
  const edge = HT(i + 1, j) < h || HT(i - 1, j) < h || HT(i, j + 1) < h || HT(i, j - 1) < h
  if (edge && !THM.plates) {
    vor(x * 3.2, y * 3.2, 41)
    if (VD1 < 0.13 && h2(VID, 2) < 0.28) return lit(THM.pebble[Math.floor(h2(VID, 6) * 3.99)])
  }
  if (edge && THM.plates) {
    const e = Math.min(x - Math.floor(x), y - Math.floor(y))
    if (e < 0.1) return lit(THM.pebble[0])
  }
  const road = roadDist(x, y)
  if (road < -0.05) {
    if (THM.plates) {
      const f = ((x + y) * 3) % 1
      return lit(f < 0.12 ? shadeC(THM.path, -0.25) : THM.path)
    }
    return lit(THM.path)
  }
  if (road < 0.03) return lit(THM.groundLine)
  const yard =
    THM.yard &&
    x >= L.outside.i &&
    x < L.outside.i + L.outside.w &&
    y >= L.outside.j &&
    y < L.outside.j + L.outside.d
  const at = THM.patchAt,
    t = yard ? at + 0.03 + blobT(x, y) * 0.12 : blobT(x, y)
  const [p0, p1, pl, pd] = THM.patch
  if (t > at + 0.1) return lit(t > at + 0.14 ? pd : pl)
  if (t > at) return lit(t > at + 0.012 ? (t > at + 0.06 ? p1 : p0) : pl)
  return groundCell(x, y)
}
function cliff(x, y, z) {
  if (FVOID || FTOP <= 1.01) {
    if (THM.plates) {
      const hc = FT === 'L' ? x : y
      return lit((hc * 3) % 1 < 0.08 ? THM.pebble[0] : z > FTOP - 0.08 ? THM.pebble[1] : THM.side)
    }
    return lit(THM.side)
  }
  return paintStone(x, y, z)
}

const SUB = 8
function subH(gx, gy) {
  return landAt((gx + 0.5) / SUB, (gy + 0.5) / SUB)
}
function tileUniform(i, j) {
  const h = HT(i, j),
    m = MT(i, j)
  for (let b = -1; b <= 1; b++)
    for (let a = -1; a <= 1; a++) if (HT(i + a, j + b) !== h || MT(i + a, j + b) !== m) return false
  return true
}
function drawTerrain() {
  CUR_ID = 0
  CUR_KIND = K_TERRAIN
  const depth = THM.plates ? 0.85 : 0.45
  for (let j = GJ; j < GJ + NJ; j++)
    for (let i = GI; i < GI + NI; i++) {
      let any = false
      for (let b = -1; b <= 1 && !any; b++)
        for (let a = -1; a <= 1; a++)
          if (HT(i + a, j + b)) {
            any = true
            break
          }
      if (!any) continue
      const h = HT(i, j)
      if (h && tileUniform(i, j)) {
        FMAT = 0
        quad([i, j, h], [i + 1, j, h], [i + 1, j + 1, h], [i, j + 1, h], terrTop, 'T')
        continue
      }
      for (let b = 0; b < SUB; b++)
        for (let a = 0; a < SUB; a++) {
          const gx = i * SUB + a,
            gy = j * SUB + b,
            hh = subH(gx, gy)
          if (!hh) continue
          const mat = LM,
            x0 = gx / SUB,
            y0 = gy / SUB,
            x1 = x0 + 1 / SUB,
            y1 = y0 + 1 / SUB
          FMAT = mat
          quad([x0, y0, hh], [x1, y0, hh], [x1, y1, hh], [x0, y1, hh], terrTop, 'T')
          FMAT = 0
          const hx = subH(gx + 1, gy),
            hy = subH(gx, gy + 1)
          if (hx < hh) {
            FVOID = !hx
            const zb = hx || hh - depth
            FTOP = hh
            quad([x1, y1, zb], [x1, y0, zb], [x1, y0, hh], [x1, y1, hh], cliff, 'R')
          }
          if (hy < hh) {
            FVOID = !hy
            const zb = hy || hh - depth
            FTOP = hh
            quad([x0, y1, zb], [x1, y1, zb], [x1, y1, hh], [x0, y1, hh], cliff, 'L')
          }
        }
    }
  FVOID = false
  FMAT = 0
}

/* ---------- shadows ---------- */
let SHADOWS = []
const caster = (x, y, z, w, d, h) => SHADOWS.push({ x, y, z, w, d, h })
function hull(pts) {
  pts = pts.slice().sort((a, b) => a[0] - b[0] || a[1] - b[1])
  const cr = (o, a, b) => (a[0] - o[0]) * (b[1] - o[1]) - (a[1] - o[1]) * (b[0] - o[0])
  const lo = [],
    up_ = []
  for (const p of pts) {
    while (lo.length >= 2 && cr(lo[lo.length - 2], lo[lo.length - 1], p) <= 0) lo.pop()
    lo.push(p)
  }
  for (const p of pts.slice().reverse()) {
    while (up_.length >= 2 && cr(up_[up_.length - 2], up_[up_.length - 1], p) <= 0) up_.pop()
    up_.push(p)
  }
  return lo.slice(0, -1).concat(up_.slice(0, -1))
}
function shadowPass() {
  const { W, H, col, kind } = RB,
    mask = new Uint8Array(W * H)
  const fill = (poly) => {
    const xs = poly.map((p) => p[0]),
      ys = poly.map((p) => p[1])
    const x0 = Math.max(0, Math.floor(Math.min(...xs))),
      x1 = Math.min(W - 1, Math.ceil(Math.max(...xs))),
      y0 = Math.max(0, Math.floor(Math.min(...ys))),
      y1 = Math.min(H - 1, Math.ceil(Math.max(...ys)))
    for (let py = y0; py <= y1; py++)
      for (let px = x0; px <= x1; px++) {
        let inside = true
        for (let k = 0; k < poly.length; k++) {
          const a = poly[k],
            b = poly[(k + 1) % poly.length]
          if ((b[0] - a[0]) * (py + 0.5 - a[1]) - (b[1] - a[1]) * (px + 0.5 - a[0]) < 0) {
            inside = false
            break
          }
        }
        if (inside) mask[py * W + px] = 1
      }
  }
  SHADOWS.forEach((s) => {
    const off = [-0.22 * s.h, -0.62 * s.h],
      P = []
    ;[
      [0, 0],
      [s.w, 0],
      [s.w, s.d],
      [0, s.d],
    ].forEach(([a, c]) => {
      P.push([sx(s.x + a, s.y + c), sy(s.x + a, s.y + c, s.z)])
      P.push([sx(s.x + a + off[0], s.y + c + off[1]), sy(s.x + a + off[0], s.y + c + off[1], s.z)])
    })
    fill(hull(P))
  })
  const sc = PACK(...THM.shadow)
  for (let k = 0; k < W * H; k++)
    if (mask[k] && kind[k] === K_TERRAIN) col[k] = mixP(col[k], sc, THM.plates ? 0.3 : 0.22)
}

/* ---------- shared places ---------- */
const G = 1 // the land's top: the SVG's ground (z 0 there) is z 1 here
const U = (px) => px / 24

/** A glass token container (the lab's treasury): gold coins stacked to `fill`, a gold budget mark, a lid. */
function tokenTank(cx, cy, R, H, fill, over, opts = {}) {
  const g = G,
    metal = THM.metal,
    metalL = THM.metalL,
    trim = THM.trim
  caster(cx - R * 0.95, cy - R * 0.95, g, R * 1.9, R * 1.9, H + 0.8)
  cyl(cx, cy, g, R + 0.22, 0.25, { side: solid(THM.plinthD), top: solid(THM.plinth) })
  const z0 = g + 0.25
  cyl(cx, cy, z0, R + 0.08, 0.28, {
    side: (x, y, z, u, v) => lit(v > 0.2 ? trim : metal),
    top: solid(metalL),
  })
  const zg = z0 + 0.28,
    fh = Math.max(0.05, Math.min(1, fill)) * H * 0.92
  const coin = over ? C3('#f08a3c') : C3('#f3c444')
  const stackSide = (x, y, z, u, v) => {
    const Uu = CYL_U + u,
      col = Math.floor(Uu / 0.3),
      off = (col & 1) * 0.08,
      row = Math.floor((v + off) / 0.16)
    if ((v + off) / 0.16 - row < 0.3) return lit(shadeC(coin, -0.3))
    if (Uu / 0.3 - col < 0.12) return lit(shadeC(coin, -0.16))
    return lit(h2(col, row) < 0.25 ? shadeC(coin, 0.14) : coin)
  }
  const stackTop = (x, y) => {
    vor(x * 3.2, y * 3.2, 7)
    if (VD1 > 0.36) return lit(shadeC(coin, -0.36))
    return lit(x * 3.2 - VX + (y * 3.2 - VY) < -0.1 ? shadeC(coin, 0.3) : coin)
  }
  cyl(cx, cy, zg, R - 0.08, fh, { side: stackSide, top: stackTop }, 36)
  const budZ = zg + H * 0.8
  const glass = (x, y, z, u, v) => {
    const Uu = (CYL_U + u) / (2 * Math.PI * R)
    SA = 0.2
    if (v > H - 0.08 || v < 0.06) {
      SA = 0.55
      return lit(C3('#e9f6fb'), 1)
    }
    if (Math.abs(z - budZ) < 0.06) {
      SA = 1
      return lit(over ? C3('#e5484d') : C3('#f3c444'), 1)
    }
    if ((Uu > 0.08 && Uu < 0.1) || (Uu > 0.115 && Uu < 0.125)) {
      SA = 0.6
      return lit(C3('#ffffff'), 1)
    }
    return lit(C3('#cdeef8'), 1)
  }
  BLEND = true
  cyl(cx, cy, zg, R, H, { side: glass }, 36, { noTop: true })
  BLEND = false
  SA = 1
  const zl = zg + H
  cyl(cx, cy, zl, R + 0.12, 0.22, {
    side: (x, y, z, u, v) => lit(v < 0.07 ? trim : metal),
    top: solid(metalL),
  })
  cone(cx, cy, zl + 0.22, R + 0.02, R * 0.42, solid(metal))
  cyl(
    cx,
    cy,
    zl + 0.22 + R * 0.4,
    R * 0.16,
    0.14,
    { side: solid(trim), top: solid(shadeC(trim, 0.4)) },
    14,
  )
  ;[0.15, 1.2, 2.25, 3.3, 4.35, 5.4].forEach((a) => {
    const x = cx + Math.cos(a) * (R + 0.03),
      y = cy + Math.sin(a) * (R + 0.03)
    if (Math.cos(a) + Math.sin(a) > -0.3) box(x - 0.05, y - 0.05, zg, 0.1, 0.1, H, solid(metal))
  })
  if (opts.flag) flag(cx, cy, zl + 0.22 + R * 0.5, opts.flag, 1.1)
}
function flag(x, y, z, color, h = 1.2) {
  beam([x, y, z], [x, y, z + h], 0.035, solid(THM.woodD))
  const c = RGB(color),
    cd = shadeC(c, -0.2)
  quad(
    [x, y, z + h - 0.05],
    [x + 0.55, y - 0.18, z + h - 0.2],
    [x + 0.55, y - 0.18, z + h - 0.38],
    [x, y, z + h - 0.5],
    (q, w, e, u, v) => lit(v > 0.22 ? cd : c),
    'L',
  )
}
function rockMound(x, y, g, w, d, h) {
  box(x, y, g, w, d, h, (q, w2, e) => (FT === 'T' ? paintCap(q, w2) : paintStone(q, w2, e)))
}
/** A provider's mine: a stone mound with crystals in its colour, a timber headframe and a sign with its letter. */
function mineRock(m, small = false) {
  const k = m.w / 3,
    x = m.i,
    y = m.j,
    g = G,
    col = RGB(m.color),
    colL = shadeC(col, 0.5)
  caster(x + 0.2 * k, y + 0.2 * k, g, 2.6 * k, 2.4 * k, 3 * k)
  rockMound(x + 0.15 * k, y + 0.1 * k, g, 2.7 * k, 2.6 * k, 0.9 * k)
  rockMound(x + 0.35 * k, y + 0.25 * k, g + 0.9 * k, 2.2 * k, 1.9 * k, 0.7 * k)
  if (!small) rockMound(x + 0.7 * k, y + 0.45 * k, g + 1.6 * k, 1.4 * k, 1.25 * k, 0.5 * k)
  const top = small ? 1.6 : 2.1
  ;[
    [1.05, 0.75, top, 0.28, 0.75],
    [1.55, 0.95, top, 0.22, 0.55],
    [1.3, 1.2, top, 0.18, 0.42],
    [2.2, 0.55, 1.6, 0.2, 0.5],
    [0.55, 1.75, 1.6, 0.2, 0.45],
  ].forEach(([a, c, zz, s_, h]) => {
    box(x + a * k, y + c * k, g + zz * k, s_ * k, s_ * k, h * k, {
      top: solid(colL),
      side: (q, w2, e, u, v, px, py) => lit(h2(px, py) < 0.3 ? colL : col, Math.max(FL, 0.95)),
    })
    pyr(x + a * k, y + c * k, g + (zz + h) * k, s_ * k, s_ * k, s_ * 0.9 * k, solid(colL))
  })
  if (small) return
  const ey = y + 2.7 * k
  box(
    x + 0.85 * k,
    ey,
    g,
    1.3 * k,
    0.12 * k,
    1.15 * k,
    (q, w2, e, u, v) =>
      lit(u < 0.16 * k || u > 1.14 * k || v > 0.95 * k ? THM.woodD : C3('#2a2238')),
    { noR: true },
  )
  ;[0.82, 2.06].forEach((a) =>
    box(x + a * k, ey - 0.05 * k, g, 0.13 * k, 0.2 * k, 1.25 * k, solid(THM.wood)),
  )
  box(x + 0.75 * k, ey - 0.06 * k, g + 1.18 * k, 1.5 * k, 0.24 * k, 0.14 * k, solid(THM.woodL))
  beam(
    [x + 0.95 * k, ey + 0.05 * k, g + 1.3 * k],
    [x + 1.5 * k, ey - 0.1 * k, g + 2.55 * k],
    0.06 * k,
    solid(THM.wood),
  )
  beam(
    [x + 2.05 * k, ey + 0.05 * k, g + 1.3 * k],
    [x + 1.5 * k, ey - 0.1 * k, g + 2.55 * k],
    0.06 * k,
    solid(THM.wood),
  )
  box(x + 1.18 * k, ey + 0.45 * k, g + 0.06, 0.64 * k, 0.46 * k, 0.3 * k, {
    top: solid(C3('#f3c444')),
    side: solid(THM.woodD),
  })
  beam(
    [x + 2.6 * k, ey + 0.35 * k, g],
    [x + 2.6 * k, ey + 0.35 * k, g + 1.25 * k],
    0.05,
    solid(THM.woodD),
  )
  signL(
    x + 2.18 * k,
    ey + 0.43 * k,
    g + 1.0 * k,
    0.85 * k,
    0.8 * k,
    col,
    C3('#ffffff'),
    GLYPH[m.mono] ? m.mono : null,
  )
}
function crates(x, y) {
  box(x, y, G, 0.42, 0.42, 0.36, { top: solid(THM.woodL), side: planks(THM.wood) })
  box(x + 0.06, y + 0.06, G + 0.36, 0.3, 0.3, 0.26, {
    top: solid(THM.woodL),
    side: planks(THM.woodD),
  })
}
function clockTower(b, roof) {
  const x = b.i + 0.3,
    y = b.j + 0.2,
    s = Math.min(b.w, b.d) - 0.4,
    g = G
  caster(x, y, g, s, s, 4.4)
  box(x - 0.1, y - 0.1, g, s + 0.2, s + 0.2, 0.35, {
    top: solid(THM.plinth),
    side: solid(THM.plinthD),
  })
  box(x, y, g + 0.35, s, s, 3.0, {
    top: solid(THM.plinth),
    left: THM.plates ? bricks(THM.wall, THM.wallD) : solid(shadeC(THM.plinth, 0.3)),
    right: THM.plates ? bricks(THM.wallD, shadeC(THM.wallD, -0.3)) : solid(shadeC(THM.plinth, 0.1)),
  })
  windowL(x + s / 2 - 0.15, y + s, g + 1.2, 0.3, 0.5)
  doorL(x + s / 2 - 0.22, y + s, g + 0.35, 0.44, 0.65)
  const frame = THM.plates ? THM.trim : THM.woodD
  signL(x + s / 2 - 0.35, y + s, g + 2.4, 0.7, 0.7, C3('#f4ecdc'), C3('#2b2118'), 'clock', frame)
  signR(x + s, y + s / 2 - 0.35, g + 2.4, 0.7, 0.7, C3('#f4ecdc'), C3('#2b2118'), 'clock', frame)
  pyr(x - 0.12, y - 0.12, g + 3.35, s + 0.24, s + 0.24, 1.0, shingles(roof))
}
function beaconPillar(b, orbC) {
  const cx = b.i + b.w / 2,
    cy = b.j + b.d / 2,
    g = G
  caster(cx - 0.3, cy - 0.3, g, 0.6, 0.6, 2.4)
  oct(cx, cy, g, 0.42, 0.25, { side: solid(THM.plinthD), top: solid(THM.plinth) })
  oct(cx, cy, g + 0.25, 0.22, 1.5, {
    side: solid(THM.plates ? THM.metal : shadeC(THM.plinth, 0.2)),
    top: solid(THM.plinth),
  })
  oct(cx, cy, g + 1.75, 0.38, 0.1, { side: solid(THM.trim), top: solid(THM.plinth) })
  const c = orbC || C3('#54b8d6')
  dome(
    cx,
    cy,
    g + 1.85,
    0.34,
    0.6,
    {
      side: (q, w, e, u, v, px, py) => lit(h2(px, py) < 0.2 ? shadeC(c, 0.6) : c, 1),
      top: solid(shadeC(c, 0.6)),
    },
    5,
  )
}
function observatory(b, on, domeC, wallC) {
  const cx = b.i + b.w / 2,
    cy = b.j + b.d / 2,
    g = G,
    r = b.w * 0.42
  caster(cx - r, cy - r, g, r * 2, r * 2, 2.2)
  oct(cx, cy, g, r + 0.15, 0.22, { side: solid(THM.plinthD), top: solid(THM.plinth) })
  oct(cx, cy, g + 0.22, r, 0.85, { side: solid(wallC), top: solid(wallC) })
  dome(
    cx,
    cy,
    g + 1.07,
    r + 0.02,
    r * 0.95,
    { side: solid(domeC), top: solid(shadeC(domeC, 0.2)) },
    5,
  )
  beam(
    [cx + 0.1, cy + 0.05, g + 1.5],
    [cx + 0.85, cy - 0.4, g + 2.2],
    0.1,
    solid(THM.plates ? THM.trim : C3('#2f2a55')),
  )
  if (on) box(cx + 0.85, cy - 0.45, g + 2.25, 0.12, 0.12, 0.12, solid(C3('#fffbe0')))
  doorL(cx - 0.2, cy + r * 0.92, g + 0.22, 0.4, 0.55)
}
function workshop(m, roof, wall, wallD) {
  const x = m.i,
    y = m.j,
    g = G,
    w = Math.max(1, m.w) * 0.85,
    d = Math.max(1, m.d) * 0.85
  caster(x, y, g, w, d, 1.6)
  box(x, y, g, w, d, 0.75, {
    top: solid(THM.plinth),
    left: THM.plates ? bricks(wall, wallD) : solid(wall),
    right: THM.plates ? bricks(wallD, shadeC(wallD, -0.3)) : solid(wallD),
  })
  doorL(x + w / 2 - 0.18, y + d, g, 0.36, 0.55)
  gable(
    x,
    y,
    g + 0.75,
    w,
    d,
    0.55,
    shingles(m.weather === 'down' ? C3('#a14a38') : roof),
    solid(wall),
    'y',
    0.1,
  )
  WIN_D = 0.36
  WIN_H = 0.36
  box(
    x + w + 0.02,
    y + 0.15,
    g + 0.25,
    0.06,
    0.36,
    0.36,
    signM(C3('#f1ece2'), C3('#4a5468'), 'gear'),
    { noL: true },
  )
}
function routerGate(gt, stoneC, stoneD, roof) {
  const x = gt.i,
    y = gt.j,
    g = G,
    w = gt.w,
    d = gt.d
  caster(x, y, g, w, d, 2.2)
  const post = (yy) => {
    box(x - 0.05, yy, g, w + 0.1, 0.36, 1.7, {
      top: solid(stoneC),
      left: solid(stoneC),
      right: solid(stoneD),
    })
    pyr(x - 0.12, yy - 0.07, g + 1.7, w + 0.24, 0.5, 0.55, shingles(roof))
  }
  post(y)
  box(x, y + 0.36, g + 1.15, w, d - 0.72, 0.35, {
    top: solid(stoneC),
    left: solid(stoneC),
    right: solid(stoneD),
  })
  for (let k = 0; k < 4; k++)
    box(x + w * 0.45, y + 0.5 + (k * (d - 1)) / 3, g + 0.35, 0.05, 0.05, 0.8, solid(C3('#5d5868')))
  post(y + d - 0.36)
}

/**
 * Observability: a sentinel tower watching the fleet, in the world's materials. A plinth, a slim column, a camera
 * housing whose lens glows on its front, and a mast with a beacon. `s` scales it (the coding tools' smaller one).
 */
function sentinel(b, s = 1) {
  const cx = b.i + b.w / 2,
    cy = b.j + b.d / 2,
    g = G
  const col = THM.plates ? THM.brass : THM.wall,
    colD = THM.plates ? shadeC(THM.brass, -0.2) : THM.wallD
  const shell = THM.plates ? THM.copper : THM.metal,
    shellD = shadeC(shell, -0.22)
  caster(cx - 0.3 * s, cy - 0.3 * s, g, 0.6 * s, 0.6 * s, 2.8 * s)
  oct(cx, cy, g, 0.46 * s, 0.22, { side: solid(THM.plinthD), top: solid(THM.plinth) })
  oct(cx, cy, g + 0.22, 0.17 * s, 1.55 * s, {
    side: (x, y, z, u, v) => lit(v % 0.5 < 0.06 ? THM.trim : col),
    top: solid(colD),
  })
  const z = g + 0.22 + 1.55 * s,
    hw = 0.36 * s,
    hh = 0.5 * s
  box(cx - hw, cy - hw, z, hw * 2, hw * 2, hh, {
    top: solid(shadeC(shell, 0.2)),
    left: solid(shell),
    right: solid(shellD),
  })
  // the lens: a dark ring round a glowing iris, on the front-left face, with a hood over it
  const lens = (q, w, e, u, v) => {
    if (FT === 'T') return lit(shell)
    const W = FT === 'L' ? WIN_W : WIN_D,
      du = u - W / 2,
      dv = v - WIN_H / 2,
      r = Math.hypot(du, dv) / (Math.min(W, WIN_H) / 2)
    if (r > 0.95) return lit(THM.trim)
    if (r > 0.62) return lit(C3('#1b2430'))
    if (r > 0.3) return lit(C3('#3ad7f0'), 1.25)
    return lit(r < 0.14 && du < 0 && dv > 0 ? C3('#ffffff') : C3('#bff6ff'), 1.3)
  }
  WIN_W = hw * 1.5
  WIN_H = hh * 0.86
  box(cx - hw * 0.75, cy + hw - 0.02, z + hh * 0.07, hw * 1.5, 0.08, hh * 0.86, lens, { noR: true })
  box(cx - hw * 0.85, cy + hw, z + hh, hw * 1.7, 0.16 * s, 0.07, {
    top: solid(shellD),
    side: solid(shellD),
  })
  WIN_D = hw * 0.9
  WIN_H = hh * 0.5
  box(
    cx + hw - 0.02,
    cy - hw * 0.45,
    z + hh * 0.25,
    0.06,
    hw * 0.9,
    hh * 0.5,
    signM(C3('#1b2430'), C3('#3ad7f0'), 'lens', THM.trim),
    { noL: true },
  )
  // a mast with a beacon on top
  beam(
    [cx + hw * 0.4, cy - hw * 0.4, z + hh],
    [cx + hw * 0.4, cy - hw * 0.4, z + hh + 0.55 * s],
    0.035,
    solid(THM.metal),
  )
  box(cx + hw * 0.4 - 0.06, cy - hw * 0.4 - 0.06, z + hh + 0.55 * s, 0.12, 0.12, 0.12, {
    top: solid(C3('#ffd0c8')),
    side: () => lit(C3('#ff5a4a'), 1.3),
  })
}

/**
 * A local coding agent (Claude Code, Codex, Cursor…): a big robot standing on its plot in the tool's own colours
 * (our drawing, never the brand's mark): feet, a chassis with a glowing chest light, arms, a head whose visor
 * shows two glowing eyes, and an antenna.
 */
function codingBot(hp) {
  const a = hp.art,
    body = RGB(a.body),
    face = RGB(a.face),
    trim = RGB(a.trim),
    g = G
  const cx = hp.i + hp.w * 0.32,
    cy = hp.j + hp.d * 0.32
  const dark = shadeC(body, -0.25)
  caster(cx - 0.35, cy - 0.3, g, 0.7, 0.6, 1.8)
  box(cx - 0.55, cy - 0.5, g, 1.1, 1.0, 0.06, { top: solid(THM.plinth), side: solid(THM.plinthD) })
  ;[-0.2, 0.08].forEach((o) =>
    box(cx + o, cy - 0.14, g + 0.06, 0.14, 0.3, 0.16, solid(C3('#2b2f36'))),
  )
  box(cx - 0.3, cy - 0.24, g + 0.22, 0.6, 0.48, 0.55, {
    top: solid(shadeC(body, 0.15)),
    left: solid(body),
    right: solid(dark),
  })
  WIN_W = 0.2
  WIN_H = 0.2
  box(
    cx - 0.1,
    cy + 0.22,
    g + 0.38,
    0.2,
    0.06,
    0.2,
    (q, w, e, u, v) => lit(Math.hypot(u - 0.1, v - 0.1) < 0.07 ? face : trim, 1.2),
    { noR: true },
  )
  box(cx - 0.42, cy - 0.08, g + 0.36, 0.12, 0.16, 0.38, solid(trim))
  box(cx + 0.3, cy - 0.08, g + 0.36, 0.12, 0.16, 0.38, solid(shadeC(trim, -0.2)))
  box(cx - 0.08, cy - 0.06, g + 0.77, 0.16, 0.12, 0.06, solid(C3('#2b2f36')))
  const hz = g + 0.83
  box(cx - 0.36, cy - 0.3, hz, 0.72, 0.6, 0.48, {
    top: solid(shadeC(body, 0.15)),
    left: solid(body),
    right: solid(dark),
  })
  const visor = (q, w, e, u, v) => {
    if (FT === 'T') return lit(body)
    const W = FT === 'L' ? WIN_W : WIN_D
    if (u < 0.04 || u > W - 0.04 || v < 0.04 || v > WIN_H - 0.04) return lit(trim)
    const eye =
      Math.hypot(u - W * 0.3, v - WIN_H * 0.52) < 0.065 ||
      Math.hypot(u - W * 0.7, v - WIN_H * 0.52) < 0.065
    return eye ? lit(face, 1.3) : lit(C3('#161a20'))
  }
  WIN_W = 0.56
  WIN_H = 0.28
  box(cx - 0.28, cy + 0.28, hz + 0.1, 0.56, 0.06, 0.28, visor, { noR: true })
  WIN_D = 0.44
  WIN_H = 0.22
  box(cx + 0.34, cy - 0.22, hz + 0.13, 0.06, 0.44, 0.22, visor, { noL: true })
  beam([cx + 0.1, cy - 0.05, hz + 0.48], [cx + 0.1, cy - 0.05, hz + 0.78], 0.03, solid(trim))
  box(cx + 0.04, cy - 0.11, hz + 0.78, 0.12, 0.12, 0.12, {
    top: solid(shadeC(face, 0.3)),
    side: () => lit(face, 1.25),
  })
}

/* ---------- medieval desert ---------- */
const D = {
  townhall(b) {
    const x = b.i,
      y = b.j,
      g = G,
      s = b.w,
      terra = THM.roof
    caster(x + 0.3, y + 0.3, g, s - 0.6, s - 0.6, 3.6)
    box(x, y, g, s, s, 0.22, { top: solid(THM.plinth), side: solid(THM.plinthD) })
    const z0 = g + 0.22,
      wi = x + 0.45,
      wj = y + 0.45,
      w = s - 0.9
    box(wi, wj, z0, w, w, 1.45, {
      top: solid(shadeC(THM.wall, 0.12)),
      left: solid(THM.wall),
      right: solid(THM.wallD),
    })
    for (let k = 0; k < 6; k++) {
      const t = 0.1 + (k * (w - 0.35)) / 5
      box(wi + t, wj + w - 0.22, z0 + 1.45, 0.22, 0.22, 0.2, solid(THM.wall))
      box(wi + w - 0.22, wj + t, z0 + 1.45, 0.22, 0.22, 0.2, solid(THM.wallD))
    }
    doorL(wi + w / 2 - 0.3, wj + w, z0, 0.6, 0.85, C3('#6b4630'))
    windowL(wi + 0.3, wj + w, z0 + 0.6, 0.35, 0.45)
    windowL(wi + w - 0.65, wj + w, z0 + 0.6, 0.35, 0.45)
    windowR(wi + w, wj + 0.4, z0 + 0.6, 0.35, 0.45)
    windowR(wi + w, wj + w - 0.75, z0 + 0.6, 0.35, 0.45)
    const cx = wi + w / 2,
      cy = wj + w / 2
    oct(cx, cy, z0 + 1.45, w * 0.36, 0.4, {
      side: solid(shadeC(THM.wall, 0.06)),
      top: solid(THM.wall),
    })
    dome(
      cx,
      cy,
      z0 + 1.85,
      w * 0.36,
      w * 0.38,
      { side: solid(terra), top: solid(shadeC(terra, 0.15)) },
      6,
    )
    flag(cx, cy, z0 + 1.85 + w * 0.36, '#e5484d', 1.1)
    ;[
      [wi, wj],
      [wi + w, wj],
      [wi, wj + w],
      [wi + w, wj + w],
    ].forEach(([tx, ty]) => {
      cyl(tx, ty, z0, 0.36, 2.05, { side: solid(THM.wall), top: solid(THM.wallD) }, 18)
      dome(
        tx,
        ty,
        z0 + 2.05,
        0.38,
        0.42,
        { side: solid(terra), top: solid(shadeC(terra, 0.15)) },
        4,
      )
    })
  },
  team(t) {
    const x = t.i,
      y = t.j,
      g = G,
      s = 1.31,
      col = RGB(t.color)
    caster(x + 0.1, y + 0.1, g, s - 0.2, s - 0.2, 1.8)
    box(x + 0.08, y + 0.08, g, s - 0.16, s - 0.16, 0.8, {
      top: solid(C3('#f1e8d4')),
      left: solid(THM.wall),
      right: solid(THM.wallD),
    })
    doorL(x + 0.5, y + s - 0.08, g, 0.32, 0.55, C3('#6b4630'))
    windowR(x + s - 0.08, y + 0.4, g + 0.32, 0.3, 0.3)
    gable(
      x + 0.08,
      y + 0.08,
      g + 0.8,
      s - 0.16,
      s - 0.16,
      0.62,
      shingles(col),
      solid(shadeC(col, 0.5)),
      'x',
      0.12,
    )
    flag(x + s + 0.05, y + 0.1, g, t.color, 1.45)
  },
  mine: mineRock,
  barn(b, down) {
    const x = b.i + 0.1,
      y = b.j + 0.1,
      w = b.w - 0.2,
      d = b.d - 0.4,
      g = G
    caster(x, y, g, w, d, 1.8)
    box(x, y, g, w, d, 0.95, {
      top: solid(C3('#f1e8d4')),
      left: solid(C3('#f6f3ec')),
      right: solid(C3('#e3ddd0')),
    })
    doorL(x + w / 2 - 0.25, y + d, g, 0.5, 0.75, C3('#7a8a8f'))
    windowL(x + 0.3, y + d, g + 0.35, 0.4, 0.35)
    windowL(x + w - 0.7, y + d, g + 0.35, 0.4, 0.35)
    signR(x + w, y + d / 2 - 0.3, g + 0.25, 0.6, 0.55, C3('#2f9e6b'), C3('#ffffff'), 'plus')
    hip(x, y, g + 0.95, w, d, 0.75, shingles(C3('#3f9f90')), 0.14)
    for (let k = 0; k < Math.min(3, Math.max(1, down)); k++)
      box(x + 0.2 + k * 0.62, y + d + 0.15, g, 0.48, 0.3, 0.2, {
        top: solid(C3('#f2efe6')),
        side: solid(THM.wood),
      })
  },
  savings(b, saved) {
    const cx = b.i + b.w / 2,
      cy = b.j + b.d / 2,
      g = G,
      r = b.w * 0.5
    oct(cx, cy, g, r, 0.32, { side: solid(THM.plinthD), top: solid(THM.plinth) })
    oct(cx, cy, g + 0.05, r * 0.82, saved ? 0.26 : 0.08, {
      side: solid(THM.waterD),
      top: (x, y, z, u, v, px, py) => lit((px + py) % 7 === 0 ? THM.waterL : THM.water),
    })
    oct(cx, cy, g + 0.1, 0.16, 0.9, { side: solid(THM.plinthD), top: solid(THM.plinth) })
    oct(cx, cy, g + 1.0, 0.32, 0.08, { side: solid(THM.plinthD), top: solid(THM.plinth) })
    if (saved)
      box(cx - 0.1, cy - 0.1, g + 1.08, 0.2, 0.2, 0.2, {
        top: solid(C3('#fff0a6')),
        side: solid(C3('#f3c444')),
      })
  },
  tree(x, y, s) {
    const c = THM.leaf[Math.floor(h2(Math.floor(x * 7), Math.floor(y * 7)) * 2.99)],
      r = (0.38 + h2(Math.floor(x * 3), 1) * 0.18) * Math.max(0.6, s)
    box(x - 0.05, y - 0.05, G, 0.1, 0.1, 0.45 * s, solid(C3('#7a5c44')))
    dome(x, y, G + 0.35 * s, r + 0.06, r * 1.7, { side: solid(c), top: solid(shadeC(c, 0.15)) }, 4)
  },
  gate: (gt) => routerGate(gt, THM.plinth, THM.plinthD, C3('#4f69a6')),
  clock: (b) => clockTower(b, C3('#4f69a6')),
  orchestrator: (b) => beaconPillar(b),
  orb: (b, on) => observatory(b, on, C3('#6c63ad'), C3('#ebe7f3')),
  mcp: (m) => workshop(m, C3('#6c7a92'), C3('#d8c6a6'), C3('#bca98a')),
}

/* ---------- ice age ---------- */
const FELT = C3('#e6d8bc')
/** A yurt: an octagonal felt wall with a coloured band, a dome roof with a felt crown, a door on the front. */
function yurt(cx, cy, r, wall, domeH, roofC, bandC) {
  const g = G
  caster(cx - r * 0.8, cy - r * 0.8, g, r * 1.6, r * 1.6, wall + domeH)
  oct(cx, cy, g, r, wall, {
    side: (x, y, z, u, v) =>
      lit(v > wall * 0.62 && v < wall * 0.8 ? bandC : v > wall * 0.86 ? C3('#6b4a33') : FELT),
    top: solid(FELT),
  })
  dome(
    cx,
    cy,
    g + wall,
    r * 1.08,
    domeH,
    { side: solid(roofC), top: solid(shadeC(roofC, 0.12)) },
    6,
  )
  oct(cx, cy, g + wall + domeH * 0.84, r * 0.42, 0.06, {
    side: solid(C3('#e6dcc6')),
    top: solid(C3('#efe6d2')),
  })
  oct(cx, cy, g + wall + domeH - 0.02, r * 0.28, 0.08, {
    side: solid(C3('#4a3424')),
    top: solid(C3('#efe6d2')),
  })
  doorL(cx - r * 0.25, cy + r * 0.93, g, r * 0.5, wall * 0.8, C3('#9c3c2c'))
}
function pine(x, y, s) {
  const g = G,
    green = THM.leaf,
    snow = C3('#f4f7fa')
  box(x - 0.06 * s, y - 0.06 * s, g, 0.12 * s, 0.12 * s, 0.35 * s, solid(C3('#4a3424')))
  for (let k = 0; k < 3; k++) {
    const r = (0.62 - k * 0.15) * s,
      z = g + (0.25 + k * 0.48) * s,
      h = 0.75 * s,
      c = green[k % 3]
    cone(x, y, z, r, h, (q, w, e) => lit(e > z + h * 0.62 ? snow : c), 10)
  }
}
const I = {
  townhall(b) {
    const x = b.i,
      y = b.j,
      w = b.w,
      g = G
    // a trodden-earth ring, the big yurt between two small ones, a fire and banners
    cyl(
      x + w / 2,
      y + w / 2,
      g,
      w * 0.55,
      0.03,
      {
        side: solid(C3('#8c705d')),
        top: (q, ww, e, u, v, px, py) => lit(h2(px, py) < 0.15 ? C3('#a08670') : C3('#8c705d')),
      },
      28,
    )
    yurt(x + 3.1, y + 0.6, 0.55, 0.62, 0.5, THM.roof, C3('#2b4f86'))
    yurt(x + 0.6, y + 3.1, 0.55, 0.62, 0.5, THM.roof, C3('#2b4f86'))
    yurt(x + w / 2, y + w / 2, 1.3, 1.2, 1.15, THM.roof, C3('#2b4f86'))
    flag(x + w / 2, y + w / 2, g + 2.35, '#e5484d', 1.0)
    // the fire
    const fx = x + 3.25,
      fy = y + 3.25
    for (let k = 0; k < 6; k++) {
      const a = (k / 6) * 6.28
      box(
        fx + Math.cos(a) * 0.32 - 0.07,
        fy + Math.sin(a) * 0.32 - 0.07,
        g,
        0.14,
        0.14,
        0.1,
        solid(C3('#7d8590')),
      )
    }
    beam([fx - 0.2, fy, g + 0.08], [fx + 0.2, fy, g + 0.08], 0.05, solid(THM.woodD))
    beam([fx, fy - 0.2, g + 0.08], [fx, fy + 0.2, g + 0.08], 0.05, solid(THM.woodD))
    cone(
      fx,
      fy,
      g + 0.1,
      0.16,
      0.42,
      (q, w2, e) => lit(e > g + 0.32 ? C3('#ffe08a') : C3('#ff8a2a'), 1.2),
      8,
    )
    flag(x + 0.45, y + 0.6, g, '#3f6fb2', 2.2)
    flag(x + 3.5, y + 2.1, g, '#a8402e', 1.8)
    box(x + 2.55, y + 3.6, g, 0.3, 0.3, 0.32, { top: solid(THM.woodL), side: planks(THM.wood) })
  },
  team(t) {
    yurt(
      t.i + 0.66,
      t.j + 0.66,
      0.6,
      0.62,
      0.5,
      mixC(RGB(t.color), C3('#5f6f86'), 0.4),
      C3('#2b4f86'),
    )
    box(t.i + 0.12, t.j + 1.38, G, 0.32, 0.3, 0.28, {
      top: solid(THM.woodL),
      side: planks(THM.wood),
    })
  },
  mine: mineRock,
  barn(b, down) {
    // a timber longhouse: plank walls, a thatched gable roof with snow on it
    const x = b.i + 0.1,
      y = b.j + 0.15,
      w = b.w - 0.2,
      d = b.d - 0.5,
      g = G
    caster(x, y, g, w, d, 1.9)
    box(x, y, g, w, d, 0.85, {
      top: solid(THM.woodL),
      left: planks(THM.wood),
      right: planks(shadeC(THM.wood, -0.12)),
    })
    doorL(x + w / 2 - 0.3, y + d, g, 0.6, 0.72, C3('#3b2a1c'))
    gable(
      x,
      y,
      g + 0.85,
      w,
      d,
      0.95,
      (q, ww, e, u, v) =>
        lit(
          v > 0.55
            ? C3('#f2f6fa')
            : h2(Math.floor(u * 9), Math.floor(v * 5)) < 0.3
              ? C3('#6e5235')
              : C3('#7d6040'),
        ),
      planks(THM.woodL),
      'x',
      0.18,
    )
    for (let k = 0; k < Math.min(3, Math.max(1, down)); k++)
      box(x + 0.2 + k * 0.62, y + d + 0.15, g, 0.48, 0.3, 0.2, {
        top: solid(C3('#d9cdb8')),
        side: solid(THM.wood),
      })
  },
  savings(b, saved) {
    // a frozen well: a stone ring round an ice-covered hole, a winch frame, gold in a bucket when there are savings
    const cx = b.i + b.w / 2,
      cy = b.j + b.d / 2,
      g = G,
      r = b.w * 0.42
    oct(cx, cy, g, r, 0.4, { side: solid(THM.plinthD), top: solid(THM.plinth) })
    oct(cx, cy, g + 0.05, r * 0.8, 0.36, {
      side: solid(THM.waterD),
      top: (x, y, z, u, v, px, py) => lit((px * 3 + py) % 13 === 0 ? THM.waterL : THM.water),
    })
    beam([cx - r, cy, g + 0.4], [cx - r, cy, g + 1.2], 0.05, solid(THM.woodD))
    beam([cx + r, cy, g + 0.4], [cx + r, cy, g + 1.2], 0.05, solid(THM.woodD))
    beam([cx - r, cy, g + 1.2], [cx + r, cy, g + 1.2], 0.06, solid(THM.wood))
    if (saved)
      box(cx - 0.12, cy - 0.12, g + 0.45, 0.24, 0.24, 0.24, {
        top: solid(C3('#f3c444')),
        side: solid(THM.wood),
      })
  },
  tree: pine,
  gate: (gt) => routerGate(gt, C3('#9fb6cc'), C3('#7f97ae'), C3('#5b4630')),
  clock: (b) => clockTower(b, C3('#6b5134')),
  orchestrator: (b) => beaconPillar(b, C3('#7fc4e8')),
  orb: (b, on) => observatory(b, on, C3('#4d5f9c'), C3('#e8e2d6')),
  mcp: (m) => workshop(m, C3('#6b5134'), C3('#a8835c'), C3('#8a6a48')),
}

/* ---------- steampunk factory ---------- */
const MORTAR = C3('#cbbfa9'),
  MORTAR_D = C3('#a89c87')
function chimney(x, y, z, r, h, smokeCap = true) {
  cyl(x, y, z, r, h, { side: bricks(THM.wall, MORTAR), top: solid(C3('#1c1410')) }, 14)
  cyl(x, y, z + h * 0.32, r * 1.08, 0.08, { side: solid(THM.copper), top: solid(THM.copper) }, 14)
  cyl(x, y, z + h - 0.1, r * 1.12, 0.14, { side: solid(THM.copper), top: solid(C3('#1c1410')) }, 14)
  if (smokeCap)
    for (let k = 0; k < 3; k++) {
      const sz = z + h + 0.25 + k * 0.42,
        s = 0.16 + k * 0.08
      dome(
        x + k * 0.12,
        y - k * 0.06,
        sz,
        s,
        s * 1.4,
        {
          side: solid(shadeC(C3('#6b6560'), k * 0.12)),
          top: solid(shadeC(C3('#7a746e'), k * 0.12)),
        },
        3,
      )
    }
}
function vault(x, y, z, w, d, h, m) {
  // a barrel roof along x: quads round a half circle in the y-z plane
  const n = 10
  for (let k = 0; k < n; k++) {
    const a0 = (Math.PI * k) / n,
      a1 = (Math.PI * (k + 1)) / n
    const y0 = y + d / 2 - (Math.cos(a0) * d) / 2,
      y1 = y + d / 2 - (Math.cos(a1) * d) / 2,
      z0 = z + Math.sin(a0) * h,
      z1 = z + Math.sin(a1) * h
    quad([x, y0, z0], [x + w, y0, z0], [x + w, y1, z1], [x, y1, z1], m, 'S')
  }
  // the gable end facing +x
  for (let k = 0; k < n; k++) {
    const a0 = (Math.PI * k) / n,
      a1 = (Math.PI * (k + 1)) / n
    triangle(
      [x + w, y + d / 2, z],
      [x + w, y + d / 2 - (Math.cos(a0) * d) / 2, z + Math.sin(a0) * h],
      [x + w, y + d / 2 - (Math.cos(a1) * d) / 2, z + Math.sin(a1) * h],
      solid(THM.copper),
      'R',
    )
  }
}
const AMBER = winM(C3('#d4a24c'), AMBER_GLASS, C3('#fff1c9'))
const AMBER_GL = (x, y, z, u) => lit(u % 0.22 < 0.03 ? C3('#3d434b') : AMBER_GLASS, 1.1)
const S = {
  townhall(b) {
    // a brick works hall with cream stone dressings, amber windows, a clock, and a verdigris dome on a cream drum
    const x = b.i,
      y = b.j,
      s = b.w,
      g = G,
      cream = THM.cream
    caster(x + 0.3, y + 0.3, g, s - 0.6, s - 0.6, 3.6)
    chimney(x + 0.7, y + 0.6, g + 0.25, 0.2, 2.5)
    chimney(x + 1.55, y + 0.5, g + 0.25, 0.17, 2.1)
    box(x, y, g, s, s, 0.25, { top: panels(THM.plinth), side: solid(THM.plinthD) })
    const z0 = g + 0.25,
      wi = x + 0.42,
      wj = y + 0.42,
      w = s - 0.84,
      H = 1.5
    box(wi, wj, z0, w, w, H, {
      top: solid(THM.roof),
      left: bricks(THM.wall, MORTAR),
      right: bricks(THM.wallD, MORTAR_D),
    })
    // stone plinth course, quoins at the corners, a cornice
    box(wi - 0.02, wj - 0.02, z0, w + 0.04, w + 0.04, 0.16, {
      top: solid(cream),
      left: solid(cream),
      right: solid(shadeC(cream, -0.14)),
    })
    ;[
      [wi - 0.04, wj + w - 0.16],
      [wi + w - 0.16, wj - 0.04],
      [wi + w - 0.16, wj + w - 0.16],
    ].forEach(([qx, qy]) =>
      box(qx, qy, z0, 0.2, 0.2, H, {
        top: solid(cream),
        left: solid(cream),
        right: solid(shadeC(cream, -0.14)),
      }),
    )
    box(wi - 0.08, wj - 0.08, z0 + H, w + 0.16, w + 0.16, 0.14, {
      top: (q, ww) =>
        lit(
          Math.min(q - wi + 0.08, ww - wj + 0.08, wi + w + 0.08 - q, wj + w + 0.08 - ww) < 0.12
            ? cream
            : THM.roof,
        ),
      left: solid(cream),
      right: solid(shadeC(cream, -0.14)),
    })
    doorL(wi + w / 2 - 0.26, wj + w, z0 + 0.16, 0.52, 0.68, C3('#2b2f36'))
    windowL(wi + 0.3, wj + w, z0 + 0.5, 0.32, 0.55, AMBER)
    windowL(wi + w - 0.62, wj + w, z0 + 0.5, 0.32, 0.55, AMBER)
    windowR(wi + w, wj + 0.35, z0 + 0.5, 0.32, 0.55, AMBER)
    windowR(wi + w, wj + w - 0.67, z0 + 0.5, 0.32, 0.55, AMBER)
    signL(
      wi + w / 2 - 0.3,
      wj + w,
      z0 + 0.92,
      0.6,
      0.5,
      C3('#f4ecdc'),
      C3('#2b2118'),
      'clock',
      THM.brass,
    )
    const cx = wi + w / 2,
      cy = wj + w / 2,
      zt = z0 + H + 0.14
    oct(cx, cy, zt, w * 0.34, 0.42, {
      side: (q, ww, e, u, v) =>
        lit(
          u % 0.32 < 0.11 && v > 0.12 && v < 0.32 ? AMBER_GLASS : cream,
          u % 0.32 < 0.11 && v > 0.12 && v < 0.32 ? 1.15 : FL,
        ),
      top: solid(cream),
    })
    dome(
      cx,
      cy,
      zt + 0.42,
      w * 0.34,
      w * 0.34,
      {
        side: (q, ww, e, u, v, px) => lit(px % 7 === 0 ? THM.brass : THM.verdigris),
        top: solid(shadeC(THM.verdigris, 0.2)),
      },
      6,
    )
    beam(
      [cx, cy, zt + 0.42 + w * 0.32],
      [cx, cy, zt + 0.42 + w * 0.32 + 0.55],
      0.04,
      solid(THM.brass),
    )
    box(cx - 0.06, cy - 0.06, zt + 0.42 + w * 0.32 + 0.55, 0.12, 0.12, 0.12, {
      top: solid(C3('#fff1c9')),
      side: solid(THM.brass),
    })
  },
  team(t) {
    // a brick workshop with cream quoins, a sawtooth roof in the team's colour toned to slate, amber glazing
    const x = t.i,
      y = t.j,
      g = G,
      s = 1.31,
      col = mixC(RGB(t.color), THM.roof, 0.5),
      cream = THM.cream
    caster(x + 0.1, y + 0.1, g, s - 0.2, s - 0.2, 2.2)
    chimney(x + 0.3, y + 0.28, g + 0.85, 0.1, 0.8, true)
    const w = s - 0.16
    box(x + 0.08, y + 0.08, g, w, w, 0.85, {
      top: solid(THM.metal),
      left: bricks(THM.wall, MORTAR),
      right: bricks(THM.wallD, MORTAR_D),
    })
    ;[
      [x + 0.06, y + w - 0.04],
      [x + w - 0.04, y + 0.06],
      [x + w - 0.04, y + w - 0.04],
    ].forEach(([qx, qy]) =>
      box(qx, qy, g, 0.12, 0.12, 0.85, {
        top: solid(cream),
        left: solid(cream),
        right: solid(shadeC(cream, -0.14)),
      }),
    )
    const half = w / 2
    for (const k of [0, 1]) {
      // each tooth: a sloped face in the team's colour toward the viewer, upright amber glazing behind
      const yy = y + 0.08 + k * half,
        z = g + 0.85
      quad(
        [x + 0.08, yy, z + 0.42],
        [x + 0.08 + w, yy, z + 0.42],
        [x + 0.08 + w, yy + half, z],
        [x + 0.08, yy + half, z],
        shingles(col),
        'S',
      )
      box(x + 0.08, yy - 0.01, z, w, 0.03, 0.42, {
        left: AMBER_GL,
        right: solid(THM.metal),
        top: solid(THM.metal),
      })
      triangle(
        [x + 0.08 + w, yy + half, z],
        [x + 0.08 + w, yy, z],
        [x + 0.08 + w, yy, z + 0.42],
        bricks(THM.wallD, MORTAR_D),
        'R',
      )
    }
    doorL(x + 0.5, y + s - 0.08, g, 0.32, 0.55, C3('#2b2f36'))
    WIN_W = 0.26
    WIN_H = 0.26
    box(x + 0.2, y + s - 0.1, g + 0.42, 0.26, 0.06, 0.26, AMBER, { noR: true })
    windowR(x + s - 0.08, y + 0.35, g + 0.3, 0.3, 0.32, AMBER)
  },
  mine(m, small = false) {
    // a copper boiler on a glowing firebox, a flue, a valve, and the provider's sign
    const k = m.w / 2.42,
      x = m.i + m.w / 2,
      y = m.j + m.d / 2,
      g = G,
      col = RGB(m.color)
    const r = (small ? 0.4 : 0.62) * k
    caster(x - r, y - r, g, r * 2, r * 2, 2.6 * k)
    box(x - r - 0.08, y - r - 0.08, g, r * 2 + 0.16, r * 2 + 0.16, 0.35 * k, {
      top: solid(THM.metalL),
      left: (q, w, e, u, v) =>
        lit(
          u > 0.3 && u < 0.8 && v < 0.28 ? C3('#ff9a3a') : THM.metal,
          u > 0.3 && u < 0.8 && v < 0.28 ? 1.3 : FL,
        ),
      right: solid(shadeC(THM.metal, -0.2)),
    })
    cyl(
      x,
      y,
      g + 0.35 * k,
      r,
      1.35 * k,
      {
        side: (q, w, e, u, v) => lit(v % 0.4 < 0.06 ? THM.brass : THM.copper),
        top: solid(THM.copper),
      },
      20,
    )
    dome(
      x,
      y,
      g + (0.35 + 1.35) * k,
      r,
      r * 0.7,
      { side: solid(THM.copper), top: solid(THM.brass) },
      4,
    )
    chimney(x + r * 0.35, y - r * 0.35, g + (0.35 + 1.35) * k + r * 0.4, 0.09 * k, 0.8 * k, !small)
    cyl(
      x - r * 0.75,
      y + r * 0.6,
      g + 0.9 * k,
      0.12,
      0.06,
      { side: solid(THM.brass), top: solid(C3('#f4ecdc')) },
      10,
    )
    if (small) return
    beam(
      [x + r + 0.3, y + r * 0.2, g],
      [x + r + 0.3, y + r * 0.2, g + 1.2 * k],
      0.05,
      solid(THM.metal),
    )
    signL(
      x + r + 0.02,
      y + r * 0.2 + 0.08,
      g + 0.8 * k,
      0.6,
      0.6,
      col,
      C3('#ffffff'),
      GLYPH[m.mono] ? m.mono : null,
      THM.brass,
    )
  },
  barn(b, down) {
    // an engine shed: brick walls under a corrugated barrel vault
    const x = b.i + 0.1,
      y = b.j + 0.15,
      w = b.w - 0.2,
      d = b.d - 0.5,
      g = G
    caster(x, y, g, w, d, 1.9)
    box(x, y, g, w, d, 0.8, {
      top: solid(THM.metal),
      left: bricks(THM.wall, MORTAR),
      right: bricks(THM.wallD, MORTAR_D),
    })
    vault(x - 0.05, y - 0.05, g + 0.8, w + 0.1, d + 0.1, 0.75, (q, ww, e, u) =>
      lit(u % 0.18 < 0.05 ? shadeC(THM.roof, -0.25) : THM.roof),
    )
    WIN_W = 0.7
    WIN_H = 0.68
    box(
      x + w / 2 - 0.35,
      y + d - 0.02,
      g,
      0.7,
      0.06,
      0.68,
      winM(THM.copper, AMBER_GLASS, C3('#fff1c9')),
      { noR: true },
    )
    for (let k = 0; k < Math.min(3, Math.max(1, down)); k++)
      box(x + 0.2 + k * 0.62, y + d + 0.15, g, 0.48, 0.3, 0.2, {
        top: solid(C3('#8a8f96')),
        side: solid(THM.metal),
      })
  },
  savings(b, saved) {
    // a steel strongbox with brass corners and dial, gold beside it when the month saved
    const x = b.i + 0.15,
      y = b.j + 0.15,
      s = b.w - 0.3,
      g = G
    caster(x, y, g, s, s, 1.3)
    box(x, y, g, s, s, 0.92, {
      top: solid(THM.metalL),
      left: panels(C3('#4a5059')),
      right: panels(C3('#3a3f47')),
    })
    ;[
      [x - 0.03, y + s - 0.1],
      [x + s - 0.1, y - 0.03],
      [x + s - 0.1, y + s - 0.1],
    ].forEach(([qx, qy]) => box(qx, qy, g, 0.13, 0.13, 0.92, solid(THM.brass)))
    signL(
      x + s / 2 - 0.24,
      y + s,
      g + 0.28,
      0.48,
      0.48,
      C3('#2e333a'),
      THM.brass,
      'gear',
      THM.brass,
    )
    if (saved) {
      box(x + s + 0.05, y + 0.1, g, 0.28, 0.4, 0.14, {
        top: solid(C3('#fff0a6')),
        side: solid(C3('#f3c444')),
      })
      box(x + s + 0.08, y + 0.15, g + 0.14, 0.22, 0.3, 0.12, {
        top: solid(C3('#fff0a6')),
        side: solid(C3('#d9a92b')),
      })
    }
  },
  tree(x, y, s) {
    // a little stack of barrels with copper hoops
    const r = 0.15 * Math.max(0.7, s),
      band = (q, w, e, u, v) => lit(v % 0.3 < 0.05 ? THM.copper : THM.wood)
    cyl(x - r * 0.9, y, G, r, 0.38 * s + 0.1, { side: band, top: solid(THM.woodL) }, 12)
    cyl(x + r * 0.9, y + r * 0.4, G, r, 0.38 * s + 0.1, { side: band, top: solid(THM.woodL) }, 12)
    cyl(
      x,
      y + r * 0.2,
      G + 0.38 * s + 0.1,
      r,
      0.34 * s + 0.08,
      { side: band, top: solid(THM.woodL) },
      12,
    )
  },
  gate(gt) {
    // a copper pipe manifold on iron legs, with a valve wheel
    const x = gt.i + gt.w / 2,
      y0 = gt.j,
      d = gt.d,
      g = G,
      z = g + 1.4
    caster(gt.i, gt.j, g, gt.w, gt.d, 1.8)
    ;[0.15, d - 0.15].forEach((o) => {
      beam([x - 0.15, y0 + o, g], [x - 0.15, y0 + o, z], 0.04, solid(THM.metal))
      beam([x + 0.15, y0 + o, g], [x + 0.15, y0 + o, z], 0.04, solid(THM.metal))
    })
    beam([x, y0 - 0.05, z + 0.15], [x, y0 + d + 0.05, z + 0.15], 0.16, solid(THM.copper))
    ;[0.35, d / 2, d - 0.35].forEach((o) =>
      cyl(x, y0 + o, z + 0.3, 0.1, 0.35, { side: solid(THM.brass), top: solid(THM.brass) }, 10),
    )
    WIN_D = 0.3
    WIN_H = 0.3
    box(
      x + 0.2,
      y0 + d / 2 - 0.15,
      z,
      0.05,
      0.3,
      0.3,
      signM(THM.metal, THM.brass, 'gear', THM.brass),
      { noL: true },
    )
  },
  clock: (b) => clockTower(b, THM.verdigris),
  orchestrator: (b) => beaconPillar(b, THM.amber),
  orb: (b, on) => observatory(b, on, THM.copper, C3('#8e4b35')),
  mcp: (m) => workshop(m, C3('#4b4f56'), C3('#8e4b35'), C3('#743c2a')),
  /** Nothing stands round the deck (no stacks): the village is the picture. */
  extras() {
    return
    ;[
      [-1.4, 3, 0.42, 5.6],
      [1.2, -1.6, 0.5, 6.4],
      [7.5, -1.5, 0.36, 4.6],
      [13.8, -1.7, 0.46, 6.0],
      [21.5, -1.5, 0.4, 5.0],
      [-1.5, 11, 0.36, 4.4],
      [28.6, 2.5, 0.38, 4.8],
    ].forEach(([x, y, r, h]) => {
      caster(x - r, y - r, 1, r * 2, r * 2, h)
      chimney(x, y, 0.15, r, h)
    })
  },
}
const KITS = { desert: D, ice: I, steam: S }

/* ---------- the bake ---------- */
let ENT = []
function entIndex(key) {
  let k = ENT.indexOf(key)
  if (k < 0) {
    ENT.push(key)
    k = ENT.length - 1
  }
  return k + 1
}
function layout(v) {
  const C = v.core
  return {
    main: { i: 0, j: 0, w: 19, d: 18 },
    outside: { i: 19, j: 0, w: 8, d: 8 },
    cobble: [
      { i: C.townhall.i - 0.5, j: C.townhall.j - 0.5, w: C.townhall.w + 1, d: C.townhall.d + 1 },
      { i: C.depot.i, j: C.depot.j, w: C.depot.w, d: C.depot.d },
    ],
    roads: [
      [[0, 8.85], [5.25, 8.85], 0.6],
      [[4.65, 8.25], [4.65, 18], 0.6],
      [[0, 13.25], [11.9, 13.25], 0.6],
    ],
    lobes: [
      [-3.2, 15.5, 3.2],
      [9.5, 21.2, 2.8],
      [30.2, 2.6, 2.6],
      [24.6, 11.6, 2.2],
    ],
    holes: [],
  }
}
function bounds() {
  let x0 = 1e9,
    x1 = -1e9,
    y0 = 1e9,
    y1 = -1e9
  for (let j = GJ; j < GJ + NJ; j++)
    for (let i = GI; i < GI + NI; i++)
      if (HT(i, j)) {
        const h = HT(i, j)
        x0 = Math.min(x0, (i - j - 1) * TW)
        x1 = Math.max(x1, (i - j + 1) * TW)
        y0 = Math.min(y0, (i + j) * TH - (h + 3) * ZH)
        y1 = Math.max(y1, (i + j + 2) * TH + ZH)
      }
  return { x0: x0 - 24 * RES, x1: x1 + 24 * RES, y0: y0 - 24 * RES * 3, y1: y1 + 16 * RES }
}
/**
 * Bakes the whole village into the buffer, in one theme ('desert', 'ice' or 'steam'): returns its size, where world
 * (0, 0, 1) lands in it, the colour, the place-id buffer and the place keys (id k is keys[k - 1]).
 */
export function bake(v, saved, trees, bushes, theme = 'desert') {
  const t0 = performance.now()
  setRes(3)
  THM = THEMES[theme] || THEMES.desert
  const K = KITS[theme] || D
  L = layout(v)
  genTerrain()
  bakeSegs()
  const B = bounds()
  rbInit(Math.ceil(B.x1 - B.x0), Math.ceil(B.y1 - B.y0), -B.x0, -B.y0)
  ENT = []
  SHADOWS = []
  drawTerrain()
  const C = v.core,
    place = (key, fn) => withId(entIndex(key), K_OBJ, fn)
  const b = v.hud.budget
  if (K.extras) withId(0, K_DECOR, () => K.extras())
  place('townhall', () => K.townhall(C.townhall))
  place('tank', () =>
    tokenTank(
      C.tank.i + 1.06,
      C.tank.j + 1.06,
      1.0,
      U(82),
      b && b.limit ? ((b.left / b.limit) * 0.8) / 0.92 : 0.05,
      !!b && b.pace > 1,
      { flag: '#f3c444' },
    ),
  )
  place('piggy', () => K.savings(C.piggy, !!saved))
  place('orb', () => K.orb(C.orb, v.log.upgrade.length > 0))
  place('orchestrator', () => K.orchestrator(C.orchestrator))
  place('canopy', () => K.barn(C.barn, v.cars.filter((c) => c.pose === 'down').length))
  place('workflows', () => K.clock(C.clock))
  v.teams.forEach((t) =>
    place(`team:${t.id}`, () => {
      K.team(t)
      const tb = t.budget,
        left = tb && tb.limit > 0 ? Math.max(0, 1 - tb.spent / tb.limit) : 0.05
      tokenTank(t.i + 1.97, t.j + 0.85, 0.4, U(32), left, (t.pace ?? 0) > 1)
    }),
  )
  v.mines.forEach((m) => place(`mine:${m.id}`, () => K.mine(m)))
  v.xmines.forEach((m) => place(`xmine:${m.id}`, () => K.mine(m, true)))
  v.gates.forEach((g) => place(`gate:${g.id}`, () => K.gate(g)))
  v.mcps.forEach((m) => place(`mcp:${m.id}`, () => K.mcp(m)))
  v.tools.forEach((t) => place(`tool:${t.id}`, () => crates(t.i + 0.16, t.j + 0.16)))
  if (v.outpost)
    place('xtank', () => tokenTank(C.xtank.i + 1.06, C.xtank.j + 1.06, 1.0, U(82), 0.62, false))
  place('observatory', () => sentinel(C.observatory, 1))
  ;(v.harnesses || []).forEach((hp) => place(`harness:${hp.kind}`, () => codingBot(hp)))
  if (v.outpost) place('outpost', () => sentinel(C.outpost, 0.85))
  withId(0, K_DECOR, () => {
    trees.forEach(([x, y, s]) => K.tree(x, y, s))
    bushes.forEach(([x, y, s]) => K.tree(x, y, s * 0.6))
    if (!THM.plates)
      [
        [-1.8, 12.6, 0.9],
        [-0.8, 16.8, 0.8],
        [7.6, 19.4, 0.85],
        [20.5, 9.6, 0.8],
        [28.3, 7.2, 0.9],
        [-1.6, 4.8, 0.8],
        [-2.4, 14.2, 0.7],
        [11.2, 20.2, 0.75],
      ].forEach(([x, y, s]) => K.tree(x, y, s))
  })
  THM.mesas.forEach(([mx, my, mr, mh]) =>
    SHADOWS.push({ x: mx - mr * 0.7, y: my - mr * 0.7, z: 1, w: mr * 1.4, d: mr * 1.4, h: mh - 1 }),
  )
  shadowPass()
  outlinePass(PACK(...THM.ink), 0.8, true)
  return {
    W: RB.W,
    H: RB.H,
    OX: RB.OX,
    OY: RB.OY,
    col: RB.col,
    id: RB.id,
    keys: ENT.slice(),
    ms: performance.now() - t0,
  }
}

/** The baked colours with the selection's dimming and the hovered place lit, as RGBA bytes. */
export function compose(baked, dimKeys, hoverKey) {
  const { W, H, col, id, keys } = baked
  const out = new Uint32Array(col)
  const dim = new Uint8Array(keys.length + 1)
  keys.forEach((k, n) => {
    if (dimKeys.has(k)) dim[n + 1] = 1
  })
  const hov = hoverKey ? keys.indexOf(hoverKey) + 1 : -1
  const any = dimKeys.size > 0,
    dimC = PACK(40, 34, 56)
  if (any || hov > 0)
    for (let k = 0; k < W * H; k++) {
      const me = id[k]
      if (!me) continue
      if (dim[me]) {
        const c = out[k],
          l = UNR(c) * 0.3 + UNG(c) * 0.59 + UNB(c) * 0.11
        out[k] = mixP(mixP(c, PACK(l, l, l), 0.7), dimC, 0.38)
        continue
      }
      if (me === hov) out[k] = mixP(out[k], PACK(255, 250, 230), 0.16)
    }
  return new Uint8ClampedArray(out.buffer)
}

/* ---------- agents as pixel bots ---------- */
/*
 * An AI agent: a sleek little robot, its chassis off-white, lit on the left and shaded on the right like every voxel,
 * a dark visor, and its state only in the light: the eyes, the antenna tip and the chest. Running: cyan. Slow: amber.
 * Broken: red X eyes and a spark. Asleep: lights off, eyes shut, the chassis a shade darker.
 */
const BOT = [
  '....g....',
  '....s....',
  '..HHhhk..',
  '.Hvvvvvk.',
  '.Hvevevk.',
  '.Hvvvvvk.',
  '..Hhhhk..',
  '...sss...',
  '.HHhhhkk.',
  's.HhghK.s',
  '..Hhhhk..',
  '..Hk.Hk..',
  '..ss.ss..',
]
export function pieceSprite(beacon, _theme = 'desert') {
  const glow = {
    active: '#56e3f5',
    running: '#56e3f5',
    slow: '#ffb547',
    broken: '#ff5252',
    asleep: null,
  }[beacon]
  const k = beacon === 'asleep' ? 0.86 : 1
  const tone = (c) => hex(RGB(c).map((v) => v * k))
  const C = {
    H: tone('#f7f9fb'),
    h: tone('#e2e6ea'),
    k: tone('#b7bfc8'),
    K: tone('#b7bfc8'),
    v: '#1b2129',
    s: tone('#59626d'),
    g: glow || '#4a535d',
    e: glow || '#3a434d',
  }
  const rows = BOT.map((r) => r.split(''))
  if (beacon === 'broken') {
    rows[0] = '..g...g..'.split('')
    rows[1] = '...gs....'.split('')
    rows[4] = '.HvXvXvk.'.split('')
  }
  if (!glow) rows[4] = '.Hv-v-vk.'.split('')
  const col = (ch) =>
    ch === '.' ? null : ch === 'X' ? glow : ch === '-' ? '#4a535d' : C[ch] || null
  const pix = rows.map((r) => r.map(col))
  const h = pix.length,
    w = 9
  const out = new Uint8ClampedArray(w * h * 4)
  for (let y = 0; y < h; y++)
    for (let x = 0; x < w; x++) {
      const c = pix[y][x]
      if (!c) continue
      const [r, g, b] = RGB(c),
        o = (y * w + x) * 4
      out[o] = r
      out[o + 1] = g
      out[o + 2] = b
      out[o + 3] = 255
    }
  return { w, h, data: out }
}
const hex = (c) =>
  '#' +
  c
    .map((v) =>
      Math.round(Math.max(0, Math.min(255, v)))
        .toString(16)
        .padStart(2, '0'),
    )
    .join('')
