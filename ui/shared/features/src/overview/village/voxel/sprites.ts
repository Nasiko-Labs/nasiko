/** Agents' pixel sprites for the voxel world, as image URLs the SVG pieces show (cached per state). */
import type { Beacon } from '../model'
import { pieceSprite } from './core'

const cache = new Map<string, { href: string; w: number; h: number }>()

export function agentSprite(beacon: Beacon, theme: string) {
  const hit = cache.get(beacon + theme)
  if (hit) return hit
  const s = pieceSprite(beacon, theme) as {
    w: number
    h: number
    data: Uint8ClampedArray<ArrayBuffer>
  }
  const c = document.createElement('canvas')
  c.width = s.w
  c.height = s.h
  c.getContext('2d')?.putImageData(new ImageData(s.data, s.w, s.h), 0, 0)
  const out = { href: c.toDataURL(), w: s.w, h: s.h }
  cache.set(beacon + theme, out)
  return out
}
