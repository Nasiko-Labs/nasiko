/**
 * The Overview's Setup guide for returning users (spec §4): the header button and the empty-fleet card's body. Both
 * reopen the guide at the first step not done yet.
 */
import { BookOpen, Check, Loader2 } from 'lucide-react'
import { Badge } from '@nasiko/ui/primitives/badge'
import { Button } from '@nasiko/ui/primitives/button'
import { Progress } from '@nasiko/ui/primitives/progress'
import { useAgentsDirectory } from '@nasiko/features/agents/api'
import { useConfigs } from '@nasiko/features/router/api'
import { cn } from '@nasiko/ui/lib/utils'
import { openGuide, useSeededDeploys } from './api'
import { copy } from './copy'
import { firstOpenStep, ticks as tick, type Ticks } from './logic'

function useTicks(): Ticks {
  const configs = useConfigs().data?.length ?? 0
  const agents = useAgentsDirectory().data?.length ?? 0
  return tick({ configs, agents })
}

/** Each tick, its row's words and the step it opens: the model and the agent share the Key and agent step. */
const ROWS = [
  ['model', copy.card.model, 'start'],
  ['agent', copy.card.agent, 'start'],
] as const

const doneCount = (t: Ticks) => ROWS.filter(([id]) => t[id]).length

/** The Overview header's button, with the setup's progress beside its label (user request 2026-11-09), so the home
 *  screen shows it whether or not the fleet is empty. */
export function SetupGuideButton({ className }: { className?: string }) {
  const t = useTicks()
  const done = doneCount(t)
  return (
    <Button
      variant="outline"
      size="sm"
      className={className}
      // The bar and count are hidden from the name ("Setup guide"); the description carries them.
      title={copy.card.progress(done, ROWS.length)}
      onClick={() => openGuide(firstOpenStep(t))}
    >
      <BookOpen aria-hidden /> {copy.card.title}
      {/* Spans, not Progress: its root is a div, which a button can't hold. The label above carries the count. */}
      <span aria-hidden className="relative h-1.5 w-12 overflow-hidden rounded-full bg-primary/20">
        <span
          className="absolute inset-y-0 left-0 rounded-full bg-primary transition-[width]"
          style={{ width: `${(done / ROWS.length) * 100}%` }}
        />
      </span>
      <span aria-hidden className="text-xs text-muted-foreground tabular-nums">
        {copy.card.progressShort(done, ROWS.length)}
      </span>
    </Button>
  )
}

export function GuideSteps({ buttonClassName }: { buttonClassName?: string }) {
  const t = useTicks()
  // "Resume" only once a step is done: before that there is nothing to resume.
  const started = t.model || t.agent
  const done = doneCount(t)
  const next = ROWS.find(([id]) => !t[id])?.[0]
  // The fleet is still empty while the guide's prebuilt deploy runs, or after it didn't start: the agent row says so
  // (NAS-1450). It turns Done once the agents list has the agent.
  const deploys = [...useSeededDeploys().values()]
  const agentDeploying = deploys.some((d) => d.phase === 'deploying')
  const agentFailed = deploys.some((d) => d.phase === 'failed' || d.phase === 'notRunning')
  return (
    <div className="flex flex-col gap-3">
      <p className="text-sm text-muted-foreground">{copy.card.intro}</p>
      <div className="flex max-w-xl items-center gap-3">
        <Progress
          value={(done / ROWS.length) * 100}
          aria-label={copy.card.progress(done, ROWS.length)}
          className="h-1.5"
        />
        <span className="shrink-0 text-xs text-muted-foreground tabular-nums">
          {copy.card.progress(done, ROWS.length)}
        </span>
      </div>
      {/* Each row opens the guide at its own step; the button below opens it at the first one not done. */}
      <ul className="-mx-2 flex max-w-xl flex-col">
        {ROWS.map(([id, s, step]) => (
          <li key={id}>
            <Button
              variant="ghost"
              className={cn(
                // Top-aligned: when the title wraps, the circle and the status stay on its first line.
                'h-auto w-full items-start justify-start gap-3 px-2 py-1 text-left font-normal whitespace-normal',
                buttonClassName,
              )}
              onClick={() => openGuide(step)}
            >
              <span
                aria-hidden
                className={cn(
                  'flex size-5 shrink-0 items-center justify-center rounded-full',
                  t[id] ? 'bg-primary text-primary-foreground' : 'border',
                )}
              >
                {t[id] ? <Check className="size-3" /> : null}
              </span>
              <span className="flex-1">
                <span className="font-medium">{s.title}</span>{' '}
                <span className="text-muted-foreground">· {s.sub}</span>
              </span>
              {t[id] ? (
                // leading-5: the title's first-line height, so the smaller text centers on that line.
                <span className="shrink-0 text-xs leading-5 text-muted-foreground">
                  {copy.card.done}
                </span>
              ) : id === 'agent' && agentDeploying ? (
                <span className="flex shrink-0 items-center gap-1 text-xs leading-5 text-muted-foreground">
                  <Loader2 aria-hidden className="size-3 motion-safe:animate-spin" />
                  {copy.card.deploying}
                </span>
              ) : id === 'agent' && agentFailed ? (
                <Badge variant="destructive">{copy.card.notStarted}</Badge>
              ) : id === next ? (
                <Badge variant="info">{copy.card.next}</Badge>
              ) : null}
            </Button>
          </li>
        ))}
      </ul>
      <Button
        variant="outline"
        size="sm"
        className={cn('self-start', buttonClassName)}
        onClick={() => openGuide(firstOpenStep(t))}
      >
        {started ? copy.card.resume : copy.card.start}
      </Button>
    </div>
  )
}
