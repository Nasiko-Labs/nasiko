/**
 * A world theme for the village (exploration, `explore/village-themes`): the same village, its places, data and
 * interactions, drawn as another world. A theme is data plus optional drawings:
 *
 * - `art` and `ink` repaint every drawing that reads `ART` / `INK` (`palette.ts`): ground, cliffs, paths, the parking,
 *   scenery, shadows. Anything a theme leaves out keeps the meadow's colour.
 * - `skin` swaps a landmark's drawing for the theme's own (same footprint and anchor, so selection, labels, links and
 *   motion stay where they were). A landmark without a skin is drawn as in the meadow, in the theme's colours.
 * - `under` draws scenery below the island (the world around it), `onGround` draws on the island's surface under
 *   everything else (patches, seams), and `backdrop` is the canvas's CSS background.
 */
import type { CSSProperties, ComponentType } from 'react'
import type { Pt } from '../geo'
import type { Mine, Team } from '../model'
import type { ArtKey } from '../palette'

export type WorldId = 'desert' | 'tundra' | 'foundry'

export interface WorldSkin {
  Tree?: ComponentType<{ t: readonly [number, number, number] }>
  Bush?: ComponentType<{ b: readonly [number, number, number] }>
  Tuft?: ComponentType<{ p: Pt }>
  Townhall?: ComponentType<{ i: number; j: number }>
  TeamArt?: ComponentType<{ t: Pick<Team, 'i' | 'j' | 'color' | 'budget' | 'pace'> }>
  MineArt?: ComponentType<{ m: Pick<Mine, 'i' | 'j' | 'w' | 'color' | 'mono'> }>
}

export interface WorldTheme {
  id: WorldId
  label: string
  /** One line for the picker's tooltip. */
  blurb: string
  /** A swatch for the picker. */
  swatch: readonly [string, string]
  art: Partial<Record<ArtKey, string>>
  ink: string
  backdrop?: CSSProperties
  skin: WorldSkin
  under?: ComponentType
  onGround?: ComponentType
  /**
   * The world drawn by the voxel rasterizer (the Nasiko village lab's painted style) under the SVG, which then keeps
   * only its hit targets and overlays. Its drawings above stay as the picture shown while the voxel world bakes.
   */
  voxel?: 'desert' | 'ice' | 'steam'
}
