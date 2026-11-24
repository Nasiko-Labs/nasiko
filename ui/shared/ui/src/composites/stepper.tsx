/**
 * A stepper: the steps on a track, the current step's content, then Back, "Step n of N" and the step's own action.
 *
 * Source: shadcn-space's `stepper-03` (components/shadcn-space/stepper/stepper-03.tsx), made a shared composite.
 * Local edits (re-apply after any re-vendor):
 * - the steps, the current step, which steps can open and the footer action are props (the demo's own four steps,
 *   their placeholder content and its Continue are gone); a step that can't open yet is disabled;
 * - the step circles are the `Button` primitive, not raw buttons (eslint `noRawElements`), in an `ol` inside a `nav`;
 * - the demo's motion is kept as named presets (`lib/motion.ts` `stepTrack`, `stepDot`, `stepLift`, `stepIcon`,
 *   `stepPulse`; `disclosure` for the content), the current step's pulse included (the user asked for the stepper's
 *   own animation, 2026-11-24). Under reduced motion nothing moves: the pulse is a still ring instead, and the dot
 *   (it moves by `left`, which `MotionConfig reducedMotion="user"` doesn't stop) jumps;
 * - theme tokens only: the check was `text-teal-400`, now `text-primary-foreground` on the primary circle;
 * - no card: the demo's bordered, padded box is gone, so the stepper sits on the page (the user, 2026-11-24);
 * - edge to edge, so it is as wide as the content under it: the first step sits on the left edge and the last on the
 *   right (their labels aligned to that edge), the track runs from the first circle's center to the last's, and the
 *   steps between are evenly spaced on it (the demo centered every step in an equal column, inset 12.5 % for four);
 * - an optional `detail` line under a step's title; the content is a region named after its step that takes focus
 *   when the step changes (never on first paint); the footer's count is a polite live region naming the step;
 * - no "use client" (a Vite SPA).
 */
import { AnimatePresence, motion, useReducedMotion } from 'motion/react'
import { Check, ChevronLeft, type LucideIcon } from 'lucide-react'
import { useCallback, useEffect, useRef, type ReactNode } from 'react'
import { transitions } from '../lib/motion'
import { cn } from '../lib/utils'
import { Button } from '../primitives/button'
import { Separator } from '../primitives/separator'

export interface StepperStep {
  title: string
  icon: LucideIcon
  /** A short line under the title: what was picked there. */
  detail?: string | undefined
}

const ICON_IN = { scale: 1, rotate: 0, opacity: 1 }

export function Stepper({
  label,
  steps,
  active,
  canOpen,
  onStepChange,
  action,
  children,
  className,
}: {
  /** Names the list of steps, e.g. "Evals steps". */
  label: string
  steps: readonly StepperStep[]
  active: number
  /** Whether a step other than the current one can be opened now. */
  canOpen: (index: number) => boolean
  onStepChange: (index: number) => void
  /** The current step's own action (its Continue), at the end of the footer. */
  action?: ReactNode
  /** The current step's content. */
  children: ReactNode
  className?: string
}) {
  const reduce = useReducedMotion()
  const n = steps.length
  const progress = n > 1 ? active / (n - 1) : 0
  const current = steps[active]

  // The first content is the page's own; a later one arrived by a step change, so it takes focus (and the view).
  const painted = useRef(false)
  useEffect(() => {
    painted.current = true
  }, [])
  const arrive = useCallback((el: HTMLElement | null) => {
    if (el && painted.current) el.focus()
  }, [])

  return (
    <div className={cn('flex flex-col gap-6', className)}>
      <nav aria-label={label} className="relative">
        {/* From the first circle's center to the last's: inset by a circle's radius (size-10 / 2). */}
        <div aria-hidden className="absolute inset-x-5 top-5">
          <div className="h-0.5 bg-border" />
          <motion.div
            className="absolute inset-x-0 top-0 h-0.5 origin-left bg-primary"
            initial={false}
            animate={{ scaleX: progress }}
            transition={transitions.stepTrack}
          />
          <motion.span
            className="absolute top-px size-2 rounded-full bg-primary"
            style={{ x: '-50%', y: '-50%' }}
            initial={{ left: '0%' }}
            animate={{ left: `${progress * 100}%` }}
            transition={reduce ? { duration: 0 } : transitions.stepDot}
          />
        </div>
        {/* Over the track's width the end steps get half a share and the others a whole one, so every circle sits on
            its point of the track; the end steps are then nudged out by a radius, onto the edges. */}
        <ol className="relative flex items-start px-5">
          {steps.map((step, i) => {
            const isActive = i === active
            const done = i < active
            const Icon = step.icon
            return (
              <li
                key={step.title}
                className={cn(
                  'relative flex min-w-0 flex-col gap-1.5',
                  i === 0
                    ? '-left-5 flex-[0.5] items-start pr-2 text-left'
                    : i === n - 1
                      ? 'left-5 flex-[0.5] items-end pl-2 text-right'
                      : 'flex-1 items-center px-1 text-center',
                )}
              >
                <Button
                  variant="ghost"
                  size="icon"
                  aria-current={isActive ? 'step' : undefined}
                  aria-label={`${step.title} step`}
                  disabled={!isActive && !canOpen(i)}
                  onClick={() => {
                    if (!isActive) onStepChange(i)
                  }}
                  className="group relative z-10 size-10 rounded-full p-0 hover:bg-transparent"
                >
                  <span
                    className={cn(
                      'absolute inset-0 rounded-full transition-[background-color,box-shadow] duration-200',
                      done || isActive ? 'bg-primary' : 'bg-muted group-hover:bg-muted/80',
                      isActive && reduce && 'ring-4 ring-primary/20',
                    )}
                  />
                  {isActive && !reduce ? (
                    <motion.span
                      aria-hidden
                      className="absolute inset-0 rounded-full ring-2 ring-primary/50"
                      initial={{ scale: 1, opacity: 1 }}
                      animate={{ scale: [1, 1.45, 1], opacity: [1, 0.2, 1] }}
                      transition={transitions.stepPulse}
                    />
                  ) : null}
                  <motion.span
                    className="relative flex items-center justify-center"
                    animate={{ scale: isActive ? 1.1 : 1 }}
                    transition={transitions.stepLift}
                  >
                    <AnimatePresence mode="wait" initial={false}>
                      {done ? (
                        <motion.span
                          key="check"
                          initial={{ scale: 0, rotate: -90, opacity: 0 }}
                          animate={ICON_IN}
                          exit={{ scale: 0, rotate: 90, opacity: 0 }}
                          transition={transitions.stepIcon}
                          className="flex text-primary-foreground"
                        >
                          <Check className="size-5" strokeWidth={3} />
                        </motion.span>
                      ) : (
                        <motion.span
                          key="icon"
                          initial={{ scale: 0, opacity: 0 }}
                          animate={ICON_IN}
                          exit={{ scale: 0, opacity: 0 }}
                          transition={transitions.stepIcon}
                          className="flex"
                        >
                          <Icon
                            className={cn(
                              'size-5',
                              isActive ? 'text-primary-foreground' : 'text-muted-foreground',
                            )}
                          />
                        </motion.span>
                      )}
                    </AnimatePresence>
                  </motion.span>
                </Button>
                <span
                  className={cn(
                    'mt-0.5 text-xs font-medium transition-colors duration-200',
                    isActive || done ? 'text-foreground' : 'text-muted-foreground',
                  )}
                >
                  {step.title}
                </span>
                {step.detail ? (
                  <span className="max-w-full truncate text-xs text-muted-foreground">
                    {step.detail}
                  </span>
                ) : null}
              </li>
            )
          })}
        </ol>
      </nav>

      <AnimatePresence mode="wait" initial={false}>
        <motion.section
          key={active}
          ref={arrive}
          tabIndex={-1}
          aria-label={current?.title}
          initial={{ opacity: 0, y: 8 }}
          animate={{ opacity: 1, y: 0 }}
          exit={{ opacity: 0, y: -8 }}
          transition={transitions.disclosure}
          className="min-w-0 outline-none"
        >
          {children}
        </motion.section>
      </AnimatePresence>

      <Separator />
      {/* Three columns from `sm`, so the count stays centered whatever the action's width; wrapped below. */}
      <div className="flex flex-wrap items-center justify-between gap-3 sm:grid sm:grid-cols-[1fr_auto_1fr]">
        <Button
          variant="outline"
          disabled={active === 0 || !canOpen(active - 1)}
          onClick={() => onStepChange(active - 1)}
          className="sm:justify-self-start"
        >
          <ChevronLeft aria-hidden />
          Back
        </Button>
        <p aria-live="polite" className="text-sm text-muted-foreground">
          Step {active + 1} of {n}
          <span className="sr-only">: {current?.title}</span>
        </p>
        <div className="flex min-h-9 items-center gap-2 sm:justify-self-end">{action}</div>
      </div>
    </div>
  )
}
