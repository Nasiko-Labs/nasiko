/**
 * Previous/Next for a detail view (the trace span panel's pattern): ↑ ↓ buttons, "3 of 18", and J / K
 * (`lib/keys.ts` `stepFromKey`). At either end, or while `blocked`, the buttons stay focusable (aria-disabled),
 * so a press there doesn't drop focus. `blocked` says why stepping is off (unsaved edits) in a tooltip.
 */
import { ChevronDown, ChevronUp } from 'lucide-react'
import { useEffect, useId, useRef } from 'react'
import { Button } from '@nasiko/ui/primitives/button'
import { Tooltip, TooltipContent, TooltipTrigger } from '@nasiko/ui/primitives/tooltip'
import { stepFromKey } from '@nasiko/ui/lib/keys'
import { cn } from '@nasiko/ui/lib/utils'

/**
 * ponytail: a module-level flag, set in a click and consumed by the next PrevNext to mount. A detail route remounts
 * on a step, which drops focus; the new instance puts it back on the arrow used, but only if focus fell to <body>,
 * within 10 s, and the clicked button is gone (the view really remounted: a sheet that stays open keeps its button).
 * A step blocked by a leave prompt just expires.
 */
const refocus: { which?: 'prev' | 'next'; el?: HTMLElement; at: number } = { at: 0 }

export type StepVia = 'click' | 'key'

export function PrevNext({
  noun,
  index,
  count,
  onPrev,
  onNext,
  blocked,
  restoreFocus = true,
  className,
}: {
  /** What is stepped through, for the labels ("Previous agent"). */
  noun: string
  /** 0-based position in the list. */
  index: number
  count: number
  /** `via` says what stepped: a click on an arrow or the J/K key. */
  onPrev: (via: StepVia) => void
  onNext: (via: StepVia) => void
  blocked?: string
  /** Put focus back on the arrow used after the view remounts. Off where the page focuses something better. */
  restoreFocus?: boolean
  className?: string
}) {
  const root = useRef<HTMLDivElement>(null)
  const prevButton = useRef<HTMLButtonElement>(null)
  const nextButton = useRef<HTMLButtonElement>(null)
  const reasonId = useId()
  const canPrev = !blocked && index > 0
  const canNext = !blocked && index < count - 1
  useEffect(() => {
    const { which, el, at } = refocus
    refocus.which = undefined
    refocus.el = undefined
    const fell = !document.activeElement || document.activeElement === document.body
    if (restoreFocus && which && fell && !el?.isConnected && Date.now() - at < 10_000)
      (which === 'prev' ? prevButton : nextButton).current?.focus()
  }, [restoreFocus])
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      const delta = stepFromKey(e, root.current)
      if (!delta) return
      // Claimed even at an end or while blocked, so a second control can't step (J/K has no default action outside fields).
      e.preventDefault()
      if (delta < 0 && canPrev) onPrev('key')
      if (delta > 0 && canNext) onNext('key')
    }
    window.addEventListener('keydown', onKey)
    return () => window.removeEventListener('keydown', onKey)
  }, [canPrev, canNext, onPrev, onNext])
  const button = 'aria-disabled:opacity-40 pointer-coarse:size-11'
  return (
    <div
      ref={root}
      role="group"
      aria-label={`${noun} ${index + 1} of ${count}`}
      className={cn('flex items-center gap-0.5', className)}
    >
      <span
        aria-hidden
        className="px-1 text-xs text-muted-foreground tabular-nums"
      >{`${index + 1} of ${count}`}</span>
      {blocked ? (
        <span id={reasonId} className="sr-only">
          {blocked}
        </span>
      ) : null}
      <Tooltip>
        <TooltipTrigger
          render={
            <Button
              ref={prevButton}
              variant="ghost"
              size="icon-sm"
              className={button}
              aria-label={`Previous ${noun}`}
              aria-keyshortcuts="k"
              aria-disabled={!canPrev}
              {...(blocked ? { 'aria-describedby': reasonId } : {})}
              onClick={(e) => {
                if (!canPrev) return
                Object.assign(refocus, { which: 'prev', el: e.currentTarget, at: Date.now() })
                onPrev('click')
              }}
            />
          }
        >
          <ChevronUp className="size-4" aria-hidden />
        </TooltipTrigger>
        <TooltipContent>{blocked ?? `Previous ${noun} (K)`}</TooltipContent>
      </Tooltip>
      <Tooltip>
        <TooltipTrigger
          render={
            <Button
              ref={nextButton}
              variant="ghost"
              size="icon-sm"
              className={button}
              aria-label={`Next ${noun}`}
              aria-keyshortcuts="j"
              aria-disabled={!canNext}
              {...(blocked ? { 'aria-describedby': reasonId } : {})}
              onClick={(e) => {
                if (!canNext) return
                Object.assign(refocus, { which: 'next', el: e.currentTarget, at: Date.now() })
                onNext('click')
              }}
            />
          }
        >
          <ChevronDown className="size-4" aria-hidden />
        </TooltipTrigger>
        <TooltipContent>{blocked ?? `Next ${noun} (J)`}</TooltipContent>
      </Tooltip>
    </div>
  )
}
