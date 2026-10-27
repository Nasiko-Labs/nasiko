/**
 * The painted voxel world under the SVG (exploration, the desert theme): the Nasiko village lab's rasterizer bakes the
 * whole village once, in a worker, into one image whose tiles are the SVG's tiles (48 × 24 px), and this layer only
 * draws that image at the viewport's pan and zoom. Hover and the selection's dimming re-tint the baked pixels by
 * place. A bake runs again only when something drawn changes (`worldSig.ts`).
 */
import { useEffect, useRef } from 'react'
import type { ViewState } from '../../map/useViewport'
import { WORLD } from '../geo'
import type { Village } from '../model'
import { HARNESS_ART, HARNESS_FALLBACK } from '../palette'
import { BUSHES, TREES } from '../scenery'
import type { HoverStore } from '../stores'
import { signature } from '../worldSig'
import { bakes, type Baked } from './cache'
import { compose } from './core'

/** How long a newly baked world takes to fade in over the last one. */
const FADE_MS = 250
const reducedMotion = () => window.matchMedia?.('(prefers-reduced-motion: reduce)').matches ?? false

/** The village's data the bake reads, as plain values a worker can receive. */
function plain(v: Village) {
  return {
    core: v.core,
    hud: { budget: v.hud.budget },
    teams: v.teams.map((t) => ({
      id: t.id,
      i: t.i,
      j: t.j,
      color: t.color,
      budget: t.budget,
      pace: t.pace,
    })),
    mines: v.mines.map((m) => ({
      id: m.id,
      i: m.i,
      j: m.j,
      w: m.w,
      d: m.d,
      color: m.color,
      mono: m.mono,
    })),
    xmines: v.xmines.map((m) => ({
      id: m.id,
      i: m.i,
      j: m.j,
      w: m.w,
      d: m.d,
      color: m.color,
      mono: m.mono,
    })),
    gates: v.gates.map((g) => ({ id: g.id, i: g.i, j: g.j, w: g.w, d: g.d })),
    mcps: v.mcps.map((m) => ({ id: m.id, i: m.i, j: m.j, w: m.w, d: m.d, weather: m.weather })),
    tools: v.tools.map((m) => ({ id: m.id, i: m.i, j: m.j })),
    outpost: !!v.outpost,
    log: { upgrade: { length: v.log.upgrade.length } },
    cars: v.cars.map((c) => ({ pose: c.pose })),
    harnesses: v.harnesses.map((h) => ({
      kind: h.kind,
      i: h.i,
      j: h.j,
      w: h.w,
      d: h.d,
      art: HARNESS_ART[h.kind] ?? HARNESS_FALLBACK,
    })),
  }
}

export type VoxelTheme = 'desert' | 'ice' | 'steam'

export function VoxelLayer({
  theme,
  village,
  saved,
  dim,
  hover,
  subscribe,
  viewNow,
  onReady,
}: {
  theme: VoxelTheme
  village: Village
  saved: number | null
  dim: string
  hover: HoverStore
  subscribe(l: (v: ViewState) => void): () => void
  viewNow(): ViewState
  onReady?(): void
}) {
  const canvas = useRef<HTMLCanvasElement>(null)
  const baked = useRef<Baked | null>(null)
  const image = useRef<HTMLCanvasElement | null>(null)
  const view = useRef<ViewState>(viewNow())
  const dimRef = useRef(dim)
  const readyRef = useRef(onReady)
  // Declared before the effects that read them, so those see this render's values.
  useEffect(() => {
    dimRef.current = dim
    readyRef.current = onReady
  })
  const worker = useRef<Worker | null>(null)
  const seq = useRef(0)
  /** What the pending bake was asked to draw, so its result can be kept as the last world. */
  const asked = useRef({ theme, sig: '' })
  // A newly baked world (another theme, or changed data) fades in over the one before it, never through a blank.
  const leaving = useRef<{ img: HTMLCanvasElement; b: Baked } | null>(null)
  /** How far the new world has faded in, 0 to 1 (advanced by animation frames, `fadeIn`). */
  const fade = useRef(1)

  const draw = () => {
    const c = canvas.current
    const img = image.current
    const b = baked.current
    if (!c || !img || !b) return
    const dpr = Math.min(2, window.devicePixelRatio || 1)
    const w = Math.round(c.clientWidth * dpr)
    const h = Math.round(c.clientHeight * dpr)
    if (c.width !== w || c.height !== h) {
      c.width = w
      c.height = h
    }
    const g = c.getContext('2d')
    if (!g) return
    // The viewport may have moved (fitted, eased) since this layer last heard: always draw where it is now.
    view.current = viewNow()
    const { x, y, z } = view.current
    g.setTransform(1, 0, 0, 1, 0, 0)
    g.clearRect(0, 0, w, h)
    // Crisp voxels when drawn at or above their size, smooth when shrunk.
    g.imageSmoothingEnabled = z * dpr < 0.99
    const k = z * dpr
    // Buffer pixel (bx, by) is SVG point (bx − OX, by − OY + 24): the land's top is one height unit (24 px) up.
    const paint = (from: HTMLCanvasElement, at: Baked, alpha: number) => {
      g.globalAlpha = alpha
      g.setTransform(k, 0, 0, k, k * (WORLD.ox - at.OX - x), k * (WORLD.oy - at.OY + 24 - y))
      g.drawImage(from, 0, 0)
    }
    const old = leaving.current
    if (old) paint(old.img, old.b, 1 - fade.current)
    paint(img, b, fade.current)
    g.globalAlpha = 1
  }

  /** Fades the new world in over [`FADE_MS`], one frame at a time, then lets the old one (if any) go. */
  const fadeIn = () => {
    fade.current = 0
    let start = -1
    const tick = (now: number) => {
      if (start < 0) start = now
      fade.current = Math.min(1, (now - start) / FADE_MS)
      if (fade.current >= 1) leaving.current = null
      draw()
      if (fade.current < 1) requestAnimationFrame(tick)
    }
    requestAnimationFrame(tick)
  }

  const recolor = () => {
    const b = baked.current
    if (!b) return
    const px = compose(b, new Set(dimRef.current ? dimRef.current.split('|') : []), hover.get())
    const img = image.current ?? document.createElement('canvas')
    img.width = b.W
    img.height = b.H
    img.getContext('2d')?.putImageData(new ImageData(px, b.W, b.H), 0, 0)
    image.current = img
    draw()
  }

  // One worker for the layer's life.
  useEffect(() => {
    // No workers (tests, very old browsers): the flat village stays.
    if (typeof Worker === 'undefined') return
    const w = new Worker(new URL('./bake.worker.ts', import.meta.url), { type: 'module' })
    w.onmessage = (e: MessageEvent<Baked & { seq: number; ms: number }>) => {
      if (e.data.seq !== seq.current) return
      // Keep the world on screen as the one fading out; the new one gets a canvas of its own.
      const before = image.current
      const prev = baked.current
      // Every world fades in: the first over the empty canvas, the next ones over the last.
      const fading = !reducedMotion()
      if (fading && before && prev) {
        leaving.current = { img: before, b: prev }
        image.current = null
      }
      // Before the first draw below, so the new world starts invisible rather than flashing in for a frame.
      if (fading) fade.current = 0
      baked.current = e.data
      bakes.last = { ...asked.current, baked: e.data }
      recolor()
      if (fading) fadeIn()
      readyRef.current?.()
    }
    worker.current = w
    return () => {
      w.terminate()
      worker.current = null
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps -- the worker's handler reads refs only
  }, [])

  // A world already baked this visit shows at once (coming back from the Overview's cards or the Map). Before the
  // effects below, so the bake one skips it when nothing changed, and the viewport's first apply draws it.
  useEffect(() => {
    const last = bakes.last
    if (!last) return
    baked.current = last.baked
    recolor()
    // eslint-disable-next-line react-hooks/exhaustive-deps -- once, on mount
  }, [])

  // Bake again only when something drawn changed.
  const sig = signature(village, saved)
  useEffect(() => {
    const last = bakes.last
    if (last && last.theme === theme && last.sig === sig && baked.current === last.baked) return
    seq.current += 1
    asked.current = { theme, sig }
    worker.current?.postMessage({
      seq: seq.current,
      village: plain(village),
      saved,
      trees: TREES,
      bushes: BUSHES,
      theme,
    })
    // eslint-disable-next-line react-hooks/exhaustive-deps -- `sig` stands for the village's drawn data
  }, [sig, theme])

  // eslint-disable-next-line react-hooks/exhaustive-deps -- recolor reads refs
  useEffect(() => recolor(), [dim])
  // eslint-disable-next-line react-hooks/exhaustive-deps -- recolor reads refs
  useEffect(() => hover.subscribe(recolor), [hover])
  useEffect(
    () =>
      subscribe((v) => {
        view.current = v
        draw()
      }),
    // eslint-disable-next-line react-hooks/exhaustive-deps -- draw reads refs
    [subscribe],
  )
  useEffect(() => {
    const c = canvas.current
    if (!c) return
    const ro = new ResizeObserver(() => draw())
    ro.observe(c)
    return () => ro.disconnect()
    // eslint-disable-next-line react-hooks/exhaustive-deps -- draw reads refs
  }, [])

  return (
    <canvas ref={canvas} aria-hidden className="pointer-events-none absolute inset-0 size-full" />
  )
}
