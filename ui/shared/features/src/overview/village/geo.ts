/**
 * The village's geometry (plans/feat-village-overview.md §4, V19, V50): tile coordinates (i, j) to SVG points on the
 * isometric grid, the two lands (the main island of grass, 19 × 18 tiles, and the dirt plot of the local tools, 8 × 8,
 * set against its east edge), and where the drawn world sits in map units for the viewport. One tile is 48 × 24 px, the
 * reference design's own grid (Figma "Group 60"), so its sizes carry over one to one. Pure: no JSX.
 */

/** Half a tile's width and height in px. */
export const HW = 24
export const HH = 12

export interface Land {
  i0: number
  j0: number
  /** Tiles along i (down-right on screen) and along j (down-left). */
  ni: number
  nj: number
}
/** The main island: the org, its fleet, its providers. */
export const MAIN: Land = { i0: 0, j0: 0, ni: 19, nj: 18 }
/** The dirt plot: the local coding tools, their battery, their observatory and the providers they call. */
export const OUTSIDE: Land = { i0: 19, j0: 0, ni: 8, nj: 8 }

export type Pt = readonly [number, number]

/** A tile corner (i, j) as a point on screen. */
export const P = (i: number, j: number): Pt => [(i - j) * HW, (i + j) * HH]
/** The same point lifted `h` px. */
export const up = (p: Pt, h: number): Pt => [p[0], p[1] - h]
export const pts = (a: readonly Pt[]) =>
  a.map((p) => `${p[0].toFixed(1)},${p[1].toFixed(1)}`).join(' ')

/** The iso ellipse of a circle on the ground, for a radius in tiles. */
export const ringRadii = (r: number) => [r * HW * 1.414, r * HH * 1.414] as const

/** How deep the lands' cliffs are. */
export const CLIFF = 21

/** The drawn part of the world, in map units: both lands plus room above them for the tallest buildings. */
const corners = [MAIN, OUTSIDE].flatMap((l) => [
  P(l.i0, l.j0),
  P(l.i0 + l.ni, l.j0),
  P(l.i0, l.j0 + l.nj),
  P(l.i0 + l.ni, l.j0 + l.nj),
])
const minX = Math.min(...corners.map((p) => p[0]))
const maxX = Math.max(...corners.map((p) => p[0]))
const maxY = Math.max(...corners.map((p) => p[1]))
const PAD = 28
const TOP = 120
export const WORLD = {
  width: maxX - minX + PAD * 2,
  height: maxY + TOP + PAD + CLIFF,
  ox: -minX + PAD,
  oy: TOP,
} as const
export const toMap = (p: Pt): Pt => [p[0] + WORLD.ox, p[1] + WORLD.oy]

/**
 * The voxel worlds paint a coast round both lands. Its noise could reach 4.7 tiles out, but at the island's outermost
 * corners (what the fit is limited by) it is measured at about 2.6 tiles in the widest worlds (the desert and the ice
 * age; the steampunk deck is narrower): this frame leaves a clear margin past that, not room for the coast's maximum.
 * It rises this far above the lands' top edge.
 */
const COAST = 2.5
const COAST_RISE = 60
const coast = [MAIN, OUTSIDE].flatMap((l) => [
  P(l.i0 - COAST, l.j0 - COAST),
  P(l.i0 + l.ni + COAST, l.j0 - COAST),
  P(l.i0 - COAST, l.j0 + l.nj + COAST),
  P(l.i0 + l.ni + COAST, l.j0 + l.nj + COAST),
])
const coastX = coast.map((p) => p[0] + WORLD.ox)
const coastY = coast.map((p) => p[1] + WORLD.oy)
const frameX0 = Math.min(...coastX) - PAD
const frameY0 = Math.min(0, Math.min(...coastY) - COAST_RISE)
/**
 * What the village view fits, in map units: the voxel world with its coast, which reaches past `WORLD` on every side.
 * Every world theme is voxel, so this is fitted from the start and the view doesn't jump when the bake replaces the
 * flat drawing.
 */
export const VILLAGE_FRAME = {
  x0: frameX0,
  y0: frameY0,
  width: Math.max(...coastX) + PAD - frameX0,
  height: Math.max(...coastY) + CLIFF + PAD - frameY0,
} as const
