/**
 * Home's call to run the agent the viewer picked during setup: "Run it" plays a run on the button itself (Starting your
 * agent → Running your agent → Your agent completed, then the button leaves), never leaving Home; a line under it then
 * says what the run cost, links to what the agent did and asks for a refresh to see its stats. Then "Run it optimized"
 * becomes the call: the same task with Token optimization on, the same experience, and a refresh to see what it saved.
 * Simulated for now (tryRun.ts): no request is sent and the figures are the demo's.
 */
import { Link } from '@tanstack/react-router'
import { CheckCircle2, Loader2, Play, RotateCw, Sparkles } from 'lucide-react'
import { useEffect, useState } from 'react'
import { Button } from '@nasiko/ui/primitives/button'
import { cn } from '@nasiko/ui/lib/utils'
import { copy } from '../copy'
import {
  nextPhase,
  readDone,
  TRY_RUN_AGENT,
  writeDone,
  type TryRunKind,
  type TryRunPhase,
} from '../tryRun'
import { Card, TOUCH } from './Card'

const t = copy.tryRun

function RunButton({
  kind,
  phase,
  primary,
  disabled,
  onRun,
}: {
  kind: TryRunKind
  /** The card's call to action: the run still to do. */
  primary: boolean
  /** This button's run, or idle while the other one runs. */
  phase: TryRunPhase
  disabled: boolean
  onRun(): void
}) {
  const Icon = kind === 'baseline' ? Play : Sparkles
  const label =
    phase === 'starting'
      ? t.starting
      : phase === 'running'
        ? t.running
        : phase === 'completed'
          ? t.completed
          : t[kind]
  return (
    <Button
      variant={primary ? 'default' : 'outline'}
      className={`justify-start ${TOUCH}`}
      disabled={disabled}
      aria-busy={phase === 'starting' || phase === 'running'}
      onClick={onRun}
    >
      {phase === 'starting' || phase === 'running' ? (
        <Loader2 className="animate-spin motion-reduce:animate-none" aria-hidden />
      ) : phase === 'completed' ? (
        <CheckCircle2 aria-hidden />
      ) : (
        <Icon aria-hidden />
      )}
      {label}
    </Button>
  )
}

function Result({ kind }: { kind: TryRunKind }) {
  return (
    <div className="flex flex-col gap-1 text-sm" data-testid={`try-run-result-${kind}`}>
      <p>{kind === 'baseline' ? t.baselineDone(TRY_RUN_AGENT) : t.optimizedDone(TRY_RUN_AGENT)}</p>
      <div className="flex flex-wrap items-center gap-x-3 gap-y-1">
        <Link to="/sessions" className="text-primary-text underline underline-offset-4">
          {t.seeRun}
        </Link>
        <span className="text-muted-foreground">
          {kind === 'baseline' ? t.refreshStats : t.refreshOptimized}
        </span>
        <Button size="sm" variant="outline" onClick={() => globalThis.location.reload()}>
          <RotateCw className="size-3.5" aria-hidden /> {t.refresh}
        </Button>
      </div>
    </div>
  )
}

export function TryAgentRun({ userId, className }: { userId: string; className?: string }) {
  // Runs finished in this tab (kept through the refresh), and the one playing now.
  const [done, setDone] = useState(() => readDone(userId))
  const [active, setActive] = useState<{ kind: TryRunKind; phase: TryRunPhase } | null>(null)
  // Results shown on this visit, newest last; a refresh starts the card clean.
  const [shown, setShown] = useState<TryRunKind[]>([])

  useEffect(() => {
    if (!active) return
    const step = nextPhase(active.phase)
    if (!step) return
    const timer = setTimeout(() => {
      if (step.next === 'completed') {
        writeDone(userId, active.kind)
        setDone((d) => (d.includes(active.kind) ? d : [...d, active.kind]))
        setShown((s) => [...s.filter((k) => k !== active.kind), active.kind])
      }
      setActive(step.next === 'idle' ? null : { kind: active.kind, phase: step.next })
    }, step.afterMs)
    return () => clearTimeout(timer)
  }, [active, userId])

  const run = (kind: TryRunKind) => setActive({ kind, phase: 'starting' })
  const phaseOf = (kind: TryRunKind): TryRunPhase => (active?.kind === kind ? active.phase : 'idle')
  // Once the agent has run as it is, the optimized run is the call to action.
  const ranBaseline = done.includes('baseline')

  return (
    // A mild gradient border to draw the eye to the button: a thin frame glowing from the bottom-left corner, fading into
    // the usual hairline elsewhere, around the card, whose own border is clear.
    <div
      className={cn(
        'rounded-lg bg-radial-[at_0%_100%] from-info via-chart-3/60 to-border to-70% p-[1.5px]',
        className,
      )}
    >
      <Card
        id="overview-try-run"
        title={t.title(TRY_RUN_AGENT)}
        className="h-full rounded-[calc(var(--radius-lg)-1.5px)] border-transparent"
      >
        <div className="flex flex-col gap-3">
          <p className="text-sm text-muted-foreground">{t.lead}</p>
          <figure className="flex flex-col gap-1">
            <figcaption className="text-xs font-medium">{t.requestLabel}</figcaption>
            <blockquote className="border-l-2 pl-3 text-xs text-muted-foreground">
              {t.request}
            </blockquote>
          </figure>
          {ranBaseline && !done.includes('optimized') && !active ? (
            <p className="text-sm font-medium">{t.nextStep}</p>
          ) : null}
          <div className="flex flex-wrap gap-2">
            {/* Gone once it has run (after its completed state plays out): the optimized run is what's left to do. */}
            {!ranBaseline || active?.kind === 'baseline' ? (
              <RunButton
                kind="baseline"
                phase={phaseOf('baseline')}
                primary
                disabled={!!active}
                onRun={() => run('baseline')}
              />
            ) : null}
            {ranBaseline && active?.kind !== 'baseline' ? (
              <RunButton
                kind="optimized"
                phase={phaseOf('optimized')}
                primary
                disabled={!!active}
                onRun={() => run('optimized')}
              />
            ) : null}
          </div>
          <div aria-live="polite" className="flex flex-col gap-3 empty:hidden">
            {shown.map((kind) => (
              <Result key={kind} kind={kind} />
            ))}
          </div>
        </div>
      </Card>
    </div>
  )
}
