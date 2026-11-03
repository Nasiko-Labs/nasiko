/**
 * Pan and zoom for the map: the SVG's viewBox, moved by dragging empty space, Ctrl/⌘ + wheel, the zoom buttons and
 * the keyboard (arrows pan, + and − zoom, 0 fits). The viewBox is written straight to the element, so a drag never
 * re-renders the map; only the zoom percentage is state. Fit leaves room for the insight panel when it sits over the
 * canvas (`reserve`), so nothing hides behind it. The view refits on resize until the person moves it.
 *
 * The controller lives outside React (created once per canvas): its listeners and the viewBox are DOM work.
 */
import { useEffect, useRef, useState, type RefObject } from 'react'

/** From this canvas width the insight panel sits over the map (Canvas.tsx). */
export const OVERLAY = 980

interface Size {
  width: number
  height: number
  /** Where the fitted area starts in map units, when it reaches past the map's origin (0, 0 when absent). */
  x0?: number
  y0?: number
}

/** Options: `anywhere` lets a drag start on a node too (the village is dense); it captures the pointer only once
 * the drag moves, so a plain click still reaches the node. */
interface ViewportOptions {
  anywhere?: boolean
  /** A change of `reserve` (a drawer opening) eases the fitted view over instead of jumping (the board). */
  easeReserve?: boolean
}

/** What the view shows: the viewBox's top-left in map units and its zoom (screen px per map unit). */
export interface ViewState {
  x: number
  y: number
  z: number
}

function createViewport(setPct: (n: number) => void, opts: ViewportOptions = {}) {
  let el: HTMLDivElement | null = null
  const listeners = new Set<(v: ViewState) => void>()
  let tween = 0
  let size: Size = { width: 1, height: 1 }
  let reserve = 0
  let vp = { z: 1, x: 0, y: 0, fit: 1, auto: true }
  let moved = false

  const apply = () => {
    const svg = el?.querySelector('svg')
    if (!el || !svg) return
    const { z, x, y } = vp
    svg.setAttribute(
      'viewBox',
      `${x.toFixed(1)} ${y.toFixed(1)} ${(el.clientWidth / z).toFixed(1)} ${(el.clientHeight / z).toFixed(1)}`,
    )
    listeners.forEach((l) => l({ x, y, z }))
  }
  const side = () => (el && el.clientWidth >= OVERLAY ? reserve : 0)
  /** The view that fits the whole map in the free part of the canvas; null before the canvas has a size. */
  const fitTarget = () => {
    if (!el || !el.clientWidth || !el.clientHeight) return null
    const top = 36
    const bottom = 50
    const aw = el.clientWidth - side() - 32
    const ah = el.clientHeight - top - bottom
    // On a phone the whole map would be too small to read: it opens at a readable size and pans sideways.
    const z = Math.max(
      el.clientWidth < 640 ? 0.5 : 0.2,
      Math.min(aw / size.width, ah / size.height, 1.25),
    )
    return {
      z,
      x: (size.x0 ?? 0) - 16 / z - Math.max(0, (aw / z - size.width) / 2),
      y: (size.y0 ?? 0) - top / z - Math.max(0, (ah / z - size.height) / 2),
    }
  }
  const fit = () => {
    const t = fitTarget()
    if (!t) return
    if (tween) cancelAnimationFrame(tween)
    tween = 0
    vp = { ...t, fit: t.z, auto: true }
    setPct(100)
    apply()
  }
  const zoomAt = (factor: number, px?: number, py?: number) => {
    if (!el) return
    const cx = px ?? (el.clientWidth - side()) / 2
    const cy = py ?? el.clientHeight / 2
    const nz = Math.max(vp.fit * 0.5, Math.min(vp.fit * 4, vp.z * factor))
    const mx = vp.x + cx / vp.z
    const my = vp.y + cy / vp.z
    vp = { ...vp, z: nz, x: mx - cx / nz, y: my - cy / nz, auto: false }
    setPct(Math.round((nz / vp.fit) * 100))
    apply()
  }
  const pan = (dx: number, dy: number) => {
    vp = { ...vp, x: vp.x + dx / vp.z, y: vp.y + dy / vp.z, auto: false }
    apply()
  }
  const reapply = () => (vp.auto ? fit() : apply())
  /** As reapply, but a fitted view eases to its new fit. */
  const refitSmooth = () => (vp.auto ? fitSmooth() : apply())
  /** Eases the view to `to` (unless `instant`), then marks it moved by the person or, for a fit, auto again. */
  const tweenTo = (to: { x: number; y: number; z: number }, instant: boolean, auto = false) => {
    if (tween) cancelAnimationFrame(tween)
    tween = 0
    const done = () => {
      vp = { ...vp, ...to, auto }
      setPct(Math.round((vp.z / vp.fit) * 100))
      apply()
    }
    if (instant || typeof requestAnimationFrame !== 'function') return done()
    const from = { x: vp.x, y: vp.y, z: vp.z }
    const t0 = performance.now()
    const step = (t: number) => {
      const k = Math.min(1, (t - t0) / 480)
      const e = 1 - Math.pow(1 - k, 3)
      vp = {
        ...vp,
        x: from.x + (to.x - from.x) * e,
        y: from.y + (to.y - from.y) * e,
        z: from.z + (to.z - from.z) * e,
        auto: false,
      }
      apply()
      if (k < 1) tween = requestAnimationFrame(step)
      else {
        tween = 0
        done()
      }
    }
    tween = requestAnimationFrame(step)
  }
  /** Puts a map point in the middle of the free part of the canvas, eased unless `instant`. */
  const center = (mx: number, my: number, zoom?: number, instant = false) => {
    if (!el) return
    const z = Math.max(vp.fit * 0.5, Math.min(vp.fit * 4, zoom ?? Math.max(vp.z, vp.fit * 1.4)))
    tweenTo(
      { z, x: mx - (el.clientWidth - side()) / 2 / z, y: my - el.clientHeight / 2 / z },
      instant,
    )
  }
  /** Back to the whole map, eased unless `instant` (the village's click on open ground). */
  const fitSmooth = (instant = false) => {
    const t = fitTarget()
    if (!t) return
    vp = { ...vp, fit: t.z }
    tweenTo(t, instant, true)
  }

  /** Wires the element's listeners; returns the cleanup. */
  const attach = (node: HTMLDivElement) => {
    el = node
    let drag: { x: number; y: number; vx: number; vy: number; id: number } | null = null
    const down = (e: PointerEvent) => {
      if (e.button !== 0) return
      if (!opts.anywhere && (e.target as Element).closest('[role="button"]')) return
      if ((e.target as Element).closest('[data-vp-ignore]')) return
      drag = { x: e.clientX, y: e.clientY, vx: vp.x, vy: vp.y, id: e.pointerId }
      moved = false
      if (!opts.anywhere) {
        node.setPointerCapture?.(e.pointerId)
        node.dataset.dragging = 'true'
      }
    }
    const move = (e: PointerEvent) => {
      if (!drag) return
      const dx = e.clientX - drag.x
      const dy = e.clientY - drag.y
      if (!moved && Math.abs(dx) + Math.abs(dy) > 3) {
        moved = true
        if (tween) cancelAnimationFrame(tween)
        if (opts.anywhere) {
          node.setPointerCapture?.(drag.id)
          node.dataset.dragging = 'true'
        }
      }
      if (opts.anywhere && !moved) return
      vp = { ...vp, x: drag.vx - dx / vp.z, y: drag.vy - dy / vp.z, auto: false }
      apply()
    }
    const up = () => {
      if (!drag) return
      node.releasePointerCapture?.(drag.id)
      drag = null
      delete node.dataset.dragging
    }
    const wheel = (e: WheelEvent) => {
      if (!(e.ctrlKey || e.metaKey)) return
      e.preventDefault()
      const r = node.getBoundingClientRect()
      zoomAt(Math.exp(-e.deltaY * 0.0025), e.clientX - r.left, e.clientY - r.top)
    }
    const key = (e: KeyboardEvent) => {
      if (e.target !== node) return
      const step = 48
      const keys: Record<string, () => void> = {
        ArrowLeft: () => pan(-step, 0),
        ArrowRight: () => pan(step, 0),
        ArrowUp: () => pan(0, -step),
        ArrowDown: () => pan(0, step),
        '+': () => zoomAt(1.25),
        '=': () => zoomAt(1.25),
        '-': () => zoomAt(0.8),
        '0': fit,
      }
      const fn = keys[e.key]
      if (fn) {
        e.preventDefault()
        fn()
      }
    }
    // A drag that moved swallows the click that ends it, so letting go over a node doesn't select it.
    const click = (e: MouseEvent) => {
      if (!moved) return
      moved = false
      e.stopPropagation()
      e.preventDefault()
    }
    const ro = new ResizeObserver(reapply)
    ro.observe(node)
    node.addEventListener('pointerdown', down)
    node.addEventListener('pointermove', move)
    node.addEventListener('pointerup', up)
    node.addEventListener('pointercancel', up)
    node.addEventListener('wheel', wheel, { passive: false })
    node.addEventListener('keydown', key)
    node.addEventListener('click', click, true)
    return () => {
      ro.disconnect()
      node.removeEventListener('pointerdown', down)
      node.removeEventListener('pointermove', move)
      node.removeEventListener('pointerup', up)
      node.removeEventListener('pointercancel', up)
      node.removeEventListener('wheel', wheel)
      node.removeEventListener('keydown', key)
      node.removeEventListener('click', click, true)
      if (el === node) el = null
    }
  }
  return {
    attach,
    configure(s: Size, r: number) {
      size = s
      reserve = r
    },
    fit,
    fitSmooth,
    reapply,
    refitSmooth,
    /** Keeps the view where it is through the next resize or reserve change (a pick on the board). */
    hold: () => {
      vp = { ...vp, auto: false }
    },
    center,
    zoomIn: () => zoomAt(1.25),
    zoomOut: () => zoomAt(0.8),
    view: (): ViewState => ({ x: vp.x, y: vp.y, z: vp.z }),
    subscribe(l: (v: ViewState) => void) {
      listeners.add(l)
      return () => void listeners.delete(l)
    },
  }
}

export function useViewport(
  zone: RefObject<HTMLDivElement | null>,
  size: Size,
  reserve: number,
  opts?: ViewportOptions,
) {
  const [pct, setPct] = useState(100)
  const [ctl] = useState(() => createViewport(setPct, opts))
  const { width, height, x0, y0 } = size
  const ease = !!opts?.easeReserve
  const lastReserve = useRef(reserve)
  useEffect(() => {
    const moved = lastReserve.current !== reserve
    lastReserve.current = reserve
    ctl.configure({ width, height, x0, y0 }, reserve)
    if (moved && ease) ctl.refitSmooth()
    else ctl.reapply()
  }, [ctl, width, height, x0, y0, reserve, ease])
  useEffect(() => {
    const node = zone.current
    if (!node) return
    const detach = ctl.attach(node)
    ctl.reapply()
    return detach
  }, [ctl, zone])
  return { pct, ...ctl }
}
