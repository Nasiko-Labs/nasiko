/**
 * The village's world themes (exploration): the registry, and the switch that repaints the village when the choice in
 * its settings changes. `?world=desert|tundra|foundry` picks one from a link (handy for sharing a look).
 */
import { paintVillage } from '../palette'
import { useVillagePrefs } from '../prefs'
import { setWorld } from './current'
import { DESERT, FOUNDRY, TUNDRA } from './themes'
import type { WorldId, WorldTheme } from './types'

export type { WorldId, WorldTheme } from './types'

export const WORLDS: Record<WorldId, WorldTheme> = {
  desert: DESERT,
  tundra: TUNDRA,
  foundry: FOUNDRY,
}
export const WORLD_IDS = Object.keys(WORLDS) as WorldId[]

function apply(id: WorldId) {
  const w = WORLDS[id] ?? DESERT
  setWorld(w)
  paintVillage(w.art, w.ink)
}

try {
  const fromUrl = new URLSearchParams(window.location.search).get('world')
  if (fromUrl && fromUrl in WORLDS) useVillagePrefs.getState().set({ world: fromUrl as WorldId })
} catch {
  // No location (tests): the saved choice stands.
}
apply(useVillagePrefs.getState().world)
useVillagePrefs.subscribe((s, prev) => {
  if (s.world !== prev.world) apply(s.world)
})

/** The village's world theme, re-rendering on change. */
export const useWorld = () => WORLDS[useVillagePrefs((s) => s.world)] ?? DESERT
