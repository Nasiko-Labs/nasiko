import { useReducedMotion } from 'motion/react'
import { useEffect, useRef, useState } from 'react'
import { durations } from '@/lib/motion'
import { cn } from '@/lib/utils'
import { copy } from './copy'
import { NasikoMark } from './NasikoMark'

/**
 * The login page's showcase: the user's "Nasiko Console" prototype, screen "01 Sign in" (docs/superpowers/specs/
 * 2026-10-01-login-onboarding-design.md §1), mirrored to the left half. Aceternity's "Login Form With Gradient"
 * colour field (our own take: blurred blobs cycling its four hues) rising from under a stack of
 * translucent glass plates (the Nasiko mark on the top one), a floor shadow and a masked dot grid, then tab chips and a glass headline card (light glass in light mode, dark glass in dark mode)
 * (half-black glass, the word in the brand's gold) whose word changes every 2.8 s; the lit chip and plate follow
 * it, ringed in the theme's primary. Decorative only: aria-hidden, inert, CSS motion, still under reduced motion; the
 * stack tilts a little with the pointer, as in the prototype. Colours are the per-mode `--showcase-*` tokens (index.css). Hidden below `md`.
 */
const WORDS = copy.login.showcaseWords
/** Bottom plate first, as the prototype stacks them: Frameworks, Tools, Coding Harnesses, Agents on top. */
const PLATES = [...WORDS].reverse()
const EDGES = [
  'var(--showcase-layer-1-edge)',
  'var(--showcase-layer-2-edge)',
  'var(--showcase-layer-3-edge)',
  'var(--showcase-layer-4-edge)',
]
const FILLS = [
  'var(--showcase-layer-1)',
  'var(--showcase-layer-2)',
  'var(--showcase-layer-3)',
  'var(--showcase-layer-4)',
]

/** The stack at rest, and how far the pointer tilts it (the prototype's ±8° / ±11° at the panel's edges). */
const TILT = { x: 56, z: -38, xRange: 16, zRange: 22 }
const tiltOf = (tx: number, ty: number) =>
  `rotateX(${TILT.x - ty * TILT.xRange}deg) rotateZ(${TILT.z + tx * TILT.zRange}deg)`

export function LoginShowcase() {
  const reduce = useReducedMotion()
  const panel = useRef<HTMLDivElement>(null)
  const stack = useRef<HTMLDivElement>(null)
  // The prototype's mouse tilt (user request 2026-10-01). The panel stays inert (pointer-events-none, aria-hidden): the
  // page's pointer position is read from window and written straight to the stack, one frame at a time, so nothing
  // re-renders. Outside the panel the stack eases back to rest; under reduced motion it never moves.
  useEffect(() => {
    if (reduce) return
    let frame = 0
    const onMove = (e: PointerEvent) => {
      cancelAnimationFrame(frame)
      frame = requestAnimationFrame(() => {
        const box = panel.current?.getBoundingClientRect()
        const el = stack.current
        if (!box || !el || !box.width) return
        const tx = (e.clientX - box.left) / box.width - 0.5
        const ty = (e.clientY - box.top) / box.height - 0.5
        const inside = Math.abs(tx) <= 0.5 && Math.abs(ty) <= 0.5
        el.style.transform = inside ? tiltOf(tx, ty) : tiltOf(0, 0)
      })
    }
    const onLeave = () => {
      if (stack.current) stack.current.style.transform = tiltOf(0, 0)
    }
    window.addEventListener('pointermove', onMove)
    document.documentElement.addEventListener('pointerleave', onLeave)
    return () => {
      cancelAnimationFrame(frame)
      window.removeEventListener('pointermove', onMove)
      document.documentElement.removeEventListener('pointerleave', onLeave)
    }
  }, [reduce])
  const [index, setIndex] = useState(0)
  useEffect(() => {
    if (reduce) return
    const t = setInterval(() => setIndex((i) => (i + 1) % WORDS.length), durations.wordHold)
    return () => clearInterval(t)
  }, [reduce])
  const active = WORDS[index] ?? WORDS[0]
  return (
    <div
      aria-hidden
      ref={panel}
      data-testid="login-showcase"
      className="pointer-events-none relative hidden min-h-[max(680px,calc(100svh-2rem))] flex-col p-10 md:flex"
    >
      {/* A soft light behind the stack, and a dot grid that fades out around it. */}
      <div className="absolute inset-0 bg-[radial-gradient(ellipse_60%_45%_at_50%_40%,var(--showcase-glow),transparent_70%)]" />
      <div className="absolute inset-0 bg-[radial-gradient(var(--showcase-dot)_1px,transparent_1.3px)] mask-[radial-gradient(ellipse_50%_45%_at_50%_40%,#000,transparent_75%)] bg-size-[22px_22px]" />
      <div className="relative flex min-h-100 flex-1 items-center justify-center pt-10 perspective-[1400px]">
        <div className="absolute top-[64%] left-1/2 -ml-50 h-27.5 w-100 rounded-[50%] bg-[radial-gradient(ellipse,var(--showcase-floor),transparent_70%)] blur-[10px]" />
        <div className="animate-float transform-3d motion-reduce:animate-none">
          <div
            ref={stack}
            className="relative size-72.5 transition-transform duration-500 ease-[cubic-bezier(.2,.8,.2,1)] transform-3d motion-reduce:transition-none"
            style={{ transform: tiltOf(0, 0) }}
          >
            {PLATES.map((label, i) => {
              const on = label === active
              return (
                <div
                  key={label}
                  className="absolute inset-0 flex items-center justify-center rounded-[48px] border transition-[transform,box-shadow,border-color] duration-700 ease-[cubic-bezier(.2,.8,.2,1)] motion-reduce:transition-none"
                  style={{
                    background: FILLS[i],
                    // The lit plate rises out of the stack; its edge and ring turn the brand's gold (the logo colour).
                    borderColor: on
                      ? 'color-mix(in oklab, var(--logo) 80%, transparent)'
                      : EDGES[i],
                    transform: `translateZ(${i * 54 + (on ? 26 : 0)}px)`,
                    boxShadow: `inset 0 1px 0 ${EDGES[i]}, 0 ${16 + i * 4}px 40px -18px var(--showcase-layer-shadow)${on ? ', 0 0 0 1.5px color-mix(in oklab, var(--logo) 60%, transparent)' : ''}`,
                  }}
                >
                  {i === PLATES.length - 1 ? (
                    <NasikoMark className="size-21 text-logo drop-shadow-[0_8px_14px_rgb(0_0_0/0.35)]" />
                  ) : null}
                  <span
                    className={cn(
                      'absolute bottom-4.5 left-6 text-[13px] font-semibold tracking-[.02em] whitespace-nowrap transition-colors duration-500',
                      on ? 'text-primary-text' : 'text-(--showcase-tag)',
                    )}
                  >
                    {label}
                  </span>
                </div>
              )
            })}
          </div>
        </div>
      </div>
      <div className="relative z-2 flex max-w-140 flex-col gap-3">
        <div className="flex flex-wrap gap-2">
          {WORDS.map((w) => (
            <span
              key={w}
              className={cn(
                'inline-flex h-7.5 items-center rounded-full border px-3.25 text-[13px] font-medium backdrop-blur-sm transition-colors duration-300',
                w === active
                  ? 'border-transparent bg-(--showcase-chip-on) text-(--showcase-chip-on-fg)'
                  : 'border-(--showcase-glass-border) bg-(--showcase-chip) text-(--showcase-chip-fg)',
              )}
            >
              {w}
            </span>
          ))}
        </div>
        <div className="rounded-[18px] border border-(--showcase-card-border) bg-(--showcase-card) px-6 py-5.5 text-(--showcase-card-foreground) shadow-[0_20px_40px_-24px_rgb(0_0_0/0.4)] backdrop-blur-[18px] backdrop-saturate-[1.2]">
          <p className="text-[clamp(30px,2.8vw,42px)] leading-[1.1] font-semibold tracking-[-0.03em]">
            {copy.login.showcaseLead}
            <br />
            <span
              key={active}
              className="inline-block min-h-[1.12em] animate-word-in text-(--showcase-accent) motion-reduce:animate-none"
            >
              {active}
            </span>
          </p>
          <p className="mt-3 text-[15px] leading-[1.55] text-(--showcase-card-muted)">
            {copy.login.showcaseLine}
          </p>
        </div>
      </div>
    </div>
  )
}

/**
 * The page's glow (user review 2026-10-01: one page, not two sections): Aceternity's colour field, blurred blobs
 * cycling the block's four hues, rising from the bottom-left behind both columns and fading into the page with no
 * edge. A page-level layer, so no column clips it; decorative, hidden below `md`, still under reduced motion.
 */
export function LoginGlow() {
  return (
    <div
      aria-hidden
      className="pointer-events-none absolute inset-0 -z-10 hidden overflow-hidden md:block"
    >
      <div className="absolute -bottom-1/4 -left-[8%] h-[85%] w-[68%] opacity-55 blur-[110px] dark:opacity-40">
        <Blob className="bottom-0 left-0 h-3/5 w-1/2 bg-showcase-glow-1" />
        <Blob className="bottom-[5%] left-[35%] h-[55%] w-[45%] bg-showcase-glow-2 [animation-delay:-3s] [animation-duration:12s]" />
        <Blob className="bottom-[30%] left-[10%] h-1/2 w-[45%] bg-showcase-glow-3 [animation-delay:-6s] [animation-duration:14s]" />
        <Blob className="bottom-[10%] left-[55%] h-1/2 w-2/5 bg-showcase-glow-4 [animation-delay:-9s] [animation-duration:11s]" />
      </div>
    </div>
  )
}

function Blob({ className }: { className: string }) {
  return (
    <div
      className={cn(
        'absolute animate-glow rounded-full opacity-75 motion-reduce:animate-none',
        className,
      )}
    />
  )
}
