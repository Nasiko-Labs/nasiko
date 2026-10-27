/**
 * The village's art palette (plans/feat-village-overview.md decisions V3, V50): the reference design's own colours
 * (playground ref-026, its Figma export), one dark ink for every outline. Grass, dirt, roofs and pieces are a picture,
 * the same in light and dark and in every colour theme, like the login showcase's palette. Nothing here is a UI colour: controls, panels and text use theme tokens, and status marks on the
 * art (chips, badges) use the status tokens.
 */

function hex2rgb(h: string): number[] {
  const s = h.replace('#', '')
  const f = s.length === 3 ? [...s].map((c) => c + c).join('') : s
  return [0, 2, 4].map((k) => Number.parseInt(f.slice(k, k + 2), 16))
}

/** Lighten (a > 0) or darken (a < 0) a hex colour by the fraction |a|. */
export function shade(h: string, a: number): string {
  const t = a < 0 ? 0 : 255
  const f = Math.abs(a)
  return `#${hex2rgb(h)
    .map((v) =>
      Math.round(v + (t - v) * f)
        .toString(16)
        .padStart(2, '0'),
    )
    .join('')}`
}

/** The one ink every outline is drawn in (the reference design's #22252B). A world theme may swap it (`worlds/`). */
export let INK = '#22252b'

/** The meadow's colours: the reference design's own. */
export const MEADOW_ART = {
  // The main island: grass on a dirt slab, a checker of lighter tiles.
  grass: '#94d46d',
  grassTile: '#8bcc65',
  grassLipLeft: '#74b350',
  grassLipRight: '#619643',
  cliffLeft: '#c98f52',
  cliffRight: '#a07241',
  tuft: '#5ea544',
  // The sand strips and the dirt (the projects' plots and the coding tools' plot).
  sand: '#ecdcab',
  dirt: '#d2a774',
  dirtTile: '#c99d69',
  dirtLine: '#a97a4b',
  // The parking.
  asphalt: '#cdd1d8',
  asphaltLine: '#6c727d',
  bayLine: '#ffffff',
  // Gold: tokens, batteries, the celebration banner.
  gold: '#f5c22e',
  goldHi: '#ffe27a',
  goldLine: '#c99a1b',
  coin: '#f6c431',
  glass: 'rgba(220,236,243,.75)',
  glassTop: '#eef7fb',
  cap: '#9aa3ad',
  capTop: '#b8c0c8',
  shadow: 'rgba(0,0,0,.13)',
  sideShade: 'rgba(0,0,0,.13)',
  over: '#e5484d',
  // Spend no budget governs (V58): a red outline, its halo and a faint wash over the footprint.
  ungoverned: '#e5484d',
  ungovernedHalo: 'rgba(229,72,77,.32)',
  ungovernedFill: 'rgba(229,72,77,.08)',
  // Trees and bushes.
  leaf: '#3aa84a',
  leafHi: '#71c07c',
  trunk: '#7a4e2d',
  // The setup's scaffolding.
  wood: '#7a5532',
  plank: '#a26f3c',
  post: '#83552b',
  scaffold: '#a8743f',
  ghost: '#9b8a5e',
  // Plots: empty, open for placing, locked on this plan.
  plot: 'rgba(255,255,255,.28)',
  plotLine: 'rgba(60,80,40,.45)',
  plotOpen: 'rgba(76,201,107,.42)',
  plotOpenLine: '#2f9e4f',
  plotLocked: 'rgba(80,80,90,.18)',
} as const

export type ArtKey = keyof typeof MEADOW_ART
/**
 * The colours the village draws with now: the meadow's, unless a world theme (`worlds/`) has painted over them. Every
 * drawing reads it at render time, so a theme change re-renders the world (keyed by the theme) in the new colours.
 */
export const ART: Record<ArtKey, string> = { ...MEADOW_ART }

/** Repaint the village: the meadow's colours with a theme's on top, and its ink. */
export function paintVillage(art: Partial<Record<ArtKey, string>>, ink: string) {
  Object.assign(ART, MEADOW_ART, art)
  INK = ink
}

/**
 * An agent's piece (V52): its colour says how it is. White running (and walking to work), orange slow, red broken,
 * pale grey asleep. Shape carries it too: an asleep piece has its z, a broken one stands in front of the barn.
 */
export const BEACON = {
  active: '#ffffff',
  running: '#f7f7f7',
  slow: '#f0954a',
  broken: '#d8392e',
  asleep: '#d4d4d6',
} as const

/** The local harnesses' bots: our own drawing in a brand-like colour, never a copied mark (V19). */
export const HARNESS_ART: Record<string, { body: string; face: string; trim: string }> = {
  claude: { body: '#d97757', face: '#fbe3d6', trim: '#9a4a2c' },
  codex: { body: '#2b2f36', face: '#d8f5ea', trim: '#10a37f' },
  cursor: { body: '#3d3d46', face: '#e8e8ee', trim: '#9ca3af' },
  opencode: { body: '#4b5563', face: '#e5e7eb', trim: '#f59e0b' },
  windsurf: { body: '#0e9f9f', face: '#e0fbfb', trim: '#066b6b' },
  grok: { body: '#151517', face: '#f4f4f5', trim: '#a1a1aa' },
}
export const HARNESS_FALLBACK = { body: '#7c7f8a', face: '#eef0f3', trim: '#4b4f59' }

/** Team roofs, by house: the reference's magenta, rose, green and purple, then more for a paid plan's teams. */
export const TEAM_COLORS: readonly [string, ...string[]] = [
  '#d94acb',
  '#e0457b',
  '#16a34a',
  '#a855f7',
  '#e8833a',
  '#4479e0',
  '#14a3a3',
]

/** Provider mines: a known brand's colour and monogram, else a neutral one. */
export const PROVIDER_ART: Record<string, { color: string; mono: string }> = {
  anthropic: { color: '#c96f4a', mono: 'A' },
  openai: { color: '#109b77', mono: 'O' },
  gemini: { color: '#4285f4', mono: 'G' },
  google: { color: '#4285f4', mono: 'G' },
  mistral: { color: '#f07a12', mono: 'M' },
  cursor: { color: '#3d3d46', mono: 'C' },
}
export const PROVIDER_FALLBACK = '#8a8a96'
