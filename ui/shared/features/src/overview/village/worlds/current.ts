/**
 * The world theme the village is drawn in right now. Drawings read it at render time (`world().skin.Tree`), the way
 * they read `ART`; `worlds/index.ts` sets it from the village's settings. Kept apart from the registry so the drawings
 * (`art.tsx`, `places.tsx`) can read it without importing every theme.
 */
import type { WorldTheme } from './types'

/** The plain village, only until a theme applies (every world is one of the three voxel themes). */
export const MEADOW: WorldTheme = {
  id: 'desert',
  label: 'Meadow',
  blurb: 'The original: green grass on a floating island.',
  swatch: ['#94d46d', '#c98f52'],
  art: {},
  ink: '#22252b',
  skin: {},
}

let current: WorldTheme = MEADOW

export const world = () => current
export function setWorld(w: WorldTheme) {
  current = w
}
