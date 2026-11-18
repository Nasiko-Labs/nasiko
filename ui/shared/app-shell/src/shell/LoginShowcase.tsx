import { useReducedMotion } from 'motion/react'
import { useEffect, useRef, useState } from 'react'
import { durations } from '@nasiko/ui/lib/motion'
import { copy } from '@nasiko/app-core/shell/copy'
import { NasikoLockup } from '@nasiko/ui/composites/NasikoMark'
import { startLoginField } from '@nasiko/app-shell/shell/loginField'

/**
 * The login page's showcase (nasiko-login-gradient, 2026-11-12): the left half, full height, on the brand's black
 * (#151515), with the fluted-glass field (`loginField.ts`) behind it and a shade rising from the foot so the white
 * headline reads. The lockup in white at the top, nasiko.com's hero at the foot: the lead, then each word of
 * `copy.login.showcaseWords` in turn every `wordHold`. Decorative only: aria-hidden, inert (the field reads the pointer
 * from window); under reduced motion the field is still and the first word stays. Hidden below `md`.
 */
const WORDS = copy.login.showcaseWords

export function LoginShowcase() {
  const reduce = useReducedMotion()
  const canvas = useRef<HTMLCanvasElement>(null)
  useEffect(
    () => (canvas.current ? startLoginField(canvas.current, !!reduce) : undefined),
    [reduce],
  )
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
      data-testid="login-showcase"
      className="pointer-events-none relative hidden min-h-[max(640px,100svh)] flex-col justify-between overflow-hidden bg-[#151515] p-[clamp(32px,4vw,72px)] text-white [--logo:#fff] md:flex"
    >
      <canvas ref={canvas} className="absolute inset-0 size-full" />
      <div className="absolute inset-x-0 bottom-0 h-1/2 bg-linear-to-t from-[#151515]/75 via-[#151515]/30 to-transparent" />
      <NasikoLockup className="relative h-[clamp(20px,1.7vw,30px)] w-auto self-start text-white" />
      <div className="relative flex flex-col gap-[clamp(16px,1.8vw,28px)]">
        <p className="text-[clamp(36px,3.6vw,68px)] leading-[1.1] font-medium tracking-[-0.025em]">
          {copy.login.showcaseLead}
          <br />
          {copy.login.showcaseFor}{' '}
          <span
            key={active}
            className="inline-block min-h-[1.12em] animate-word-in motion-reduce:animate-none"
          >
            {active}
          </span>
        </p>
        <p className="max-w-[42ch] text-[clamp(16px,1.35vw,24px)] leading-[1.35] text-white/90">
          {copy.login.showcaseLine}
        </p>
      </div>
    </div>
  )
}
