/**
 * Aceternity UI "Glowing Effect" (registry item `@aceternity/glowing-effect`, MIT): a band of colour on the border that
 * turns to face a point, brightest at that point and fading out `spread` degrees either side.
 * Local edits:
 * - faces a given `angle` (0 = up, clockwise) instead of following the pointer: no pointer or scroll listeners, and no
 *   `proximity`/`inactiveZone`/`disabled`. It turns (Aceternity's ease and shortest way round) only when `angle` changes.
 * - theme tokens, not hex: the four colours are `--chart-1`..`--chart-4` (each theme's own order); no `white` variant.
 * - the colours are laid over the parent, not the viewport (no `background-attachment: fixed`), so every edge shows
 *   all four whatever its place on screen; the band is solid across 90% of its angle and fades only at its ends.
 * - always lit (`--active` 1), so `glow` is only Aceternity's hairline border round the whole parent.
 * - `animate={false}` jumps to the angle (reduced motion); a turn takes `movementDuration` seconds (1 s, was 2 s).
 * - decorative: `aria-hidden`, `pointer-events-none`; no `memo` (the React Compiler memoises).
 */
import { animate as tween } from 'motion/react'
import { useEffect, useRef, type CSSProperties } from 'react'
import { cn } from '@nasiko/ui/lib/utils'

const GRADIENT = `radial-gradient(circle, var(--chart-1) 10%, transparent 20%),
  radial-gradient(circle at 40% 40%, var(--chart-2) 5%, transparent 15%),
  radial-gradient(circle at 60% 60%, var(--chart-3) 10%, transparent 20%),
  radial-gradient(circle at 40% 60%, var(--chart-4) 10%, transparent 20%),
  repeating-conic-gradient(from 236.84deg at 50% 50%,
    var(--chart-1) 0%,
    var(--chart-2) calc(25% / 5),
    var(--chart-3) calc(50% / 5),
    var(--chart-4) calc(75% / 5),
    var(--chart-1) calc(100% / 5))`

export function GlowingEffect({
  angle,
  spread = 20,
  blur = 0,
  borderWidth = 1,
  glow = false,
  animate = true,
  movementDuration = 1,
  className,
}: {
  /** Where the band is brightest: degrees clockwise from the top. */
  angle: number
  /** How far the band reaches either side of `angle`, degrees. */
  spread?: number
  blur?: number
  borderWidth?: number
  glow?: boolean
  animate?: boolean
  movementDuration?: number
  className?: string
}) {
  const ref = useRef<HTMLSpanElement>(null)
  useEffect(() => {
    const el = ref.current
    if (!el) return
    const current = parseFloat(el.style.getPropertyValue('--start')) || 0
    // The shortest way round, as Aceternity's handleMove.
    const target = current + ((((angle - current + 180) % 360) + 360) % 360) - 180
    if (!animate) {
      el.style.setProperty('--start', String(target))
      return
    }
    const run = tween(current, target, {
      duration: movementDuration,
      ease: [0.16, 1, 0.3, 1],
      onUpdate: (v) => el.style.setProperty('--start', String(v)),
    })
    return () => run.stop()
  }, [angle, animate, movementDuration])

  return (
    <>
      {glow ? (
        <span
          aria-hidden
          className="pointer-events-none absolute -inset-px rounded-[inherit] border"
        />
      ) : null}
      <span
        ref={ref}
        aria-hidden
        style={
          {
            '--blur': `${blur}px`,
            '--spread': spread,
            '--start': String(angle),
            '--glowingeffect-border-width': `${borderWidth}px`,
            '--gradient': GRADIENT,
          } as CSSProperties
        }
        className={cn(
          'pointer-events-none absolute inset-0 rounded-[inherit]',
          blur > 0 && 'blur-(--blur)',
          className,
        )}
      >
        <span
          className={cn(
            'absolute inset-0 rounded-[inherit]',
            "after:absolute after:inset-[calc(-1*var(--glowingeffect-border-width))] after:rounded-[inherit] after:content-['']",
            'after:[border:var(--glowingeffect-border-width)_solid_transparent]',
            'after:[background:var(--gradient)]',
            'after:[mask-composite:intersect] after:[mask-clip:padding-box,border-box]',
            'after:[mask-image:linear-gradient(#0000,#0000),conic-gradient(from_calc((var(--start)-var(--spread))*1deg),#00000000_0deg,#fff_calc(var(--spread)*0.1deg),#fff_calc(var(--spread)*1.9deg),#00000000_calc(var(--spread)*2deg))]',
          )}
        />
      </span>
    </>
  )
}
