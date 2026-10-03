/**
 * "How routing works" (plan §4.1): the page title's disclosure. Five numbered steps, left to right from lg on a line a
 * beam travels along and stacked below with a connecting line, then a footer with this viewer's own numbers.
 * Opens by default on a first visit with no configs; an explicit choice is remembered (prefs.ts).
 */
import { AnimatePresence, m } from 'motion/react'
import {
  ChevronDown,
  KeyRound,
  Layers,
  RotateCw,
  Send,
  SlidersHorizontal,
  X,
  type LucideIcon,
} from 'lucide-react'
import { useRef, useState } from 'react'
import { PageHeader } from '@/components/shared/page-header'
import { Button } from '@/components/ui/button'
import { enter, transitions } from '@/lib/motion'
import { cn } from '@/lib/utils'
import { copy } from '../copy'
import { PREF_HOW, readOpen, writeOpen } from '../prefs'
import { LinkButton } from './bits'

const ICONS: readonly LucideIcon[] = [Send, SlidersHorizontal, Layers, KeyRound, RotateCw]
/** When each step lights in the beam's 4 s pass: 0.2 s before the beam's centre reaches it (steps at 0, ¼ … 1 of the
 *  line, so centres at 0.4, 1.2 … 3.6 s); negative, so the loop is already running when the card opens. */
const STEP_DELAYS = ['-3.8s', '-3s', '-2.2s', '-1.4s', '-0.6s']

export function RouterTitle({
  configsEmpty,
  onDefault,
  total,
  defaultName,
  onShowDefaults,
}: {
  configsEmpty: boolean
  /** "14" (or "at least 13") agents on the default, when there is a default. */
  onDefault?: string
  total?: string
  defaultName?: string
  /** Narrow Your agents to the ones on the default. */
  onShowDefaults?: () => void
}) {
  const [open, setOpen] = useState<boolean>(() => readOpen(PREF_HOW) ?? false)
  const [touched, setTouched] = useState(() => readOpen(PREF_HOW) !== null)
  // First visit with no configs: open by default (plan §4.1); an explicit choice always wins.
  const shown = touched ? open : open || configsEmpty
  const toggle = (next: boolean) => {
    setTouched(true)
    setOpen(next)
    writeOpen(PREF_HOW, next)
  }
  const toggleRef = useRef<HTMLButtonElement>(null)
  return (
    <div className="space-y-3">
      <PageHeader
        title={copy.title}
        actions={
          <Button
            ref={toggleRef}
            variant="ghost"
            size="sm"
            aria-expanded={shown}
            aria-controls="router-how"
            onClick={() => toggle(!shown)}
            className="pointer-coarse:min-h-11"
          >
            {copy.howItWorks}{' '}
            <ChevronDown
              className={cn(
                'size-4 transition-transform motion-reduce:transition-none',
                shown && 'rotate-180',
              )}
              aria-hidden
            />
          </Button>
        }
      />
      <AnimatePresence initial={false}>
        {shown ? (
          <m.section
            id="router-how"
            aria-labelledby="router-how-h"
            initial={{ opacity: 0, y: -4 }}
            animate={{ opacity: 1, y: 0 }}
            exit={{ opacity: 0, y: -4 }}
            transition={transitions.disclosure}
            className="relative rounded-lg border border-border bg-card"
          >
            <div className="flex items-center justify-between gap-2 px-4 pt-3">
              <h2
                id="router-how-h"
                className="text-xs font-medium tracking-wide text-muted-foreground uppercase"
              >
                {copy.howTitle}
              </h2>
              <Button
                variant="ghost"
                size="sm"
                className="size-7 px-0 text-muted-foreground pointer-coarse:size-11"
                onClick={() => {
                  toggle(false)
                  toggleRef.current?.focus()
                }}
                aria-label={copy.howClose}
              >
                <X className="size-4" aria-hidden />
              </Button>
            </div>
            {/* Our take on Aceternity's Tracing Beam, as on the onboarding Welcome step (index.css `--animate-beam` /
                `--animate-stage`): from lg a beam runs along the line through the five steps and each step's icon
                lights as it passes; stacked, the icons still light in turn. Under reduced motion nothing moves and the
                first step stays lit. Decorative: the words carry the meaning. */}
            <div className="relative px-4 pt-3 pb-4">
              <div
                aria-hidden
                className="pointer-events-none absolute top-7 right-[calc(20%-1.2rem)] left-8 hidden h-px overflow-hidden bg-border lg:block"
              >
                <div className="absolute inset-y-0 w-1/4 animate-beam bg-linear-to-r from-transparent via-primary to-transparent motion-reduce:hidden" />
              </div>
              <ol className="grid gap-0 lg:grid-cols-5 lg:gap-4">
                {copy.howSteps.map((step, i) => {
                  const Icon = ICONS[i] ?? Send
                  const last = i === copy.howSteps.length - 1
                  return (
                    <m.li
                      key={step.title}
                      initial={{ opacity: 0, y: enter.rise }}
                      animate={{ opacity: 1, y: 0 }}
                      transition={{ ...enter.transition, delay: i * enter.stagger * 2 }}
                      className="relative flex gap-3 pb-4 last:pb-0 lg:flex-col lg:gap-2.5 lg:pb-0"
                    >
                      {/* Stacked: a line joins the icons. */}
                      {!last ? (
                        <span
                          aria-hidden
                          className="absolute top-8 bottom-0 left-4 w-px bg-border lg:hidden"
                        />
                      ) : null}
                      <span
                        aria-hidden
                        style={{ animationDelay: STEP_DELAYS[i] }}
                        className={cn(
                          'relative z-10 flex size-8 shrink-0 animate-stage items-center justify-center rounded-lg border bg-muted text-foreground ring-4 ring-card motion-reduce:animate-none',
                          i === 0 &&
                            'motion-reduce:border-primary motion-reduce:bg-primary motion-reduce:text-primary-foreground',
                        )}
                      >
                        <Icon className="size-4" />
                      </span>
                      <div className="min-w-0 space-y-0.5">
                        <p className="flex items-baseline gap-2 text-sm font-medium">
                          {step.title}
                          <span className="font-mono text-2xs font-normal text-muted-foreground tabular-nums">
                            {String(i + 1).padStart(2, '0')}
                          </span>
                        </p>
                        <p className="text-xs text-pretty text-muted-foreground">{step.detail}</p>
                      </div>
                    </m.li>
                  )
                })}
              </ol>
            </div>
            <div className="flex flex-wrap items-center gap-x-3 gap-y-1 border-t border-border bg-muted/40 px-4 py-2.5 text-xs">
              {onDefault && total ? (
                <span>
                  {copy.howCounts(onDefault, total, defaultName)}
                  {onShowDefaults ? (
                    <>
                      {' '}
                      <LinkButton className="text-xs" onClick={onShowDefaults}>
                        {copy.howShowDefaults}
                      </LinkButton>
                    </>
                  ) : null}
                </span>
              ) : null}
              <span className="text-muted-foreground">{copy.configuredNote}</span>
            </div>
          </m.section>
        ) : null}
      </AnimatePresence>
    </div>
  )
}
