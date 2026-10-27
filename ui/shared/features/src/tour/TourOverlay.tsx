/**
 * The open tour (lazy, `index.ts`): a spotlight that glides from target to target over the dimmed, blurred page, and the step's
 * card beside it. The card is a modal Base UI Popover, so focus stays in it, the page under it is inert and hidden from
 * screen readers, and Esc ends the tour; ←/→ step. The card goes on the step's side of the target, else the first side
 * with room for it; a target that leaves room on no side (a long list) docks the card at the bottom of the screen.
 *
 * Effects (CLAUDE.md, Aceternity effects): the spotlight's edge is Aceternity's Glowing Effect (a band of theme
 * colours lighting the side that faces the card, still between steps) and each title is Aceternity's Text Generate Effect. Both are decorative and
 * stand still under reduced motion, where the spotlight also jumps instead of gliding.
 */
import {
  animate,
  m,
  useMotionValue,
  useReducedMotion,
  useTransform,
  type MotionValue,
  type Transition,
} from 'motion/react'
import { use, useEffect, useId, useRef, useState, type KeyboardEvent } from 'react'
import { createPortal } from 'react-dom'
import { GlowingEffect } from '@nasiko/ui/composites/glowing-effect'
import { TextGenerateEffect } from '@nasiko/ui/composites/text-generate-effect'
import { Button } from '@nasiko/ui/primitives/button'
import { Popover, PopoverClose, PopoverContent } from '@nasiko/ui/primitives/popover'
import { transitions } from '@nasiko/ui/lib/motion'
import { cn } from '@nasiko/ui/lib/utils'
import { copy } from './copy'
import type { LayerTour } from '@nasiko/app-core/edition/edition'
import type { TourStep } from '@nasiko/app-core/tour'
import type { TourId } from './logic'
import { TOURS } from './tours'

/** Room between the target and the spotlight's edge. */
const PAD = 6

interface Box {
  x: number
  y: number
  width: number
  height: number
}

/** The first element for a target that takes up room on screen (not `display: none`, not a hidden tab). */
function findTarget(target: string): HTMLElement | null {
  for (const el of document.querySelectorAll<HTMLElement>(`[data-tour~="${target}"]`)) {
    const r = el.getBoundingClientRect()
    if (r.width > 0 && r.height > 0) return el
  }
  return null
}

/** A centred step's spotlight: a point a third of the way down, so the card below it sits near the middle. */
const centre = (): Box => ({ x: innerWidth / 2, y: innerHeight / 3, width: 0, height: 0 })

/** Keeps the spotlight inside the screen: a target taller or wider than it is lit only where it shows. */
const EDGE = 4
function clampToScreen(b: Box): Box {
  const x = Math.max(b.x, EDGE)
  const y = Math.max(b.y, EDGE)
  return {
    x,
    y,
    width: Math.max(0, Math.min(b.x + b.width, innerWidth - EDGE) - x),
    height: Math.max(0, Math.min(b.y + b.height, innerHeight - EDGE) - y),
  }
}

type Side = NonNullable<TourStep['side']>
const OPPOSITE: Record<Side, Side> = { top: 'bottom', bottom: 'top', left: 'right', right: 'left' }
/** What the card needs beside a target: its size (320 × up to ~240) plus the gap and the screen margin. */
const ROOM = { across: 270, beside: 350 }

/** The step's side if the card fits there, else its opposite, else the other axis; null when no side has room. */
function sideFor(spot: Box, wanted: Side): Side | null {
  const room: Record<Side, number> = {
    top: spot.y,
    bottom: innerHeight - spot.y - spot.height,
    left: spot.x,
    right: innerWidth - spot.x - spot.width,
  }
  const across = wanted === 'top' || wanted === 'bottom'
  const others: Side[] = across ? ['right', 'left'] : ['bottom', 'top']
  return (
    [wanted, OPPOSITE[wanted], ...others].find(
      (side) => room[side] >= (side === 'top' || side === 'bottom' ? ROOM.across : ROOM.beside),
    ) ?? null
  )
}

/** Under reduced motion the spotlight jumps. One object, so the glide effect doesn't restart on every render. */
const JUMP: Transition = { duration: 0 }

/** Radius of the spotlight's hole, as its `rounded-lg`. */
const RADIUS = 8

/**
 * The dim's clip: the whole screen minus a rounded hole over the spotlight (even-odd, so the inner path cuts).
 * The outer box is the viewport, not a big constant: once it passes Chrome's texture size in device pixels (9999 px on
 * a 2× screen), Chrome blurs the hole too.
 */
function holePath([x, y, w, h, vw, vh]: number[]): string {
  const r = Math.max(0, Math.min(RADIUS, w / 2, h / 2))
  return (
    `path(evenodd, "M0 0H${vw}V${vh}H0Z M${x + r} ${y}H${x + w - r}A${r} ${r} 0 0 1 ${x + w} ${y + r}` +
    `V${y + h - r}A${r} ${r} 0 0 1 ${x + w - r} ${y + h}H${x + r}A${r} ${r} 0 0 1 ${x} ${y + h - r}` +
    `V${y + r}A${r} ${r} 0 0 1 ${x + r} ${y}Z")`
  )
}

/**
 * The spotlight's box as motion values, gliding to each new box: the edge and the dim's hole read the same values.
 * A new `grow` key (a step entered from a centred one) first closes the box to the middle of its target, so the hole
 * opens there instead of flying in from wherever the last one closed.
 */
function useGlidingBox(
  box: Box,
  transition: Transition,
  grow: number | null,
): Record<keyof Box, MotionValue<number>> {
  const x = useMotionValue(box.x)
  const y = useMotionValue(box.y)
  const width = useMotionValue(box.width)
  const height = useMotionValue(box.height)
  const grown = useRef<number | null>(null)
  useEffect(() => {
    if (grow !== null && grown.current !== grow) {
      grown.current = grow
      x.set(box.x + box.width / 2)
      y.set(box.y + box.height / 2)
      width.set(0)
      height.set(0)
    }
    const runs = [
      animate(x, box.x, transition),
      animate(y, box.y, transition),
      animate(width, box.width, transition),
      animate(height, box.height, transition),
    ]
    return () => runs.forEach((run) => run.stop())
  }, [x, y, width, height, box.x, box.y, box.width, box.height, transition, grow])
  return { x, y, width, height }
}

/** The viewport's size as motion values, for the dim's clip. */
function useViewport(): [MotionValue<number>, MotionValue<number>] {
  const width = useMotionValue(innerWidth)
  const height = useMotionValue(innerHeight)
  useEffect(() => {
    const onResize = () => {
      width.set(innerWidth)
      height.set(innerHeight)
    }
    addEventListener('resize', onResize)
    return () => removeEventListener('resize', onResize)
  }, [width, height])
  return [width, height]
}

/** The edge's glow faces the card: degrees clockwise from the top, per side (a docked card sits below). */
const FACING: Record<Side, number> = { top: 0, right: 90, bottom: 180, left: 270 }

/** How far the glow spreads: to the corners of the side facing the card, so that whole side lights, brightest mid-way. */
function spreadFor(spot: Box, side: Side): number {
  const across = side === 'top' || side === 'bottom'
  const toCorner = Math.atan2(across ? spot.width : spot.height, across ? spot.height : spot.width)
  return Math.min(88, Math.max(30, (toCorner * 180) / Math.PI))
}

/** A spotlight closed to a point at the middle of `b`: a centred step shrinks the hole in place. */
const collapse = (b: Box): Box => ({
  x: b.x + b.width / 2,
  y: b.y + b.height / 2,
  width: 0,
  height: 0,
})

/** Where a card with nowhere beside its target goes: the bottom middle of the screen, over the target. */
const dock = (): Box => ({ x: innerWidth / 2, y: innerHeight - 16, width: 0, height: 0 })

/**
 * The target's box, followed while its step is open: the page may scroll, reflow or animate under it. Brings the
 * target into view first. `box` is null when the step has no target or it is gone (the step shows centred); until a new
 * target is read (a frame) it is the last one, so a step change never shows a centred frame. `last` is the last box
 * read, which a centred step closes the spotlight on. `fresh` once this step's own target has been read.
 * ponytail: one rect read per frame while a tour is open; ResizeObserver + scroll listeners if that ever shows up.
 */
function useTargetBox(
  target: string | undefined,
  reduced: boolean,
): { box: Box | null; last: Box | null; fresh: boolean } {
  const [seen, setSeen] = useState<{ target: string; box: Box | null } | null>(null)
  useEffect(() => {
    if (!target) return
    const el = findTarget(target)
    const r = el?.getBoundingClientRect()
    if (el && r && (r.top < 0 || r.bottom > innerHeight)) {
      el.scrollIntoView({
        // A target taller than most of the screen shows its top, the rest its middle.
        block: r.height > innerHeight * 0.6 ? 'start' : 'center',
        behavior: reduced ? 'auto' : 'smooth',
      })
    }
    let last = ''
    let frame = requestAnimationFrame(function tick() {
      const box = findTarget(target)?.getBoundingClientRect()
      const key = box ? `${box.x} ${box.y} ${box.width} ${box.height}` : ''
      if (key !== last) {
        last = key
        setSeen({
          target,
          box: box
            ? {
                x: box.x - PAD,
                y: box.y - PAD,
                width: box.width + 2 * PAD,
                height: box.height + 2 * PAD,
              }
            : null,
        })
      }
      frame = requestAnimationFrame(tick)
    })
    return () => cancelAnimationFrame(frame)
  }, [target, reduced])
  const last = seen?.box ?? null
  return { box: target ? last : null, last, fresh: !!target && seen?.target === target }
}

/**
 * A layer's steps, loaded once per tour and kept: `use()` needs the same promise on every render. A failed load is
 * dropped, so the next try loads again, and reads as no steps (the tour ends at once).
 */
const layerSteps = new Map<string, Promise<readonly TourStep[]>>()
function stepsOf(layer: LayerTour): Promise<readonly TourStep[]> {
  let p = layerSteps.get(layer.id)
  if (!p) {
    p = layer.steps().catch(() => {
      layerSteps.delete(layer.id)
      return []
    })
    layerSteps.set(layer.id, p)
  }
  return p
}

export function TourOverlay({
  id,
  layer,
  offerOptOut,
  onEnd,
  onOptOut,
}: {
  /** A core tour (`TourId`), or the layer's when `layer` is set. */
  id: string
  /** A layer's own page (the `tours` slot): its steps load first (the overlay waits in Suspense meanwhile). */
  layer: LayerTour | null
  /** A first-time user's first tour: its first card offers No thanks, which turns automatic tours off. */
  offerOptOut: boolean
  onEnd(): void
  onOptOut(): void
}) {
  // The steps this page can show now (tours.ts), read once when the tour opens: a later re-render never drops one.
  const page = layer ? use(stepsOf(layer)) : (TOURS[id as TourId] ?? [])
  const [steps] = useState(() => page.filter((s) => !s.target || findTarget(s.target)))
  // Nothing to show (a layer's steps failed to load): end at once rather than hold the page under an empty tour.
  const empty = steps.length === 0
  useEffect(() => {
    if (empty) onEnd()
  }, [empty, onEnd])
  // `from` is the step shown before this one, so a move between a centred step and a lit one can be told apart.
  const [{ index, from }, setPos] = useState({ index: 0, from: 0 })
  const step = steps[index]
  const reduced = useReducedMotion() ?? false
  const { box: target, last: lastTarget, fresh } = useTargetBox(step?.target, reduced)
  // A centred step closes the spotlight where it was, rather than flying the hole to the middle of the screen.
  const spot = target
    ? clampToScreen(target)
    : lastTarget
      ? collapse(clampToScreen(lastTarget))
      : centre()
  const side = target ? sideFor(spot, step?.side ?? 'bottom') : 'bottom'
  // The edge keeps facing the last card while it fades out on a centred step.
  const lit = target ?? lastTarget
  const litSpot = lit ? clampToScreen(lit) : spot
  const litSide =
    (target ? side : lit && sideFor(litSpot, steps[from]?.side ?? 'bottom')) ?? 'bottom'
  // Into or out of a centred step the card doesn't fly across the screen: it jumps, and appears where it lands.
  const swap = from !== index && !steps[from]?.target !== !step?.target
  // The card hangs off the spotlight, or off the dock when there is no room beside it; a centred card off the middle,
  // where a card leaving a centred step also waits until its target is read (a frame), so it never shows at the old one.
  const anchor = !target || (swap && !fresh) ? centre() : side ? spot : dock()
  const glide = reduced ? JUMP : transitions.spotlight
  const card = useRef<HTMLDivElement>(null)
  // The card hangs off this gliding box: Base UI's Positioner `anchor`, tracked as it moves.
  const anchorBox = useRef<HTMLDivElement>(null)
  useEffect(() => {
    const el = card.current
    if (!swap || reduced || !el) return
    // Hidden at once, shown once the popover has moved to its new anchor (a few frames behind the state).
    el.style.opacity = '0'
    const run = animate(
      el,
      { opacity: [0, 1], scale: [0.96, 1] },
      { duration: 0.28, delay: 0.08, ease: [0.16, 1, 0.3, 1] },
    )
    return () => run.stop()
  }, [index, swap, reduced])
  const box = useGlidingBox(spot, glide, swap && fresh ? index : null)
  const [vw, vh] = useViewport()
  const hole = useTransform([box.x, box.y, box.width, box.height, vw, vh], holePath)
  const titleId = useId()
  const bodyId = useId()
  const primary = useRef<HTMLButtonElement>(null)
  if (!step) return null

  const last = index === steps.length - 1
  const optOut = offerOptOut && index === 0
  const next = () => (last ? onEnd() : setPos({ index: index + 1, from: index }))
  const back = () => setPos({ index: Math.max(0, index - 1), from: index })
  const onKeyDown = (e: KeyboardEvent) => {
    if (e.key === 'ArrowRight') next()
    else if (e.key === 'ArrowLeft') back()
    else return
    e.preventDefault()
  }

  return (
    <Popover
      open
      modal
      onOpenChange={(open, details) => {
        if (open) return
        // A click on the dimmed page doesn't end the tour; Skip and Esc do.
        if (details.reason === 'outside-press') {
          details.cancel()
          return
        }
        onEnd()
      }}
    >
      {createPortal(
        <>
          {/* Dims and blurs the page everywhere but the spotlight, so only the step's target stays sharp. */}
          <m.div
            aria-hidden
            className="pointer-events-none fixed inset-0 z-50 bg-black/40 backdrop-blur-[3px]"
            style={{ clipPath: hole }}
          />
          <m.div
            aria-hidden
            data-testid="tour-spotlight"
            className="pointer-events-none fixed top-0 left-0 z-50 rounded-lg"
            style={box}
            initial={false}
            animate={{ opacity: target && (fresh || !swap) ? 1 : 0 }}
            transition={reduced ? JUMP : transitions.fade}
          >
            {lit ? (
              <>
                {/* Aceternity's demo pairing: a sharp band on the edge over a blurred copy of it. */}
                <GlowingEffect
                  angle={FACING[litSide]}
                  spread={spreadFor(litSpot, litSide)}
                  blur={12}
                  borderWidth={6}
                  className="opacity-80"
                  animate={!reduced}
                />
                <GlowingEffect
                  glow
                  angle={FACING[litSide]}
                  spread={spreadFor(litSpot, litSide)}
                  borderWidth={2}
                  animate={!reduced}
                />
              </>
            ) : null}
          </m.div>
          <m.div
            ref={anchorBox}
            aria-hidden
            className="pointer-events-none fixed top-0 left-0"
            initial={false}
            animate={{ x: anchor.x, y: anchor.y, width: anchor.width, height: anchor.height }}
            transition={swap ? JUMP : glide}
          />
        </>,
        document.body,
      )}
      <PopoverContent
        ref={card}
        side={side ?? 'top'}
        sideOffset={target && side ? 12 : 0}
        collisionPadding={16}
        anchor={anchorBox}
        // Fixed, as Radix's popper was: placed absolutely, the card's interim positions past the viewport's foot grow
        // the document mid-glide and throw it off the gliding anchor (up to ~200 px for a few frames).
        positionMethod="fixed"
        aria-labelledby={titleId}
        aria-describedby={bodyId}
        initialFocus={primary}
        onKeyDown={onKeyDown}
        className="flex w-80 max-w-[calc(100vw-2rem)] flex-col gap-0 overflow-hidden p-0"
      >
        {/* Stays mounted across steps, so a screen reader hears each new step while focus stays on Next. */}
        <div aria-live="polite" className="flex flex-col gap-2 p-4">
          <div className="flex items-center justify-between gap-2">
            <p className="text-xs text-muted-foreground tabular-nums">
              {copy.step(index + 1, steps.length)}
            </p>
            <Dots count={steps.length} index={index} />
          </div>
          {/* Keyed by step: the title's words arrive again on every step. */}
          <div key={index} className="flex flex-col gap-1.5">
            <h2 id={titleId} className="text-base font-semibold text-balance">
              <TextGenerateEffect words={step.title} />
            </h2>
            <p id={bodyId} className="text-sm text-pretty text-muted-foreground">
              {step.body}
            </p>
            {optOut ? <p className="text-xs text-muted-foreground">{copy.noThanksHint}</p> : null}
          </div>
        </div>
        <div className="flex items-center gap-2 border-t border-border bg-muted/40 px-4 py-3">
          {optOut ? (
            <Button variant="ghost" size="sm" className="-ml-2" onClick={onOptOut}>
              {copy.noThanks}
            </Button>
          ) : last ? null : (
            <Button variant="ghost" size="sm" className="-ml-2" onClick={onEnd}>
              {copy.skip}
            </Button>
          )}
          <div className="ml-auto flex gap-2">
            {index > 0 ? (
              <Button variant="outline" size="sm" onClick={back}>
                {copy.back}
              </Button>
            ) : null}
            <Button ref={primary} size="sm" onClick={next}>
              {optOut ? copy.start : last ? copy.done : copy.next}
            </Button>
          </div>
        </div>
        {/* A modal Base UI popover traps focus only while it holds a Close; Skip and Esc are the visible ways out. */}
        <PopoverClose className="sr-only">{copy.close}</PopoverClose>
      </PopoverContent>
    </Popover>
  )
}

/** Where the tour is, as a row of dots; the current one stretches. The "Step n of m" line says it in words. */
function Dots({ count, index }: { count: number; index: number }) {
  return (
    <span aria-hidden className="flex items-center gap-1">
      {Array.from({ length: count }, (_, n) => (
        <m.span
          key={n}
          className={cn('h-1.5 rounded-full', n === index ? 'bg-primary' : 'bg-border')}
          initial={false}
          animate={{ width: n === index ? 16 : 6 }}
          transition={transitions.fade}
        />
      ))}
    </span>
  )
}
