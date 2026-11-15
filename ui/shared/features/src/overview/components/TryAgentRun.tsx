/**
 * Home's call to run the agent the viewer picked during setup. "Run it" sends the card's task to the agent for real
 * (tryRunApi.ts) and plays the run on the button (Starting your agent → Running your agent → Your agent completed),
 * never leaving Home; a line under it then says what the run cost, from the run itself, and links to what the agent
 * did. Home refreshes its spend and tokens through `onRan`. Then "Run it optimized" becomes the call: it turns the
 * agent's Token optimization on, runs the same task, and says how much less it cost than the first run. With no LLM
 * router config it can't, so the card asks for one first and offers the run again once one exists.
 */
import { Link } from '@tanstack/react-router'
import { CheckCircle2, Loader2, Play, RotateCw, Sparkles, Waypoints } from 'lucide-react'
import { useEffect, useState } from 'react'
import { Button, asButton } from '@nasiko/ui/primitives/button'
import { fmtCostPerOp, fmtInt } from '@nasiko/ui/lib/format'
import { cn, uuid } from '@nasiko/ui/lib/utils'
import { useAgentsDirectory } from '@nasiko/features/agents/api'
import { useSeededDeploys } from '@nasiko/features/onboarding/api'
import { useConfigs } from '@nasiko/features/router/api'
import { copy } from '../copy'
import {
  readDone,
  readFigures,
  savedPercent,
  TRY_RUN_AGENT,
  writeDone,
  writeFigures,
  type TryRunFigures,
  type TryRunKind,
  type TryRunPhase,
} from '../tryRun'
import { runAgentOnce } from '@nasiko/features/chat/runOnce'
import { enableOptimization } from '../tryRunApi'
import { TRY_RUN_COMPLETED_MS } from '../tuning'
import { Card, TOUCH } from './Card'

const t = copy.tryRun

function RunButton({
  kind,
  phase,
  disabled,
  onRun,
}: {
  kind: TryRunKind
  /** This button's run, or idle while the other one runs. */
  phase: TryRunPhase
  disabled: boolean
  onRun(): void
}) {
  const Icon = kind === 'baseline' ? Play : Sparkles
  const busy = phase === 'starting' || phase === 'running'
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
      className={`justify-start ${TOUCH}`}
      disabled={disabled}
      aria-busy={busy}
      onClick={onRun}
    >
      {busy ? (
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

const figuresText = (f: TryRunFigures | undefined) =>
  f && f.tokens != null && f.costUsd != null
    ? t.figures(fmtInt(f.tokens), fmtCostPerOp(f.costUsd))
    : ''

function Result({
  kind,
  figures,
  sessionId,
}: {
  kind: TryRunKind
  figures: Partial<Record<TryRunKind, TryRunFigures>>
  sessionId: string | null
}) {
  const own = figuresText(figures[kind])
  const saved = kind === 'optimized' ? savedPercent(figures.baseline, figures.optimized) : null
  return (
    <div className="flex flex-col gap-1 text-sm" data-testid={`try-run-result-${kind}`}>
      <p>
        {kind === 'baseline'
          ? t.baselineDone(TRY_RUN_AGENT, own)
          : t.optimizedDone(TRY_RUN_AGENT, own)}
        {saved !== null ? ` ${t.saved(saved)}` : null}
      </p>
      <div className="flex flex-wrap items-center gap-x-3 gap-y-1">
        {sessionId ? (
          <Link
            to="/sessions/$sessionId"
            params={{ sessionId }}
            className="text-primary-text underline underline-offset-4"
          >
            {t.seeRun}
          </Link>
        ) : (
          <Link to="/sessions" className="text-primary-text underline underline-offset-4">
            {t.seeRun}
          </Link>
        )}
        <span className="text-muted-foreground">{t.updated}</span>
      </div>
    </div>
  )
}

export function TryAgentRun({
  userId,
  className,
  onRan,
}: {
  userId: string
  className?: string
  /** A run finished: Home refreshes what it shows (spend, tokens, runs). */
  onRan?: () => void
}) {
  const agentId = useAgentsDirectory().byName.get(TRY_RUN_AGENT)?.id ?? null
  // The Setup guide's deploy of it, while this page has one (NAS-1450): its row exists minutes before it can run.
  const deploy = useSeededDeploys().get(TRY_RUN_AGENT)
  const deploying = deploy?.phase === 'deploying'
  const unstarted = deploy?.phase === 'failed' || deploy?.phase === 'notRunning'
  // Runs finished in this tab (kept through a refresh), what each cost, and the one running now.
  const [done, setDone] = useState(() => readDone(userId))
  const [figures, setFigures] = useState(() => readFigures(userId))
  const [active, setActive] = useState<{ kind: TryRunKind; phase: TryRunPhase } | null>(null)
  const [failure, setFailure] = useState<string | null>(null)
  // Results shown on this visit, newest last, with the run's session; a refresh starts the card clean.
  const [shown, setShown] = useState<{ kind: TryRunKind; sessionId: string | null }[]>([])

  // "Your agent completed" holds for a moment, then the button resets.
  useEffect(() => {
    if (active?.phase !== 'completed') return
    const timer = setTimeout(() => setActive(null), TRY_RUN_COMPLETED_MS)
    return () => clearTimeout(timer)
  }, [active])

  const run = async (kind: TryRunKind) => {
    if (!agentId) return
    setFailure(null)
    setActive({ kind, phase: 'starting' })
    try {
      if (kind === 'optimized') await enableOptimization(agentId)
      const result = await runAgentOnce({
        agentId,
        text: t.request,
        newId: uuid,
        onWorking: () => setActive({ kind, phase: 'running' }),
      })
      const own = { tokens: result.tokens, costUsd: result.costUsd }
      writeDone(userId, kind)
      writeFigures(userId, kind, own)
      setDone((d) => (d.includes(kind) ? d : [...d, kind]))
      setFigures((f) => ({ ...f, [kind]: own }))
      setShown((s) => [...s.filter((r) => r.kind !== kind), { kind, sessionId: result.sessionId }])
      setActive({ kind, phase: 'completed' })
      onRan?.()
    } catch (e) {
      setActive(null)
      setFailure(e instanceof Error ? e.message : String(e))
    }
  }
  const phaseOf = (kind: TryRunKind): TryRunPhase => (active?.kind === kind ? active.phase : 'idle')
  // Once the agent has run as it is, the optimized run is the call to action.
  const ranBaseline = done.includes('baseline')
  const next: TryRunKind = ranBaseline && active?.kind !== 'baseline' ? 'optimized' : 'baseline'
  // Home's own "LLM router" setup rule (api.ts useSetupSteps): no config yet. A config saved on /router (or in another
  // tab, refetched on focus) brings the button back. Unknown while loading, so the button waits; a failed read lets it run.
  const configs = useConfigs()
  const needsRouter = next === 'optimized' && configs.data?.length === 0

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
        title={deploying ? t.deployingTitle(TRY_RUN_AGENT) : t.title(TRY_RUN_AGENT)}
        className="h-full rounded-[calc(var(--radius-lg)-1.5px)] border-transparent"
      >
        <div className="flex flex-col gap-3">
          <p className="text-sm text-muted-foreground">{t.lead}</p>
          <figure className="flex flex-col gap-1">
            <figcaption className="text-xs font-medium">{t.requestLabel}</figcaption>
            <blockquote className="max-h-32 overflow-y-auto border-l-2 pl-3 text-xs whitespace-pre-line text-muted-foreground">
              {t.request}
            </blockquote>
          </figure>
          {deploying ? (
            <p className="text-sm text-muted-foreground">{t.deployingHint}</p>
          ) : unstarted ? (
            <p className="flex flex-wrap items-center gap-x-3 gap-y-1 text-sm text-destructive">
              <span>
                {t.deployFailed(TRY_RUN_AGENT, deploy.phase === 'failed' ? deploy.reason : null)}
              </span>
              <Link to="/agents" className="text-primary-text underline underline-offset-4">
                {t.openAgents}
              </Link>
            </p>
          ) : !agentId ? (
            <p className="text-sm text-muted-foreground">{t.notDeployed(TRY_RUN_AGENT)}</p>
          ) : null}
          {needsRouter ? (
            <p className="text-sm font-medium">{t.needsRouter}</p>
          ) : ranBaseline && !done.includes('optimized') && !active ? (
            <p className="text-sm font-medium">{t.nextStep}</p>
          ) : null}
          <div className="flex flex-wrap gap-2">
            {deploying ? (
              <Button className={`justify-start ${TOUCH}`} disabled aria-busy>
                <Loader2 className="animate-spin motion-reduce:animate-none" aria-hidden />
                {t.deploying}
              </Button>
            ) : needsRouter ? (
              <Link to="/router" {...asButton({ className: `justify-start ${TOUCH}` })}>
                <Waypoints aria-hidden />
                {t.setUpRouter}
              </Link>
            ) : (
              <RunButton
                kind={next}
                phase={phaseOf(next)}
                disabled={!agentId || !!active || (next === 'optimized' && configs.isPending)}
                onRun={() => void run(next)}
              />
            )}
          </div>
          <div aria-live="polite" className="flex flex-col gap-3 empty:hidden">
            {failure ? (
              <div className="flex flex-wrap items-center gap-2 text-sm text-destructive">
                <span>{t.failed(failure)}</span>
                <Button size="sm" variant="outline" onClick={() => void run(next)}>
                  <RotateCw className="size-3.5" aria-hidden /> {t.retry}
                </Button>
              </div>
            ) : null}
            {shown.map((r) => (
              <Result key={r.kind} kind={r.kind} figures={figures} sessionId={r.sessionId} />
            ))}
          </div>
        </div>
      </Card>
    </div>
  )
}
