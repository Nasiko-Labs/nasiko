// Cubic Bézier chains. A chain is [P0, C1, C2, P1, C1, C2, P2, …]: every
// segment is a true cubic, deformations move control points (never sampled
// polygons), and rendering emits C commands, so outlines stay smooth.

import type { Vec } from './path'

export type Chain = Vec[]

const f = (n: number) => (Math.round(n * 100) / 100).toString()

/** Centripetal Catmull-Rom through closed points → cubic chain (ends back at the first point). */
export function crClosed(pts: Vec[], alpha = 0.5): Chain {
  const n = pts.length
  const out: Chain = [pts[0]]
  for (let i = 0; i < n; i++) {
    const p0 = pts[(i - 1 + n) % n]
    const p1 = pts[i]
    const p2 = pts[(i + 1) % n]
    const p3 = pts[(i + 2) % n]
    const [c1, c2] = crSeg(p0, p1, p2, p3, alpha)
    out.push(c1, c2, p2)
  }
  return out
}

/** Centripetal Catmull-Rom through open points (end tangents mirrored). */
export function crOpen(pts: Vec[], alpha = 0.5): Chain {
  const n = pts.length
  const ext = (a: Vec, b: Vec): Vec => ({ x: 2 * a.x - b.x, y: 2 * a.y - b.y })
  const out: Chain = [pts[0]]
  for (let i = 0; i < n - 1; i++) {
    const p0 = i === 0 ? ext(pts[0], pts[1]) : pts[i - 1]
    const p3 = i + 2 >= n ? ext(pts[n - 1], pts[n - 2]) : pts[i + 2]
    const [c1, c2] = crSeg(p0, pts[i], pts[i + 1], p3, alpha)
    out.push(c1, c2, pts[i + 1])
  }
  return out
}

function crSeg(p0: Vec, p1: Vec, p2: Vec, p3: Vec, alpha: number): [Vec, Vec] {
  const d = (a: Vec, b: Vec) => Math.max(1e-6, Math.pow(Math.hypot(b.x - a.x, b.y - a.y), alpha))
  const d1 = d(p0, p1)
  const d2 = d(p1, p2)
  const d3 = d(p2, p3)
  // Barry–Goldman tangents for non-uniform (centripetal) parameterisation
  const m1x = d2 * ((p1.x - p0.x) / d1 - (p2.x - p0.x) / (d1 + d2)) + (p2.x - p1.x)
  const m1y = d2 * ((p1.y - p0.y) / d1 - (p2.y - p0.y) / (d1 + d2)) + (p2.y - p1.y)
  const m2x = d2 * ((p3.x - p2.x) / d3 - (p3.x - p1.x) / (d2 + d3)) + (p2.x - p1.x)
  const m2y = d2 * ((p3.y - p2.y) / d3 - (p3.y - p1.y) / (d2 + d3)) + (p2.y - p1.y)
  return [
    { x: p1.x + m1x / 3, y: p1.y + m1y / 3 },
    { x: p2.x - m2x / 3, y: p2.y - m2y / 3 },
  ]
}

export function chainD(c: Chain, closed = true): string {
  if (c.length < 4) return ''
  let d = `M${f(c[0].x)},${f(c[0].y)}`
  for (let i = 1; i + 2 < c.length; i += 3)
    d += `C${f(c[i].x)},${f(c[i].y)} ${f(c[i + 1].x)},${f(c[i + 1].y)} ${f(c[i + 2].x)},${f(c[i + 2].y)}`
  return closed ? d + 'Z' : d
}

export const mapChain = (c: Chain, fn: (p: Vec) => Vec): Chain => c.map(fn)

export function cubicAt(a: Vec, b: Vec, c: Vec, d: Vec, t: number): Vec {
  const u = 1 - t
  const A = u * u * u
  const B = 3 * u * u * t
  const C = 3 * u * t * t
  const D = t * t * t
  return { x: A * a.x + B * b.x + C * c.x + D * d.x, y: A * a.y + B * b.y + C * c.y + D * d.y }
}

export function cubicTangent(a: Vec, b: Vec, c: Vec, d: Vec, t: number): Vec {
  const u = 1 - t
  return {
    x: 3 * u * u * (b.x - a.x) + 6 * u * t * (c.x - b.x) + 3 * t * t * (d.x - c.x),
    y: 3 * u * u * (b.y - a.y) + 6 * u * t * (c.y - b.y) + 3 * t * t * (d.y - c.y),
  }
}

/** de Casteljau: the part of a cubic between t0 and t1, as a cubic. */
export function cubicSub(
  a: Vec,
  b: Vec,
  c: Vec,
  d: Vec,
  t0: number,
  t1: number,
): [Vec, Vec, Vec, Vec] {
  const split = (
    p: [Vec, Vec, Vec, Vec],
    t: number,
  ): [[Vec, Vec, Vec, Vec], [Vec, Vec, Vec, Vec]] => {
    const l = (u: Vec, v: Vec): Vec => ({ x: u.x + (v.x - u.x) * t, y: u.y + (v.y - u.y) * t })
    const ab = l(p[0], p[1])
    const bc = l(p[1], p[2])
    const cd = l(p[2], p[3])
    const abc = l(ab, bc)
    const bcd = l(bc, cd)
    const m = l(abc, bcd)
    return [
      [p[0], ab, abc, m],
      [m, bcd, cd, p[3]],
    ]
  }
  const [, right] = split([a, b, c, d], t0)
  const t = t1 >= 1 ? 1 : (t1 - t0) / (1 - t0 || 1)
  return split(right, t)[0]
}

/** Parameter where a cubic monotonic in x reaches x (bisection). */
export function cubicTAtX(a: Vec, b: Vec, c: Vec, d: Vec, x: number): number {
  let lo = 0
  let hi = 1
  const inc = d.x >= a.x
  for (let i = 0; i < 40; i++) {
    const m = (lo + hi) / 2
    const v = cubicAt(a, b, c, d, m).x
    if (v < x === inc) lo = m
    else hi = m
  }
  return (lo + hi) / 2
}

/** Cubic pieces (≤ 90° each) for a circular arc from angle a0 to a1 (radians, either direction). */
export function arcChain(c: Vec, r: number, a0: number, a1: number, rx = r): Chain {
  const n = Math.max(1, Math.ceil(Math.abs(a1 - a0) / (Math.PI / 2) - 1e-9))
  const da = (a1 - a0) / n
  const k = (4 / 3) * Math.tan(da / 4)
  const pt = (a: number): Vec => ({ x: c.x + Math.cos(a) * rx, y: c.y + Math.sin(a) * r })
  const out: Chain = [pt(a0)]
  for (let i = 0; i < n; i++) {
    const s = a0 + da * i
    const e = s + da
    const p0 = pt(s)
    const p3 = pt(e)
    out.push(
      { x: p0.x - Math.sin(s) * rx * k, y: p0.y + Math.cos(s) * r * k },
      { x: p3.x + Math.sin(e) * rx * k, y: p3.y - Math.cos(e) * r * k },
      p3,
    )
  }
  return out
}

/** A full ellipse as a closed cubic chain (4 segments), optionally rotated. */
export function ellipseChain(cx: number, cy: number, rx: number, ry: number, rot = 0): Chain {
  const k = 0.5522847498
  const a = (rot * Math.PI) / 180
  const cs = Math.cos(a)
  const sn = Math.sin(a)
  const R = (x: number, y: number): Vec => ({ x: cx + x * cs - y * sn, y: cy + x * sn + y * cs })
  return [
    R(rx, 0),
    R(rx, ry * k),
    R(rx * k, ry),
    R(0, ry),
    R(-rx * k, ry),
    R(-rx, ry * k),
    R(-rx, 0),
    R(-rx, -ry * k),
    R(-rx * k, -ry),
    R(0, -ry),
    R(rx * k, -ry),
    R(rx, -ry * k),
    R(rx, 0),
  ]
}

/** Line segment as a (degenerate) cubic, so chains stay uniform. */
export function lineSeg(a: Vec, b: Vec): Vec[] {
  return [
    { x: a.x + (b.x - a.x) / 3, y: a.y + (b.y - a.y) / 3 },
    { x: a.x + ((b.x - a.x) * 2) / 3, y: a.y + ((b.y - a.y) * 2) / 3 },
    b,
  ]
}

/** Sample a chain into a polygon (n points per segment). */
export function flatten(c: Chain, n = 6): Vec[] {
  const out: Vec[] = []
  for (let i = 0; i + 3 < c.length + 0 && i + 3 <= c.length - 1; i += 3) {
    for (let k = 0; k < n; k++) out.push(cubicAt(c[i], c[i + 1], c[i + 2], c[i + 3], k / n))
  }
  if (c.length) out.push(c[c.length - 1])
  return out
}

export function polyArea(p: Vec[]): number {
  let a = 0
  for (let i = 0; i < p.length; i++) {
    const q = p[(i + 1) % p.length]
    a += p[i].x * q.y - q.x * p[i].y
  }
  return a / 2
}

export function pointInPoly(p: Vec, poly: Vec[]): boolean {
  let inside = false
  for (let i = 0, j = poly.length - 1; i < poly.length; j = i++) {
    const a = poly[i]
    const b = poly[j]
    if (a.y > p.y !== b.y > p.y && p.x < ((b.x - a.x) * (p.y - a.y)) / (b.y - a.y) + a.x)
      inside = !inside
  }
  return inside
}

export function distToSeg(p: Vec, a: Vec, b: Vec): number {
  const dx = b.x - a.x
  const dy = b.y - a.y
  const L = dx * dx + dy * dy
  const t = L > 0 ? Math.max(0, Math.min(1, ((p.x - a.x) * dx + (p.y - a.y) * dy) / L)) : 0
  return Math.hypot(p.x - a.x - dx * t, p.y - a.y - dy * t)
}

/** Signed distance to a closed polygon: + inside, − outside. */
export function polyDepth(poly: Vec[], p: Vec): number {
  let d = Infinity
  for (let i = 0; i < poly.length; i++)
    d = Math.min(d, distToSeg(p, poly[i], poly[(i + 1) % poly.length]))
  return pointInPoly(p, poly) ? d : -d
}

/** Do two segments properly cross? */
export function segCross(a: Vec, b: Vec, c: Vec, d: Vec): boolean {
  const o = (p: Vec, q: Vec, r: Vec) => (q.x - p.x) * (r.y - p.y) - (q.y - p.y) * (r.x - p.x)
  const d1 = o(c, d, a)
  const d2 = o(c, d, b)
  const d3 = o(a, b, c)
  const d4 = o(a, b, d)
  return d1 * d2 < 0 && d3 * d4 < 0
}

/** Self-intersection check for a closed polygon (O(n²), tests only). */
export function selfIntersects(poly: Vec[]): boolean {
  const n = poly.length
  for (let i = 0; i < n; i++)
    for (let j = i + 2; j < n; j++) {
      if (i === 0 && j === n - 1) continue
      if (segCross(poly[i], poly[(i + 1) % n], poly[j], poly[(j + 1) % n])) return true
    }
  return false
}

/** Do two closed polygons overlap (any edge crossing or containment)? */
export function polysOverlap(a: Vec[], b: Vec[]): boolean {
  for (let i = 0; i < a.length; i++)
    for (let j = 0; j < b.length; j++)
      if (segCross(a[i], a[(i + 1) % a.length], b[j], b[(j + 1) % b.length])) return true
  return pointInPoly(a[0], b) || pointInPoly(b[0], a)
}

/** Tapered stroke along a cubic as a filled closed shape (widths at start, middle, end). */
export function taperedStroke(p: [Vec, Vec, Vec, Vec], w: [number, number, number], n = 10): Vec[] {
  const left: Vec[] = []
  const right: Vec[] = []
  for (let i = 0; i <= n; i++) {
    const t = i / n
    const c = cubicAt(p[0], p[1], p[2], p[3], t)
    const tg = cubicTangent(p[0], p[1], p[2], p[3], t)
    const L = Math.hypot(tg.x, tg.y) || 1
    const nx = -tg.y / L
    const ny = tg.x / L
    const ww =
      (t < 0.5
        ? w[0] + (w[1] - w[0]) * Math.sin(t * Math.PI)
        : w[2] + (w[1] - w[2]) * Math.sin(t * Math.PI)) / 2
    left.push({ x: c.x + nx * ww, y: c.y + ny * ww })
    right.push({ x: c.x - nx * ww, y: c.y - ny * ww })
  }
  return [...left, ...right.reverse()]
}
