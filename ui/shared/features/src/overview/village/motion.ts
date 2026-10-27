/**
 * The village's life (plans/feat-village-overview.md V24, V52): agents' pieces walk from their bay on the parking to
 * the team houses and project plots that use them and back. A controller outside React, like the map's viewport: it
 * writes `transform` straight to the pieces' groups, so a frame never re-renders the page. The coins run on the
 * ground's own trails (SMIL), not here.
 *
 * Off (reduced motion or the world setting): every piece stands in its bay and nothing moves. Broken pieces stand in
 * front of the barn, asleep ones in their bays, whatever the setting.
 */
import { P, type Pt } from './geo'
import type { Car, Pose, Village } from './model'
import { BEACON } from './palette'
import { route, type Tile } from './streets'

interface Unit {
  id: string
  pose: Pose
  targets: Tile[]
  fi: number
  fj: number
  rest: Tile
  mode: 'wait' | 'drive' | 'still'
  path: Tile[]
  seg: number
  until: number
  at: 'home' | 'away'
  next: number
  face: 1 | -1
  el: SVGGElement | null
  faceEl: Element | null
  beaconEl: Element | null
  glowEl: Element | null
  lastFace: number
  lit: string
}
/** Where each car parks: its bay. */
export function restingSpots(v: Village): Map<string, Tile> {
  return new Map(v.cars.map((c) => [c.id, [c.i, c.j] as Tile]))
}

/** The light a car shows now: green while it drives to work, its resting light otherwise. */
function lightOf(u: Pick<Unit, 'pose' | 'mode' | 'at'>): string {
  if (u.pose === 'down') return BEACON.broken
  if (u.pose === 'idle') return BEACON.asleep
  if (u.pose === 'hurt') return BEACON.slow
  return u.mode === 'drive' || u.at === 'away' ? BEACON.active : BEACON.running
}

export function createMotion() {
  let village: Village | null = null
  let on = false
  let raf = 0
  let last = 0
  const units = new Map<string, Unit>()
  const frameListeners = new Set<() => void>()

  const place = (u: Unit) => {
    if (!u.el) return
    const [x, y] = P(u.fi, u.fj)
    u.el.setAttribute('transform', `translate(${x.toFixed(1)},${y.toFixed(1)})`)
    if (u.faceEl && u.lastFace !== u.face) {
      if (u.face < 0) u.faceEl.setAttribute('transform', 'scale(-1,1)')
      else u.faceEl.removeAttribute('transform')
      u.lastFace = u.face
    }
    const lit = lightOf(u)
    if (lit !== u.lit) {
      u.beaconEl?.setAttribute('fill', lit)
      u.glowEl?.setAttribute('fill', lit)
      u.lit = lit
    }
  }
  const plan = (u: Unit) => {
    if (u.pose === 'idle' || u.pose === 'down' || !u.targets.length) {
      u.mode = 'still'
      return
    }
    if (u.at === 'home') {
      const t = u.targets[u.next++ % u.targets.length] ?? u.rest
      const target: Tile = [t[0] + (Math.random() - 0.5) * 0.5, t[1] + (Math.random() - 0.5) * 0.6]
      u.path = route(u.rest, target)
      u.at = 'away'
    } else {
      u.path = route([u.fi, u.fj], u.rest)
      u.at = 'home'
    }
    u.seg = 1
    u.mode = 'drive'
  }
  const step = (
    o: { fi: number; fj: number; seg: number; path: Tile[] },
    speed: number,
    dt: number,
  ) => {
    const tg = o.path[o.seg]
    if (!tg) return { done: true, dx: 0, dy: 0 }
    const dx = tg[0] - o.fi
    const dy = tg[1] - o.fj
    const dist = Math.hypot(dx, dy)
    const stp = speed * dt
    if (dist <= stp) {
      o.fi = tg[0]
      o.fj = tg[1]
      o.seg++
    } else {
      o.fi += (dx / dist) * stp
      o.fj += (dy / dist) * stp
    }
    return { done: false, dx, dy }
  }
  const tick = (t: number) => {
    const dt = Math.min(0.05, (t - (last || t)) / 1000)
    last = t
    if (village) {
      for (const u of units.values()) {
        if (u.mode === 'wait') {
          if (t >= u.until) plan(u)
        } else if (u.mode === 'drive') {
          const r = step(u, u.pose === 'hurt' ? 0.7 : 1.7, dt)
          if (r.done) {
            u.mode = 'wait'
            u.until =
              t + (u.at === 'away' ? 2400 + Math.random() * 3200 : 1800 + Math.random() * 2600)
            place(u)
            continue
          }
          if (Math.abs(r.dx - r.dy) > 0.02) u.face = r.dx - r.dy < 0 ? -1 : 1
          place(u)
        }
      }
    }
    frameListeners.forEach((f) => f())
    raf = requestAnimationFrame(tick)
  }
  const stop = () => {
    if (raf) cancelAnimationFrame(raf)
    raf = 0
    last = 0
  }
  const start = () => {
    if (raf || !on || typeof requestAnimationFrame !== 'function') return
    raf = requestAnimationFrame(tick)
  }
  const park = (u: Unit) => {
    u.fi = u.rest[0]
    u.fj = u.rest[1]
    u.at = 'home'
    if (u.mode === 'drive') u.mode = 'wait'
    place(u)
  }

  return {
    /** Binds the pieces' groups after a render, keeping each piece's position across renders. */
    bind(v: Village, unitsLayer: SVGGElement) {
      village = v
      const seen = new Set<string>()
      const els = new Map<string, SVGGElement>()
      unitsLayer.querySelectorAll<SVGGElement>('[data-unit]').forEach((el) => {
        if (el.dataset.unit) els.set(el.dataset.unit, el)
      })
      v.cars.forEach((c: Car, k) => {
        seen.add(c.id)
        const spot: Tile = [c.i, c.j]
        let u = units.get(c.id)
        const moving = (c.pose === 'live' || c.pose === 'hurt') && c.targets.length > 0
        if (!u) {
          u = {
            id: c.id,
            pose: c.pose,
            targets: c.targets,
            fi: spot[0],
            fj: spot[1],
            rest: spot,
            mode: moving ? 'wait' : 'still',
            path: [],
            seg: 0,
            until: performance.now() + Math.random() * 2500 + k * 60,
            at: 'home',
            next: k,
            face: 1,
            el: null,
            faceEl: null,
            beaconEl: null,
            glowEl: null,
            lastFace: 1,
            lit: '',
          }
          units.set(c.id, u)
        }
        const wasStill = u.mode === 'still'
        const moved = u.rest[0] !== spot[0] || u.rest[1] !== spot[1]
        u.pose = c.pose
        u.targets = c.targets
        u.rest = spot
        if (!moving || !on || moved) {
          u.fi = spot[0]
          u.fj = spot[1]
          u.mode = moving ? 'wait' : 'still'
          u.at = 'home'
        } else if (wasStill) {
          // Back up (restarted, woken): it drives out from its bay.
          u.mode = 'wait'
          u.at = 'home'
          u.until = performance.now() + 300
        }
        u.el = els.get(c.id) ?? null
        u.faceEl = u.el?.querySelector('[data-face]') ?? null
        u.beaconEl = u.el?.querySelector('[data-beacon]') ?? null
        u.glowEl = u.el?.querySelector('[data-glow]') ?? null
        u.lastFace = 1
        u.lit = ''
        place(u)
      })
      for (const id of [...units.keys()]) if (!seen.has(id)) units.delete(id)
    },
    /** Motion on or off. */
    set(motion: boolean) {
      if (motion === on) return
      on = motion
      if (!on) {
        stop()
        for (const u of units.values()) park(u)
      } else start()
    },
    /** Where a car is now, in tiles. */
    at(id: string): Tile | null {
      const u = units.get(id)
      return u ? [u.fi, u.fj] : null
    },
    onFrame(f: () => void) {
      frameListeners.add(f)
      return () => void frameListeners.delete(f)
    },
    dispose() {
      // StrictMode remounts effects: off, so the next `set` starts the loop again.
      on = false
      stop()
      units.clear()
      frameListeners.clear()
    },
  }
}
export type Motion = ReturnType<typeof createMotion>
export type { Pt }
