/**
 * Kiko as the cactus in a pot (the Weave character lab's finalized cactus, `website/weave-monster`): the lab's own
 * puppet geometry (`./character`, copied unchanged) rendered from a pose every frame. The lab's full rig stays in the
 * lab; this driver keeps what the guide needs, the way the rig does it: every value eases on a critically damped
 * spring (eyes lead, the head follows, the body follows the head), secondary springs make the column and arms lag and
 * settle after a move, hovering perks Kiko up and leans it toward the pointer, a click is a springy hop, and each mood
 * is a face the springs morph into. Reduced motion draws the mood at rest.
 */
import { useEffect, useMemo, useRef } from 'react'
import { zeroPose, type Pose } from './anim/channels'
import { buildScene, restFrame } from './character/scene'
import { sceneMarkup } from './character/svgString'
import { defaultConfig } from './config/schema'

export type KikoMood = 'calm' | 'hand' | 'sign' | 'flare'

/** Face and arm values per mood (the lab's expressions: content, attentive, concerned, worried). */
const MOODS: Record<KikoMood, Partial<Pose>> = {
  calm: { mouthSmile: 0.35 },
  hand: { g_wide: 0.5, pupil: 0.03, mouthSmile: 0.45, gazeY: -0.1, armR: 22 },
  sign: { g_slant: 0.7, gazeY: 0.25, mouthSmile: -0.6, mouthEnds: -0.8, fxQuestion: 1 },
  flare: { g_slant: 0.9, g_wide: 0.2, mouthWave: 0.5, mouthSmile: -0.5, armL: 10, armR: 10 },
}
/** Channels a mood or hover sets; each eases on its own spring when the mood changes. */
const FACE = [
  'g_wide',
  'g_slant',
  'g_happy',
  'pupil',
  'gazeY',
  'mouthSmile',
  'mouthEnds',
  'mouthWave',
  'fxQuestion',
  'armL',
  'armR',
] as const satisfies readonly (keyof Pose)[]

interface S {
  x: number
  v: number
}
const s0 = (): S => ({ x: 0, v: 0 })

/** Exact critically damped spring step (the lab's `spring`): stable for any frame time. */
function spring(s: S, target: number, omega: number, dt: number) {
  const e = Math.exp(-omega * dt)
  const dx = s.x - target
  const tmp = (s.v + omega * dx) * dt
  s.x = target + (dx + tmp) * e
  s.v = (s.v - omega * tmp) * e
}
/** An underdamped spring, for follow-through that overshoots once and settles. */
function wobble(s: S, target: number, w: number, z: number, dt: number) {
  let t = dt
  while (t > 1e-6) {
    const h = Math.min(t, 0.008)
    s.v += (-w * w * (s.x - target) - 2 * z * w * s.v) * h
    s.x += s.v * h
    t -= h
  }
}
const clamp = (v: number, a = -1, b = 1) => Math.max(a, Math.min(b, v))

let uid = 0

export function CactusMascot({ mood, className }: { mood: KikoMood; className?: string }) {
  const cfg = useMemo(() => {
    const c = defaultConfig()
    c.character = 'cactus'
    return c
  }, [])
  const frame = useMemo(() => restFrame(cfg), [cfg])
  const prefix = useMemo(() => `kiko${++uid}`, [])
  const host = useRef<SVGSVGElement>(null)
  const layer = useRef<SVGGElement>(null)
  const moodRef = useRef(mood)
  // Declared before the animation effect, so it reads this render's mood.
  useEffect(() => {
    moodRef.current = mood
  })

  useEffect(() => {
    const svg = host.current
    const g = layer.current
    if (!svg || !g) return
    // The card round Kiko (its button) is the hover target, not just the drawing.
    const zone: Element = svg.parentElement ?? svg
    const reduced = window.matchMedia?.('(prefers-reduced-motion: reduce)').matches ?? false
    const pose = zeroPose()
    const draw = () => {
      g.innerHTML = sceneMarkup(buildScene(cfg, pose), prefix)
    }
    if (reduced) {
      Object.assign(pose, MOODS[moodRef.current])
      draw()
      return
    }

    const pointer = { x: 0, y: 0, near: false, over: false }
    const sp = {
      gx: s0(),
      gy: s0(),
      headTurn: s0(),
      headTip: s0(),
      turn: s0(),
      lean: s0(),
      hover: s0(),
      sway: s0(),
      hop: s0(),
    }
    const sec = { jx: s0(), jy: s0(), armL: s0(), armR: s0() }
    const face = Object.fromEntries(FACE.map((c) => [c, s0()])) as Record<(typeof FACE)[number], S>
    let prev: { lat: number; lift: number } | null = null
    let hopAt = -1
    let happyUntil = 0
    let nextBlink = performance.now() + 1500
    let blinkAt = -1
    let last = performance.now()
    let raf = 0

    const onMove = (e: PointerEvent) => {
      if (e.pointerType === 'touch') return
      const r = svg.getBoundingClientRect()
      // From the face (upper part of the column), in units of the drawing's size.
      const fx = r.left + r.width / 2
      const fy = r.top + r.height * 0.42
      const dx = e.clientX - fx
      const dy = e.clientY - fy
      const d = Math.hypot(dx, dy)
      const R = 520
      // Attention ramps in near Kiko, holds within R, fades beyond it.
      const fade = d <= R ? 1 : Math.max(0, 1 - (d - R) / (R * 0.6))
      const ramp = Math.min(1, d / 60)
      pointer.near = fade > 0
      pointer.x = d ? (dx / d) * ramp * fade : 0
      pointer.y = d ? (dy / d) * ramp * fade : 0
    }
    const onEnter = () => {
      pointer.over = true
    }
    const onLeave = () => {
      pointer.over = false
    }
    const onDown = () => {
      hopAt = performance.now()
      happyUntil = hopAt + 1100
    }
    const onOut = () => {
      pointer.near = false
    }
    window.addEventListener('pointermove', onMove, { passive: true })
    document.addEventListener('pointerleave', onOut)
    zone.addEventListener('pointerenter', onEnter)
    zone.addEventListener('pointerleave', onLeave)
    zone.addEventListener('pointerdown', onDown)

    const loop = (now: number) => {
      const dt = Math.min(0.05, Math.max(0.001, (now - last) / 1000))
      last = now
      const t = now / 1000
      const m = moodRef.current
      for (const k in pose) pose[k as keyof Pose] = 0

      // Attention: eyes lead, the head follows, the body follows the head (overlapping action).
      const gx = pointer.near ? pointer.x : 0
      const gy = pointer.near ? pointer.y : 0
      spring(sp.gx, gx, 14, dt)
      spring(sp.gy, gy, 14, dt)
      spring(sp.headTurn, gx * 0.4, 8.5, dt)
      spring(sp.headTip, -gy * 0.3, 8.5, dt)
      spring(sp.turn, gx * 0.4 * 0.45, 5, dt)
      spring(sp.lean, gx * 2.4 * 0.45, 5.5, dt)

      // Hover: perks up, a bigger smile, leans toward the pointer on a soft spring.
      spring(sp.hover, pointer.over ? 1 : 0, 12, dt)
      spring(sp.sway, pointer.over ? Math.tanh(gx * 1.3) * 0.05 : gx * 0.02, 5.5, dt)
      const hv = sp.hover.x

      // The mood's face, plus hover and a click's delight, each eased on its own spring.
      const target: Partial<Record<(typeof FACE)[number], number>> = { ...MOODS[m] }
      target.g_wide = (target.g_wide ?? 0) + hv * 0.45
      target.mouthSmile = (target.mouthSmile ?? 0) + hv * 0.3
      if (now < happyUntil) {
        target.g_happy = 1
        target.mouthSmile = 0.7
        target.g_slant = 0
        target.mouthWave = 0
      }
      for (const c of FACE) spring(face[c], target[c] ?? 0, 9, dt)
      for (const c of FACE) pose[c] = face[c].x

      // Mood motion, slow and smooth: a raised hand waves, a worried Kiko sways from side to side.
      if (m === 'hand')
        pose.armR += Math.sin(t * Math.PI * 2 * 0.9) * 9 * Math.min(1, face.armR.x / 18)
      const worry = m === 'flare' ? Math.sin(t * Math.PI * 2 * 0.45) : 0

      // A click: an underdamped hop up and a squash on landing.
      if (hopAt >= 0) {
        const k = (now - hopAt) / 520
        if (k >= 1) hopAt = -1
        else spring(sp.hop, Math.sin(Math.min(1, k) * Math.PI) * 0.09, 30, dt)
      } else spring(sp.hop, 0, 18, dt)

      pose.gazeX = clamp(sp.gx.x * 0.9)
      pose.gazeY = clamp(pose.gazeY + sp.gy.x * 0.75)
      pose.headTurn = sp.headTurn.x - sp.turn.x * 0.5
      pose.headTip = sp.headTip.x
      pose.turn = sp.turn.x
      pose.lean = sp.lean.x + worry * 1.6
      pose.bend = sp.sway.x + worry * 0.012
      pose.lift = Math.max(0, sp.hop.x)
      pose.pupil += hv * 0.03

      // Breathing and a slow ambient drift.
      pose.breath = 0.5 + 0.5 * Math.sin((t / 3.6) * Math.PI * 2)
      pose.dBob = Math.sin((t / 2.6) * Math.PI * 2) * 0.4
      pose.dArm = Math.sin((t / 3.1) * Math.PI * 2) * 0.5
      pose.dTilt = Math.sin((t / 4.3) * Math.PI * 2) * 0.3
      pose.fxPhase = (t / 1.8) % 1

      // Follow-through: how fast the body moves drives springs that lag and settle once.
      const lat = pose.turn * 30 + pose.lean + pose.bend * 300
      const lift = pose.lift
      let vLat = 0
      let vLift = 0
      if (prev) {
        vLat = (lat - prev.lat) / dt
        vLift = (lift - prev.lift) / dt
        if (Math.abs(vLat) > 600 || Math.abs(vLift) > 8) vLat = vLift = 0
      }
      prev = { lat, lift }
      const c = (v: number, mx: number) => Math.max(-mx, Math.min(mx, v))
      wobble(sec.jx, c(-vLat * 0.0006, 0.06), 13, 0.32, dt)
      wobble(sec.jy, c(vLift * 0.18, 0.07), 15, 0.34, dt)
      wobble(sec.armL, c(-vLift * 22 + vLat * 0.04, 7), 11, 0.5, dt)
      wobble(sec.armR, c(-vLift * 22 - vLat * 0.04, 7), 11, 0.5, dt)
      pose.jx = sec.jx.x
      pose.jy = sec.jy.x
      pose.sArmL = sec.armL.x
      pose.sArmR = sec.armR.x

      // Blinks every few seconds, eased in and out.
      if (blinkAt < 0 && now > nextBlink) blinkAt = now
      if (blinkAt >= 0) {
        const k = (now - blinkAt) / 170
        pose.blink = k < 1 ? Math.sin(k * Math.PI) : 0
        if (k >= 1) {
          blinkAt = -1
          nextBlink = now + 2800 + Math.random() * 2800
        }
      }

      draw()
      raf = requestAnimationFrame(loop)
    }
    raf = requestAnimationFrame(loop)
    return () => {
      cancelAnimationFrame(raf)
      window.removeEventListener('pointermove', onMove)
      document.removeEventListener('pointerleave', onOut)
      zone.removeEventListener('pointerenter', onEnter)
      zone.removeEventListener('pointerleave', onLeave)
      zone.removeEventListener('pointerdown', onDown)
    }
  }, [cfg, prefix])

  // Room above the rest frame for a raised arm, a hop and the floating marks.
  const padX = frame.w * 0.08
  const top = frame.cy - frame.h / 2 - frame.h * 0.16
  const vb = `${(frame.cx - frame.w / 2 - padX).toFixed(1)} ${top.toFixed(1)} ${(frame.w + padX * 2).toFixed(1)} ${(frame.h * 1.2).toFixed(1)}`
  return (
    <svg ref={host} viewBox={vb} className={className} aria-hidden>
      <g ref={layer} />
    </svg>
  )
}
