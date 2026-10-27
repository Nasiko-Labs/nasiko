/**
 * The world settings (the gear in the village's top bar): labels, motion, token flow and the tile grid, plus the plots
 * picked while placing (V28: where the next new thing of a kind lands, until a shared layout exists, V-4). Per viewer,
 * in this browser only (a convenience, like the theme), in a zustand `persist` store under `openruntime.village`.
 * Storage that throws (blocked, private mode) leaves the defaults for the page's lifetime.
 */
import { create } from 'zustand'
import { createJSONStorage, persist } from 'zustand/middleware'
import type { Claim, Claims } from './model'
import type { WorldId } from './worlds/types'
import type { Kind } from './zones'

export interface VillagePrefs {
  /** `key`: the town hall, the battery and whatever needs attention; the rest on hover, selection or zoom. */
  labels: 'key' | 'always' | 'hover' | 'off'
  motion: 'full' | 'reduced'
  flow: boolean
  grid: boolean
  claims: Claims
  /** The world theme the village is drawn in (exploration: `worlds/`). */
  world: WorldId
}

export const PREFS_KEY = 'openruntime.village'
export const DEFAULT_PREFS: VillagePrefs = {
  labels: 'key',
  motion: 'full',
  flow: true,
  grid: false,
  claims: {},
  world: 'desert',
}

const safe = createJSONStorage(() => ({
  getItem: (k: string) => {
    try {
      return localStorage.getItem(k)
    } catch {
      return null
    }
  },
  setItem: (k: string, v: string) => {
    try {
      localStorage.setItem(k, v)
    } catch {
      // Blocked storage: the choice lasts for the page's lifetime.
    }
  },
  removeItem: (k: string) => {
    try {
      localStorage.removeItem(k)
    } catch {
      // As above.
    }
  },
}))

export const useVillagePrefs = create<
  VillagePrefs & { set(p: Partial<VillagePrefs>): void; claim(kind: Kind, c: Claim): void }
>()(
  persist(
    (set) => ({
      ...DEFAULT_PREFS,
      set: (p) => set(p),
      claim: (kind, c) => set((s) => ({ claims: { ...s.claims, [kind]: c } })),
    }),
    {
      name: PREFS_KEY,
      storage: safe,
      // v1: the quieter `key` labels became the default. v2: the meadow is gone; its viewers see the desert.
      version: 2,
      migrate: (old, from) => {
        const o = old as Omit<VillagePrefs, 'world'> & { world?: string }
        return {
          ...o,
          labels: from < 1 ? 'key' : o.labels,
          world: o.world === 'meadow' || !o.world ? 'desert' : (o.world as VillagePrefs['world']),
        } as VillagePrefs
      },
      partialize: ({ labels, motion, flow, grid, claims, world }) => ({
        labels,
        motion,
        flow,
        grid,
        claims,
        world,
      }),
    },
  ),
)
