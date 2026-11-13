/**
 * The onboarding guide (docs/superpowers/specs/2026-10-01-login-onboarding-design.md §3): the prototype's dialog, a
 * left rail of three steps beside the step itself, with Back / Continue below. Lazy (index.ts). Finish lands on the
 * homepage (user request 2026-11-09).
 */
import { useQueryClient } from '@tanstack/react-query'
import { useNavigate } from '@tanstack/react-router'
import {
  ArrowLeft,
  ArrowRight,
  Bot,
  Check,
  Clock,
  DollarSign,
  Gauge,
  Play,
  Plug,
  ShieldCheck,
  SquareTerminal,
  WandSparkles,
  X,
} from 'lucide-react'
import { useEffect, useRef, useState } from 'react'
import { agentKeys } from '@nasiko/features/agents/api'
import { DotBackground } from '@nasiko/ui/composites/dot-background'
import { GlowingEffect } from '@nasiko/ui/composites/glowing-effect'
import { NasikoMark } from '@nasiko/ui/composites/NasikoMark'
import { TextGenerateEffect } from '@nasiko/ui/composites/text-generate-effect'
import { Button } from '@nasiko/ui/primitives/button'
import { Dialog, DialogContent } from '@nasiko/ui/primitives/dialog'
import { Progress } from '@nasiko/ui/primitives/progress'
import { StageIcon } from '@nasiko/features/deploy/components/StageList'
import type { DeployStarted } from '@nasiko/features/deploy/UploadTab'
import { cn } from '@nasiko/ui/lib/utils'
import { type SeededDeploy, useSeededDeploys } from './api'
import { AgentStep } from './AgentStep'
import { copy } from './copy'
import { HarnessStep } from './HarnessStep'
import { CONTINUE_HOLD_SECONDS, minutesLeft, STEPS, type StepId } from './logic'
import { ModelStep } from './ModelStep'
import { StepHeading } from './parts'

/** The Welcome rows' icons: the nav's own for Agents, Monitor, Optimize and Coding harnesses. */
const ROW_ICONS = [Bot, DollarSign, Gauge, SquareTerminal]
const STAGE_ICONS = [Plug, WandSparkles, Play, ShieldCheck]
/** When each stage lights in the beam's 4 s pass (index.css `--animate-beam` / `--animate-stage`): negative, so the
 *  loop is already running when the step opens. */
const STAGE_DELAYS = ['-3.8s', '-2.73s', '-1.67s', '-0.6s']

export function OnboardingDialog({
  initialStep,
  onClose: close,
}: {
  initialStep: StepId
  /** Skip guide, the close button, Escape, Finish and leaving through a link all end here. */
  onClose: () => void
}) {
  const navigate = useNavigate()
  const qc = useQueryClient()
  // Every way out refreshes the agent lists: an import the guide started registers its agent in seconds but finishes
  // in minutes, so without this the homepage still reads an empty fleet and offers to deploy a first agent, and an
  // Agents list read earlier hides it (NAS-1449). The restart watches are cache-only state.
  const onClose = () => {
    void qc.invalidateQueries({
      queryKey: agentKeys.directory,
      predicate: (q) => q.queryKey[1] !== 'watch',
    })
    close()
  }
  // An opening at a step that no longer exists (a stale link) starts at the beginning.
  const [index, setIndex] = useState(Math.max(0, STEPS.indexOf(initialStep)))
  const [reached, setReached] = useState(index)
  const [model, setModel] = useState<string | null>(null)
  const [agent, setAgent] = useState<DeployStarted | null>(null)
  // Every deploy started from the guide's tabs in this page, kept outside the dialog (NAS-1450): reopened, it shows
  // their progress again.
  const deploys = useSeededDeploys()
  const seeded = deploys.size > 0
  const step = STEPS[index] ?? 'welcome'
  const primary = useRef<HTMLButtonElement>(null)
  const go = (i: number) => {
    setIndex(i)
    setReached((r) => Math.max(r, i))
  }
  const next = () => go(index + 1)
  // The Agent and key step's Continue holds for CONTINUE_HOLD_SECONDS, counting down in its label; each visit restarts it.
  const [hold, setHold] = useState({ index, left: CONTINUE_HOLD_SECONDS })
  if (hold.index !== index) setHold({ index, left: CONTINUE_HOLD_SECONDS })
  const held = step === 'start' ? hold.left : 0
  useEffect(() => {
    if (held <= 0) return
    const t = setTimeout(() => setHold((h) => ({ ...h, left: h.left - 1 })), 1000)
    return () => clearTimeout(t)
  }, [held, index])
  const waiting = held > 0
  const last = index === STEPS.length - 1
  /** Out of the guide to a page: closing first, so the guide never stays open over it. */
  const leave = (to: Parameters<typeof navigate>[0]) => {
    onClose()
    void navigate(to)
  }

  const onContinue = () => {
    // Finish takes everyone to the homepage (user request 2026-11-09).
    if (last) return leave({ to: '/' })
    // Continue only moves on: it deploys nothing (user request 2026-11-12).
    next()
  }

  return (
    <Dialog open onOpenChange={(open) => !open && onClose()}>
      <DialogContent
        showCloseButton={false}
        overlayClassName="backdrop-blur-sm"
        // Start on the step's own action (Get started), not the rail's first button.
        onOpenAutoFocus={(e) => {
          e.preventDefault()
          primary.current?.focus()
        }}
        className="grid h-[min(45rem,calc(100svh-2rem))] grid-rows-1 gap-0 overflow-hidden p-0 sm:max-w-6xl md:grid-cols-[17rem_minmax(0,1fr)]"
      >
        <aside className="hidden min-h-0 flex-col gap-6 border-r bg-muted/60 p-5 md:flex">
          <div className="flex items-center gap-3">
            <NasikoMark className="size-7 shrink-0 text-logo" />
            <div className="min-w-0">
              <p className="text-sm font-semibold">{copy.guide.title}</p>
              <p className="text-xs text-muted-foreground">{copy.guide.meta}</p>
            </div>
          </div>
          <nav aria-label={copy.guide.rail} className="flex min-h-0 flex-1 flex-col gap-1">
            <p className="mb-1 px-2 text-xs font-medium tracking-wide text-muted-foreground uppercase">
              {copy.guide.rail}
            </p>
            {STEPS.map((id, i) => {
              // Ticked only when done, not merely passed: a skipped model or agent stays a number.
              const done =
                i !== index &&
                (id === 'welcome' ? reached > 0 : id === 'start' && (!!model || !!agent || seeded))
              return (
                <Button
                  key={id}
                  type="button"
                  variant="ghost"
                  disabled={i > reached}
                  aria-current={i === index ? 'step' : undefined}
                  onClick={() => go(i)}
                  className={cn(
                    'h-auto justify-start gap-3 px-2 py-2 text-left whitespace-normal',
                    i === index && 'bg-background shadow-xs hover:bg-background',
                  )}
                >
                  <span
                    className={cn(
                      'flex size-6 shrink-0 items-center justify-center rounded-full border text-xs',
                      i === index && 'border-primary bg-primary text-primary-foreground',
                      done && 'border-transparent bg-foreground/10',
                    )}
                  >
                    {done ? <Check aria-hidden className="size-3.5" /> : i + 1}
                  </span>
                  <span className="min-w-0">
                    <span className="block text-sm font-medium">{copy.steps[id].title}</span>
                    <span className="block text-xs font-normal text-muted-foreground">
                      {copy.steps[id].sub}
                    </span>
                  </span>
                </Button>
              )
            })}
          </nav>
          <div className="flex flex-col gap-2">
            <Progress
              value={((index + 1) / STEPS.length) * 100}
              aria-label={copy.guide.step(index + 1, STEPS.length)}
            />
            <p className="flex items-center gap-1.5 text-xs text-muted-foreground">
              <Clock aria-hidden className="size-3.5" />
              {copy.guide.left(minutesLeft(index + 1))}
            </p>
          </div>
        </aside>

        <div className="flex min-h-0 flex-col">
          <div className="flex items-center gap-2 border-b px-6 py-3">
            <span className="text-sm text-muted-foreground">
              {copy.guide.step(index + 1, STEPS.length)}
            </span>
            <span className="flex-1" />
            {last ? null : (
              <Button type="button" variant="ghost" size="sm" onClick={onClose}>
                {copy.guide.skipGuide}
              </Button>
            )}
            <Button
              type="button"
              variant="outline"
              size="icon-sm"
              aria-label={copy.guide.close}
              onClick={onClose}
            >
              <X aria-hidden />
            </Button>
          </div>

          <div className="relative min-h-0 flex-1 overflow-y-auto px-6 py-8 md:px-10">
            {step === 'welcome' ? (
              <>
                <DotBackground />
                <Welcome />
              </>
            ) : null}
            {/* The agent first, then the key (user request 2026-11-05). Continue needs only the hold: a user with no key
                yet may go on (user request 2026-11-06). */}
            {step === 'start' ? (
              <div className="flex flex-col gap-10">
                <StepHeading title={copy.start.title} intro={copy.start.intro} />
                <AgentStep started={agent} onStarted={setAgent} onLeave={onClose} />
                <ModelStep connected={model} onConnected={setModel} />
              </div>
            ) : null}
            {step === 'harness' ? <HarnessStep onOpen={(to) => leave({ to })} /> : null}
          </div>

          <div className="flex items-center gap-2 border-t px-6 py-3">
            {index > 0 ? (
              <Button type="button" variant="ghost" onClick={() => go(index - 1)}>
                <ArrowLeft aria-hidden />
                {copy.guide.back}
              </Button>
            ) : null}
            {/* The agent step has a row per deploy; after it, this line keeps them in view while the guide goes on. */}
            {seeded && step !== 'start' ? <DeploySummary deploys={deploys} /> : null}
            <span className="flex-1" />
            <Button
              ref={primary}
              type="button"
              // Focusable while off, so focus never drops to the page while it holds.
              focusableWhenDisabled
              className="data-disabled:pointer-events-none data-disabled:opacity-50"
              disabled={waiting}
              onClick={onContinue}
            >
              {step === 'welcome'
                ? copy.guide.getStarted
                : last
                  ? copy.guide.finish
                  : held > 0
                    ? copy.guide.continueIn(held)
                    : copy.guide.continue}
              <ArrowRight aria-hidden />
            </Button>
          </div>
        </div>
      </DialogContent>
    </Dialog>
  )
}

/** The guide's deploys (its tabs') in one line: how many are still deploying, then how many run or didn't start. A status
 *  region: it changes only when one finishes. */
function DeploySummary({ deploys }: { deploys: ReadonlyMap<string, SeededDeploy> }) {
  const all = [...deploys.values()]
  const busy = all.filter((d) => d.phase === 'deploying').length
  const bad = all.filter((d) => d.phase === 'failed' || d.phase === 'notRunning').length
  const s = copy.agent.summary
  return (
    <p role="status" className="flex min-w-0 items-center gap-2 text-xs text-muted-foreground">
      <StageIcon state={busy ? 'current' : bad ? 'failed' : 'done'} />
      <span className="truncate">
        {busy ? s.deploying(busy) : bad ? s.failed(bad, all.length) : s.running(all.length)}
      </span>
    </p>
  )
}

/** The Welcome rows' sequence, in seconds: the first row starts as the title settles, then one row every `ROW_STEP`. */
const ROW_START = 0.4
const ROW_STEP = 0.3

/** Where the panel's band sits: its bottom-left, where the last row ends (degrees clockwise from the top). Still: a
 *  band that followed the rows down the edge read as a crawl (user review 2026-11-04). */
const BAND_ANGLE = 248

function Welcome() {
  const rows = copy.welcome.rows
  return (
    // Sized to fit the dialog without scrolling (user review 2026-11-05): tight gaps, 32 px stage icons, short rows.
    <div className="relative flex flex-col gap-5">
      <StepHeading
        title={<TextGenerateEffect words={copy.welcome.title} />}
        intro={copy.welcome.intro}
      />
      {/* The four stages as one flow, as before 2026-11-04 (user review 2026-11-05): our take on Aceternity's Tracing
          Beam. A beam runs along the line from Connect to Govern and each stage's icon lights as it passes; under
          reduced motion Connect stays lit. Decorative: the words carry the meaning. */}
      {/* The line runs between the first and last icons' centres: 1 rem in from each (half a size-8 icon). */}
      <div className="relative">
        <div
          aria-hidden
          className="pointer-events-none absolute top-4 right-[calc(25%-2.125rem)] left-4 hidden h-px overflow-hidden bg-border sm:block"
        >
          <div className="absolute inset-y-0 w-1/4 animate-beam bg-linear-to-r from-transparent via-primary to-transparent motion-reduce:hidden" />
        </div>
        <ol aria-label={copy.welcome.stagesLabel} className="grid gap-x-6 gap-y-4 sm:grid-cols-4">
          {copy.welcome.stages.map((s, i) => {
            const Icon = STAGE_ICONS[i] ?? Plug
            return (
              <li key={s.title} className="relative flex flex-col gap-2">
                <span
                  aria-hidden
                  style={{ animationDelay: STAGE_DELAYS[i] }}
                  className={cn(
                    'flex size-8 animate-stage items-center justify-center rounded-lg border bg-muted text-foreground ring-4 ring-background motion-reduce:animate-none',
                    i === 0 &&
                      'motion-reduce:border-primary motion-reduce:bg-primary motion-reduce:text-primary-foreground',
                  )}
                >
                  <Icon className="size-4" />
                </span>
                <div>
                  <p className="flex items-baseline gap-2 text-sm font-medium">
                    {s.title}
                    <span className="font-mono text-[11px] font-normal text-muted-foreground">{`0${i + 1}`}</span>
                  </p>
                  <p className="text-xs text-pretty text-muted-foreground">{s.line}</p>
                </div>
              </li>
            )
          })}
        </ol>
      </div>
      {/* Capability beside outcome (user review 2026-11-04): what Nasiko does, and what that gets you. The outcome is
          the point, so it carries the weight. The step's one effect area, all Aceternity (user request 2026-11-04): the
          dot background behind it, the title's words arriving, and the Glowing Effect on the panel's edge, the tour's
          pairing (a soft blurred band under a sharp one). Once, on open, the rows switch on in turn: the icon, the
          capability, the arrow, then the outcome's words. The band stays still at the bottom-left. Then nothing moves;
          under reduced motion it is all there at once. */}
      <div className="relative rounded-xl">
        <GlowingEffect
          angle={BAND_ANGLE}
          spread={30}
          blur={12}
          borderWidth={4}
          className="opacity-60"
        />
        <GlowingEffect glow angle={BAND_ANGLE} spread={30} borderWidth={2} />
        <div className="relative overflow-hidden rounded-xl border bg-card shadow-sm">
          <div className="hidden grid-cols-2 gap-6 border-b bg-muted/40 px-4 py-2 text-xs font-medium text-muted-foreground sm:grid">
            <span>{copy.welcome.does}</span>
            <span className="pl-6">{copy.welcome.get}</span>
          </div>
          <dl className="divide-y">
            {rows.map((r, i) => {
              const Icon = ROW_ICONS[i] ?? Bot
              const on = ROW_START + i * ROW_STEP
              return (
                <div
                  key={r.does}
                  data-row
                  className="grid gap-1.5 px-4 py-2 sm:grid-cols-2 sm:items-center sm:gap-6"
                >
                  <dt
                    className={cn(
                      'flex items-center gap-3 text-sm text-muted-foreground',
                      SWITCH_ON,
                    )}
                    style={{ animationDelay: `${on}s` }}
                  >
                    <span
                      className={cn(
                        'flex size-8 shrink-0 items-center justify-center rounded-lg bg-muted text-foreground zoom-in-50',
                        SWITCH_ON,
                      )}
                      style={{ animationDelay: `${on}s` }}
                    >
                      <Icon aria-hidden className="size-4" />
                    </span>
                    {r.does}
                  </dt>
                  <dd className="flex items-center gap-2 pl-11 text-sm font-medium text-pretty sm:pl-0">
                    <ArrowRight
                      aria-hidden
                      className={cn(
                        'hidden size-4 shrink-0 text-muted-foreground slide-in-from-left-2 sm:block',
                        SWITCH_ON,
                      )}
                      style={{ animationDelay: `${on + 0.15}s` }}
                    />
                    <span>
                      <TextGenerateEffect words={r.get} stagger={0.04} delay={on + 0.25} />
                    </span>
                  </dd>
                </div>
              )
            })}
          </dl>
        </div>
      </div>
    </div>
  )
}

/** A Welcome row's part arriving on its turn (tw-animate-css, with the turn as an inline `animation-delay`): hidden
 *  until then, still under reduced motion. */
const SWITCH_ON = 'animate-in fade-in fill-mode-both duration-300 motion-reduce:animate-none'
