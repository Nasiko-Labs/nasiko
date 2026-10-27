/**
 * Bakes the voxel village off the main thread: the page keeps scrolling and the flat village keeps answering while
 * the world is drawn. Receives the village's plain data, answers with the colour and place-id buffers (transferred).
 */
import { bake } from './core'

self.onmessage = (e: MessageEvent) => {
  const { seq, village, saved, trees, bushes, theme } = e.data
  const b = bake(village, saved, trees, bushes, theme)
  const col = b.col as unknown as Uint32Array
  const id = b.id as unknown as Int32Array
  ;(self as unknown as Worker).postMessage(
    { seq, W: b.W, H: b.H, OX: b.OX, OY: b.OY, col, id, keys: b.keys, ms: b.ms },
    [col.buffer, id.buffer],
  )
}
