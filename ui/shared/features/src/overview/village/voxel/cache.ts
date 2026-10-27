/**
 * The last voxel world baked, kept for the page's life. Leaving the village (for the Overview's cards or the Map)
 * unmounts it; coming back shows the same world at once instead of the flat drawing while it bakes again. Apart from
 * `VoxelLayer` so the village can read it without loading the rasterizer.
 */

/** A baked world: its size, where world (0, 0, 1) lands in it, its colour and place-id buffers, and the place keys. */
export interface Baked {
  W: number
  H: number
  OX: number
  OY: number
  col: Uint32Array
  id: Int32Array
  keys: string[]
}

/** The world last baked: its theme, the village data it drew (`worldSig.ts`), and the result. */
export const bakes: { last: { theme: string; sig: string; baked: Baked } | null } = { last: null }

/** The theme of the world on hand, if one has been baked this visit. */
export const lastBakedTheme = () => bakes.last?.theme ?? null
