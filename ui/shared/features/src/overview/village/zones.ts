/**
 * The village's districts (plans/feat-village-overview.md V50): fixed spots for the landmarks and a plot list per
 * kind, so only that kind ever sits in its district and an empty plot shows where the next one goes. Every spot is the
 * reference design's own (playground ref-026, dec-079), read off its 48 × 24 px grid. Pure data.
 *
 *          west ─────────────────────────────────────────────────────────────► east                     dirt plot
 *   north │ TOWN HALL · battery · piggy        │ routers │ providers (tents)  │ battery · brick stacks
 *         │ opportunities · the eye            │ PROJECTS │ MCP gateways · tools │ the eye tower · harness ball
 *   south │ TEAMS (2 × 2 between sand strips)  │ globe · barn · PARKING (agents) · clock tower (workflows)
 *
 * The free plan sizes every district (`plan.ts`): what a district holds is what the plan opens, plus a few locked
 * plots on the dirt for coding tools a paid plan adds.
 */
import { MAIN, OUTSIDE } from './geo'

export interface Spot {
  i: number
  j: number
}
export interface Plot extends Spot {
  w: number
  d: number
}

/** The landmarks: one each, always drawn. */
export const CORE = {
  townhall: { i: 0.26, j: 4.26, w: 3.7, d: 3.7 },
  /** The token battery: the org's budget, in the middle of the village. A cylinder of radius 1.06 on this square. */
  tank: { i: 10.2, j: 6.8, w: 2.12, d: 2.12 },
  piggy: { i: 6.6, j: 0.3, w: 1.25, d: 1.25 },
  /** The opportunities observatory: the purple dome with the spark. */
  orb: { i: 5.7, j: 6.3, w: 1.53, d: 1.53 },
  /** The observability eye's home; it walks a loop around it (`EYE_WALK`). */
  observatory: { i: 7.16, j: 4.96, w: 0.8, d: 0.8 },
  /** The orchestrator: the globe on its pole, beside the parking. */
  orchestrator: { i: 10.86, j: 12.04, w: 0.8, d: 0.8 },
  /** The parking: the fleet's bays and its aisle. */
  depot: { i: 9.2, j: 13.7, w: 9.5, d: 3.75 },
  /** The barn on the parking's back edge: the service bay broken agents park in front of. */
  barn: { i: 12, j: 11.6, w: 2.5, d: 2.1 },
  /** The clock tower: workflows. */
  clock: { i: 16.8, j: 11.85, w: 1.8, d: 1.5 },
  /** On the dirt plot: the coding tools' eye tower and their own battery. */
  outpost: { i: 21.5, j: 5.05, w: 0.8, d: 0.8 },
  xtank: { i: 21.6, j: 0.54, w: 2.12, d: 2.12 },
} as const satisfies Record<string, Plot>
export type CoreId = keyof typeof CORE

/** The loop the observability eye walks around the park, in tiles, from its home. */
export const EYE_WALK: readonly (readonly [number, number])[] = [
  [7.56, 5.36],
  [9.6, 5.36],
  [9.6, 3.05],
  [7.56, 3.05],
]

const grid = (is: readonly number[], js: readonly number[], w: number, d = w): Plot[] =>
  js.flatMap((j) => is.map((i) => ({ i, j, w, d })))

/** A bay on the parking: a point (pieces stand on it, they don't cover tiles). */
const bay = (i: number, j: number, service = false) => ({
  i: i - 0.45,
  j: j - 0.45,
  w: 0.9,
  d: 0.9,
  service,
})
/** The back row runs along the barn; the three in front of it are the service bays broken agents park in. */
export const BAYS = [
  bay(10.9, 15),
  bay(12.3, 15, true),
  bay(13.7, 15, true),
  bay(15.1, 15, true),
  bay(16.5, 15),
  bay(11.6, 16.45),
  bay(13, 16.45),
  bay(14.4, 16.45),
  bay(15.8, 16.45),
  bay(17.2, 16.45),
] as const
/** The parking's painted bay lines, along j: [i, j0, j1]. */
export const BAY_LINES: readonly (readonly [number, number, number])[] = [
  ...[10.2, 11.6, 13, 14.4, 15.8, 17.2].map((i) => [i, 14.35, 15.35] as const),
  ...[10.9, 12.3, 13.7, 15.1, 16.5, 17.9].map((i) => [i, 15.95, 16.95] as const),
]

/** Every district's plots, in the order things fill them. */
export const PLOTS = {
  /** A team: its house and, beside it, its budget battery. */
  team: [
    { i: 1.65, j: 10.95, w: 2.45, d: 1.3 },
    { i: 6, j: 10.95, w: 2.45, d: 1.3 },
    { i: 6, j: 15.3, w: 2.45, d: 1.3 },
    { i: 1.65, j: 15.3, w: 2.45, d: 1.3 },
  ],
  /** A project: a dirt plot with its flag. */
  project: grid([1.0], [0.25, 2.1], 1.54).concat(grid([2.72], [0.25, 2.1], 1.54)),
  /** A provider: a tent. */
  mine: [
    { i: 12.64, j: 0.31, w: 2.42, d: 2.42 },
    { i: 15.94, j: 0.08, w: 2.2, d: 2.2 },
  ],
  /** An LLM router: the blue trough on stilts the tokens pass under. */
  gate: [
    { i: 11.4, j: 0.9, w: 0.56, d: 1.58 },
    { i: 11.38, j: 3.5, w: 0.56, d: 1.6 },
  ],
  bay: BAYS.map(({ i, j, w, d }) => ({ i, j, w, d })),
  /** A tool: a small white box, two columns deep in front of the MCP gateways. */
  tool: grid([16.33, 17.43], [5.19, 6.13, 7.07, 8.01, 8.95], 0.75),
  /** An MCP gateway: a grey house with its flag. */
  mcp: [
    { i: 14.3, j: 5.4, w: 1.4, d: 1.4 },
    { i: 14.2, j: 7.3, w: 1.4, d: 1.4 },
  ],
  /** A coding tool on the dirt plot: its ball on a marked square. The first is the free plan's; a paid plan opens all. */
  harness: [
    { i: 24.7, j: 5.35, w: 2, d: 2 },
    { i: 19.5, j: 5.7, w: 1.8, d: 1.8 },
    { i: 22.2, j: 6.1, w: 1.8, d: 1.8 },
    { i: 19.4, j: 3.1, w: 1.8, d: 1.8 },
    { i: 19.4, j: 0.4, w: 1.8, d: 1.8 },
  ],
  /** A provider the coding tools call: a brick stack on the dirt. */
  xmine: [
    { i: 25, j: 3.2, w: 1.65, d: 1.65 },
    { i: 25, j: 1, w: 1.7, d: 1.7 },
  ],
} as const satisfies Record<string, readonly Plot[]>
export type Kind = keyof typeof PLOTS

/** Where a district's name is painted: its corner, and the axis the text runs along (clear ground, off the art). */
export const SIGNS: Record<Kind, Spot & { axis: 'i' | 'j' }> = {
  team: { i: 0.5, j: 17.85, axis: 'i' },
  project: { i: 0.35, j: 0.25, axis: 'j' },
  mine: { i: 18.9, j: 0.4, axis: 'j' },
  gate: { i: 10.3, j: 0.75, axis: 'j' },
  bay: { i: 9.5, j: 17.9, axis: 'i' },
  tool: { i: 18.85, j: 5.3, axis: 'j' },
  mcp: { i: 13.75, j: 9.2, axis: 'i' },
  harness: { i: 23.4, j: 7.85, axis: 'i' },
  xmine: { i: 26.85, j: 0.3, axis: 'j' },
}

/** The land a plot sits on. */
export const landOf = (p: Spot) => (p.i >= OUTSIDE.i0 ? OUTSIDE : MAIN)
