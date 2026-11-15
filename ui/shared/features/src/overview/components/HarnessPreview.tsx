/**
 * The coding harness preview, in place (2026-11-06; design: the "Harness Preview Variations" canvas, D, in place of the
 * dialog). While no coding harness is connected, the summary's "No coding harnesses connected" tile carries Watch the
 * preview, which plays Home itself from a developer's first command to a month in, on sample numbers (`preview.ts`): a
 * bar takes the header's place (the stops, the day, pause, Exit), the tile shows the command at Today and from Day 1 is
 * Home's own Harnesses tile, and Home's own Coding harness activity card joins above Needs your attention, both edged
 * and badged as sample data. Once it ends, "Back to your Home in 5s" counts down and Home comes back, unless the viewer
 * picks Stay. Under reduced motion nothing grows: it opens on the end, the stops show each step whole, and only the
 * countdown runs.
 */
import { Link } from '@tanstack/react-router'
import { Pause, Play, X } from 'lucide-react'
import { useReducedMotion } from 'motion/react'
import { useEffect, useRef, useState, type ReactNode } from 'react'
import { Badge } from '@nasiko/ui/primitives/badge'
import { Button, asButton } from '@nasiko/ui/primitives/button'
import { useCopy } from '@nasiko/ui/lib/useCopy'
import { cn } from '@nasiko/ui/lib/utils'
import { connectCommand } from '@nasiko/features/cli/commands'
import { copy as cliCopy } from '@nasiko/features/cli/copy'
import { copy } from '../copy'
import {
  PREVIEW_MS,
  PREVIEW_OFF,
  PREVIEW_STEPS,
  PreviewContext,
  previewAt,
  previewFrame,
  previewTick,
  revealed,
  usePreview,
  type PreviewControl,
} from '../preview'
import { PREVIEW_HOLD_MS, PREVIEW_TICK_MS } from '../tuning'
import { TOUCH } from './Card'

const p = copy.harnessPreview
/** A control on the bar, which is drawn in the page's inverse colours. */
const ON_BAR =
  'border-background/30 bg-transparent text-background hover:bg-background/10 hover:text-background dark:border-background/30 dark:bg-transparent dark:hover:bg-background/10'

/** Home's preview clock: the page's header, tiles and cards read it, so the play happens on Home itself. */
export function PreviewProvider({ children }: { children: ReactNode }) {
  const reduced = useReducedMotion() ?? false
  const [state, set] = useState(PREVIEW_OFF)
  const ticking = state.on && (state.playing || (state.elapsed >= PREVIEW_MS && !state.stay))
  useEffect(() => {
    if (!ticking) return
    const t = setInterval(() => set((s) => previewTick(s, PREVIEW_TICK_MS)), PREVIEW_TICK_MS)
    return () => clearInterval(t)
  }, [ticking])
  // Ending with the focus in the bar takes the focus with it: it goes back to Watch the preview, whose tile is back.
  const wasOn = useRef(false)
  useEffect(() => {
    if (state.on) {
      wasOn.current = true
      return
    }
    if (!wasOn.current) return
    wasOn.current = false
    if (document.activeElement === document.body)
      document.querySelector<HTMLElement>('[data-preview-watch]')?.focus({ preventScroll: true })
  }, [state.on])
  return <PreviewContext value={{ state, reduced, set }}>{children}</PreviewContext>
}

/** The play button, in the "no coding harnesses connected" tile (user 2026-11-06: no section of its own). Only Home
 *  plays the preview: elsewhere (Weave's catalog) it isn't offered. */
export function WatchPreview({ className }: { className?: string }) {
  const pv = usePreview()
  if (!pv) return null
  return (
    <Button
      data-preview-watch
      size="sm"
      className={cn('gap-2 pl-1.5', TOUCH, className)}
      onClick={() => pv.set(previewAt(pv.reduced ? PREVIEW_STEPS - 1 : 0, pv.reduced))}
    >
      <span
        aria-hidden
        className="flex size-5 items-center justify-center rounded-full bg-primary-foreground text-primary-text"
      >
        <Play className="size-2.5 fill-current" />
      </span>
      {p.watch}
    </Button>
  )
}

/** Home's header, or while the preview plays, the bar that runs it. */
export function PreviewHeader({ children }: { children: ReactNode }) {
  const pv = usePreview()
  return pv?.state.on ? <Bar {...pv} /> : children
}

function Bar({ state, reduced, set }: PreviewControl) {
  const ref = useRef<HTMLElement>(null)
  // Watch the preview went with its tile: the bar takes the focus, so the keyboard lands on its controls.
  useEffect(() => ref.current?.focus({ preventScroll: true }), [])
  const f = previewFrame(state.elapsed)
  const counting = f.done && !state.stay
  const secs = Math.max(1, Math.ceil((PREVIEW_HOLD_MS - state.hold) / 1000))
  const stop = p.stops[f.step] ?? p.stops[0]
  const exit = () => set(PREVIEW_OFF)
  // Every developer runs these on their own machine. Copying keeps the preview, so a blocked clipboard's fallback stays.
  const commands = [cliCopy.install.unixCommand, connectCommand()].join('\n')
  const [copied, runCopy] = useCopy(commands)
  // Keyed, so Stay's switch from the countdown keeps these (and the focus on them).
  const copyButton = (
    <Button
      key="copy"
      variant="outline"
      size="sm"
      className={cn(ON_BAR, TOUCH)}
      onClick={() => {
        set((s) => ({ ...s, stay: true }))
        void runCopy()
      }}
    >
      {copied === 'copied' ? p.copied : p.copyCommands}
    </Button>
  )
  const connect = (
    <Link
      key="connect"
      to="/harnesses"
      search={{} as never}
      {...asButton({
        size: 'sm',
        className: cn('bg-background text-foreground hover:bg-background/90', TOUCH),
      })}
    >
      {p.connectYours}
    </Link>
  )
  const exitButton = (
    <Button variant="outline" size="sm" className={cn(ON_BAR, TOUCH)} onClick={exit}>
      {p.exit}
    </Button>
  )
  return (
    <>
      {/* The page keeps its heading while the bar stands in for the header. */}
      <h1 className="sr-only">{copy.title}</h1>
      {/* Sticky, so its controls and countdown stay in reach while the activity card below is read. */}
      <section
        ref={ref}
        tabIndex={-1}
        aria-label={p.bar}
        className="sticky top-(--topbar-h) z-20 flex flex-wrap items-center gap-x-4 gap-y-2 rounded-xl bg-foreground py-2 pr-2 pl-4 text-background outline-none"
      >
        <Badge className="border-transparent bg-warning text-background uppercase">
          {p.sample}
        </Badge>
        <p
          aria-live="polite"
          className={cn(
            'min-w-0 flex-1 truncate text-sm font-semibold @[1160px]/overview:flex-none',
            // A fixed width while the stops change the title, so the timeline holds still; at the end, where the
            // countdown and its buttons need the room to stay on one line, only as wide as it says.
            !f.done && '@[1160px]/overview:w-60',
          )}
        >
          {stop.title}
        </p>
        {/* One row from 1160px (the ending's countdown and buttons fit there, sidebar open on a 1440px screen);
            narrower, the timeline takes a row of its own rather than the buttons breaking mid-row. */}
        <nav
          aria-label={p.timeline}
          className="order-last grid basis-full grid-cols-4 gap-1.5 @[1160px]/overview:order-none @[1160px]/overview:flex-1 @[1160px]/overview:basis-auto"
        >
          {p.stops.map((s, i) => (
            <Button
              key={s.label}
              variant="ghost"
              aria-label={p.jump(s.label)}
              aria-current={i === f.step ? 'step' : undefined}
              onClick={() => set(previewAt(i, reduced))}
              className={cn(
                'h-auto flex-col items-stretch gap-1.5 rounded-sm px-0 py-1 text-background hover:bg-transparent hover:text-background dark:hover:bg-transparent',
                TOUCH,
              )}
            >
              <span aria-hidden className="block h-1 overflow-hidden rounded-full bg-background/25">
                <span
                  className="block h-full bg-background"
                  style={{
                    width: `${f.done || i < f.step ? 100 : i === f.step ? f.progress * 100 : 0}%`,
                  }}
                />
              </span>
              <span
                className={cn(
                  'text-left text-xs',
                  i === f.step ? 'font-semibold' : 'font-normal opacity-75',
                )}
              >
                {s.label}
              </span>
            </Button>
          ))}
        </nav>
        {f.step >= 1 ? (
          <span className="rounded-full border border-background/30 px-2.5 py-0.5 text-xs font-semibold tabular-nums">
            {p.day(f.day)}
          </span>
        ) : null}
        <div className="flex flex-wrap items-center gap-2">
          {counting ? (
            <>
              <span className="mr-1 flex items-center gap-2 text-sm">
                <Ring hold={state.hold} secs={secs} />
                {p.back(secs)}
              </span>
              <Button
                variant="outline"
                size="sm"
                className={cn(ON_BAR, TOUCH)}
                onClick={() => set((s) => ({ ...s, stay: true }))}
              >
                {p.stay}
              </Button>
              {copyButton}
              {connect}
              <Button
                variant="ghost"
                size="icon-sm"
                aria-label={p.exitNow}
                className={cn(ON_BAR, TOUCH)}
                onClick={exit}
              >
                <X aria-hidden />
              </Button>
            </>
          ) : state.stay ? (
            <>
              {copyButton}
              {connect}
              {exitButton}
            </>
          ) : (
            <>
              {reduced ? null : (
                <Button
                  variant="outline"
                  size="icon-sm"
                  aria-label={state.playing ? p.pause : p.play}
                  className={cn('rounded-full', ON_BAR, TOUCH)}
                  onClick={() => set((s) => ({ ...s, playing: !s.playing }))}
                >
                  {state.playing ? <Pause aria-hidden /> : <Play aria-hidden />}
                </Button>
              )}
              {exitButton}
            </>
          )}
        </div>
        {/* The clipboard can be blocked (plain HTTP): the commands stay selectable. */}
        {copied === 'failed' ? (
          <pre className="order-last basis-full overflow-x-auto rounded-md border border-background/30 p-2 font-mono text-xs select-all">
            {commands}
          </pre>
        ) : null}
        {/* Said once as the countdown starts; the visible seconds tick without being read out. */}
        <span className="sr-only" aria-live="polite">
          {counting ? p.backSoon(PREVIEW_HOLD_MS / 1000) : ''}
        </span>
      </section>
    </>
  )
}

/** The countdown's ring, emptying as Home comes back (at rest under reduced motion: the number still counts). */
function Ring({ hold, secs }: { hold: number; secs: number }) {
  const around = 2 * Math.PI * 11
  return (
    <span aria-hidden className="relative flex size-7 items-center justify-center">
      <svg viewBox="0 0 28 28" className="absolute inset-0 -rotate-90">
        <circle
          cx="14"
          cy="14"
          r="11"
          fill="none"
          strokeWidth="3"
          className="stroke-background/25"
        />
        <circle
          cx="14"
          cy="14"
          r="11"
          fill="none"
          strokeWidth="3"
          strokeLinecap="round"
          strokeDasharray={around}
          strokeDashoffset={(around * hold) / PREVIEW_HOLD_MS}
          className="stroke-background motion-reduce:hidden"
        />
      </svg>
      <span className="text-[0.6875rem] font-semibold tabular-nums">{secs}</span>
    </span>
  )
}

/** The dashed edge and badge that mark what draws sample numbers. Its parent is `relative`; `inset` keeps both inside a
 *  cell whose card clips its edges. */
function SampleEdge({ inset = false }: { inset?: boolean }) {
  return (
    <>
      <span
        aria-hidden
        className={cn(
          'pointer-events-none absolute rounded-lg border-[1.5px] border-dashed border-warning',
          inset ? 'inset-1.5' : 'inset-0',
        )}
      />
      <Badge
        variant="warning"
        className={cn('absolute bg-card uppercase', inset ? 'top-4 right-4' : '-top-2.5 left-4')}
      >
        {p.sample}
      </Badge>
    </>
  )
}

/** Home's own tile or card, drawing the preview's sample numbers. */
export function Sampled({ children, className }: { children: ReactNode; className?: string }) {
  return (
    <div
      className={cn(
        'relative flex animate-in duration-500 fill-mode-both fade-in *:flex-1 motion-reduce:animate-none',
        className,
      )}
    >
      {children}
      <SampleEdge />
    </div>
  )
}

/** The "no coding harnesses" tile's body: its own (`children`) until the preview plays, then Today's command, its
 *  answers appearing in turn. From Day 1 the tile is Home's own Harnesses tile (SummaryCard). Connecting signs in, then
 *  connects and imports (`nasiko connect`). The tile is `relative`. */
export function PreviewSlot({ children }: { children: ReactNode }) {
  const pv = usePreview()
  if (!pv?.state.on) return children
  const f = previewFrame(pv.state.elapsed)
  const lines = revealed(f.progress, p.terminal.length + 1) - 1
  return (
    <>
      <SampleEdge inset />
      <div className="flex w-full flex-1 flex-col rounded-md bg-muted px-3 py-2 font-mono text-xs leading-6">
        <p>
          <span aria-hidden className="text-muted-foreground select-none">
            ${' '}
          </span>
          {connectCommand()}
        </p>
        {p.terminal.slice(0, lines).map((line) => (
          <p key={line} className="text-success">
            <span aria-hidden>✓ </span>
            {line}
          </p>
        ))}
      </div>
    </>
  )
}
