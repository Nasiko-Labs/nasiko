/**
 * The page for an unknown URL (rendered outside the shell, so it gives a way back in). Its own module, loaded on
 * demand from `__root.tsx`: the root route isn't code-split, and every page would load it otherwise.
 *
 * Our take on BeUI's "404 / Not Found Glitch" block (beui.dev/components/blocks/not-found, MIT): the code scrambles
 * through glyphs on mount and settles left to right; hovering splits it into two tinted ghosts. Lab edits: theme
 * tokens for the ghosts (multiply in light mode, screen in dark), the code is decorative and the title is the h1,
 * and links back into the app instead of "Browse components".
 */
import { Link } from '@tanstack/react-router'
import { useReducedMotion } from 'motion/react'
import { useEffect, useState } from 'react'
import { Button } from '@/components/ui/button'
import { copy } from './copy'
import { NasikoMark } from './NasikoMark'

const CODE = '404'
const GLYPHS = 'ABCDEFGHJKLMNPQRSTUVWXYZ0123456789#%&@$?/\\'
const SCRAMBLE_MS = 700
const TICK_MS = 45

/** The first paint shows the real code, so the scramble is an enhancement; reduced motion never scrambles. */
function Scramble({ text }: { text: string }) {
  const reduce = useReducedMotion()
  const [display, setDisplay] = useState(text)
  useEffect(() => {
    if (reduce) return
    const start = performance.now()
    let raf = 0
    let last = 0
    const loop = (now: number) => {
      if (now - last >= TICK_MS) {
        last = now
        const settled = Math.floor(Math.min((now - start) / SCRAMBLE_MS, 1) * text.length)
        setDisplay(
          [...text]
            .map((ch, i) => (i < settled ? ch : GLYPHS[Math.floor(Math.random() * GLYPHS.length)]))
            .join(''),
        )
      }
      if (now - start < SCRAMBLE_MS) raf = requestAnimationFrame(loop)
      else setDisplay(text)
    }
    raf = requestAnimationFrame(loop)
    return () => cancelAnimationFrame(raf)
  }, [text, reduce])
  return <span className="tabular-nums">{display}</span>
}

const GHOST =
  'pointer-events-none absolute inset-0 opacity-0 mix-blend-multiply transition-[translate,opacity] duration-150 ease-out group-hover:opacity-70 motion-reduce:hidden dark:mix-blend-screen'

export function NotFound() {
  return (
    <main className="flex min-h-dvh flex-col items-center justify-center gap-8 px-4 py-16 text-center">
      <NasikoMark className="size-7 text-logo" />
      <div
        aria-hidden
        className="group relative font-mono [font-size:clamp(5rem,18vw,11rem)] leading-none font-bold tracking-tighter text-foreground select-none"
      >
        <span className={`${GHOST} text-destructive group-hover:translate-x-[3px]`}>
          <Scramble text={CODE} />
        </span>
        <span className={`${GHOST} text-info group-hover:-translate-x-[3px]`}>
          <Scramble text={CODE} />
        </span>
        <span className="relative">
          <Scramble text={CODE} />
        </span>
      </div>
      <div className="flex flex-col items-center gap-2">
        <h1 className="text-lg font-semibold">{copy.notFound.title}</h1>
        <p className="max-w-sm text-sm text-muted-foreground">{copy.notFound.body}</p>
      </div>
      <div className="flex flex-wrap justify-center gap-2">
        <Button asChild>
          <Link to="/">{copy.notFound.home}</Link>
        </Button>
        <Button asChild variant="outline">
          <Link to="/chat">{copy.notFound.toChat}</Link>
        </Button>
      </div>
    </main>
  )
}
