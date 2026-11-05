/**
 * "Did you know?" (2026-11-05): one platform capability at a time, in the lead card where the token-optimization band
 * would be, whenever that band has nothing to say (user 2026-11-06), and never with a "Tip 1 of 7" counter, which says
 * nothing. Each tip carries its feature's icon and a link to it; dots show where you are. Tips teach a feature; setup
 * steps in Needs your attention ask for one, so the two never repeat each other's words.
 *
 * Every tip sits in the same grid cell, so the cell is as tall as the longest and a change never moves the page; the
 * shown one fades in (CSS). It turns every TIP_ROTATE_MS, paused while the pointer or focus is inside and never under
 * reduced motion, where Next is the only way on. Not a live region: a rotating announcement would be noise.
 */
import { Link } from '@tanstack/react-router'
import {
  ArrowRight,
  ChevronRight,
  Gauge,
  Lightbulb,
  ListTree,
  Network,
  Plug,
  Route as RouteIcon,
  Waypoints,
  Workflow,
  type LucideIcon,
} from 'lucide-react'
import { useReducedMotion } from 'motion/react'
import { useEffect, useState } from 'react'
import { Button } from '@nasiko/ui/primitives/button'
import { cn } from '@nasiko/ui/lib/utils'
import { copy } from '../copy'
import { DAY, TIP_ROTATE_MS } from '../tuning'
import { TOUCH } from './Card'

const tips = copy.tips.items
/** Each tip's feature, drawn as the nav draws it. */
const ICON: Record<(typeof tips)[number]['to'], LucideIcon> = {
  '/chat': RouteIcon,
  '/workflows': Workflow,
  '/sessions': ListTree,
  '/flows': Network,
  '/router': Waypoints,
  '/mcp': Plug,
  '/optimization': Gauge,
}

export function DidYouKnow({ now, className }: { now: number; className?: string }) {
  // A different first tip each day, the same one all day.
  const [index, setIndex] = useState(() => Math.floor(now / DAY) % tips.length)
  const [paused, setPaused] = useState(false)
  const reduced = useReducedMotion()
  useEffect(() => {
    if (paused || reduced) return
    const t = setInterval(() => setIndex((i) => (i + 1) % tips.length), TIP_ROTATE_MS)
    return () => clearInterval(t)
  }, [paused, reduced])
  return (
    <section
      aria-labelledby="overview-tip-title"
      data-testid="overview-tip"
      className={cn('flex flex-col gap-3', className)}
      onPointerEnter={() => setPaused(true)}
      onPointerLeave={() => setPaused(false)}
      onFocus={() => setPaused(true)}
      onBlur={(e) => {
        if (!e.currentTarget.contains(e.relatedTarget as Node | null)) setPaused(false)
      }}
    >
      <div className="flex items-center justify-between gap-2">
        <h3
          id="overview-tip-title"
          className="flex items-center gap-1.5 text-xs font-medium text-muted-foreground"
        >
          <Lightbulb aria-hidden className="size-3.5 text-warning" />
          {copy.tips.title}
        </h3>
        <div className="flex items-center gap-2">
          {/* Where you are, without a counter: the shown tip's dot is the long one. */}
          <span aria-hidden className="flex items-center gap-1">
            {tips.map((t, i) => (
              <span
                key={t.to}
                className={cn(
                  'h-1.5 rounded-full transition-[width,background-color] duration-300 motion-reduce:transition-none',
                  i === index ? 'w-4 bg-foreground/60' : 'w-1.5 bg-muted-foreground/25',
                )}
              />
            ))}
          </span>
          <Button
            variant="ghost"
            size="icon-sm"
            className={TOUCH}
            aria-label={copy.tips.next}
            onClick={() => setIndex((i) => (i + 1) % tips.length)}
          >
            <ChevronRight aria-hidden />
          </Button>
        </div>
      </div>
      <div className="grid">
        {tips.map((t, i) => {
          const Icon = ICON[t.to]
          return (
            <div
              key={t.to}
              inert={i !== index}
              aria-hidden={i !== index}
              className={cn(
                'flex items-start gap-3 transition-opacity duration-300 [grid-area:1/1] motion-reduce:transition-none',
                i === index ? 'opacity-100' : 'opacity-0',
              )}
            >
              <span
                aria-hidden
                className="flex size-9 shrink-0 items-center justify-center rounded-lg bg-primary/10 text-primary-text"
              >
                <Icon className="size-4.5" />
              </span>
              <div className="flex min-w-0 flex-col items-start gap-1.5">
                <p className="text-[0.9375rem] leading-6 text-pretty">{t.text}</p>
                <Link
                  to={t.to}
                  search={(t.to === '/chat' ? { auto: 1 } : {}) as never}
                  className={cn(
                    'inline-flex items-center gap-1 rounded-sm text-sm font-medium text-primary-text underline-offset-4 hover:underline',
                    TOUCH,
                  )}
                >
                  {t.link} <ArrowRight aria-hidden className="size-3.5" />
                </Link>
              </div>
            </div>
          )
        })}
      </div>
    </section>
  )
}
