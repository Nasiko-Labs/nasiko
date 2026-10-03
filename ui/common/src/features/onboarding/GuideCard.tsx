/**
 * The Overview's Setup guide for returning users (spec §4): the header button and the empty-fleet card's body. Both
 * reopen the guide at the first step not done yet.
 */
import { BookOpen, Check } from 'lucide-react'
import { Button } from '@/components/ui/button'
import { useAgentsDirectory } from '@/features/agents/api'
import { useConfigs } from '@/features/router/api'
import { cn } from '@/lib/utils'
import { openGuide, useGuide } from './api'
import { copy } from './copy'
import { firstOpenStep, ticks as tick, type Ticks } from './logic'

function useTicks(): Ticks {
  const { persona } = useGuide()
  const configs = useConfigs().data?.length ?? 0
  const agents = useAgentsDirectory().data?.length ?? 0
  return tick({ persona, configs, agents })
}

export function SetupGuideButton({ className }: { className?: string }) {
  const t = useTicks()
  return (
    <Button
      variant="outline"
      size="sm"
      className={className}
      onClick={() => openGuide(firstOpenStep(t))}
    >
      <BookOpen aria-hidden /> {copy.card.title}
    </Button>
  )
}

const ROWS = [
  ['role', copy.steps.role],
  ['model', copy.steps.model],
  ['agent', copy.steps.agent],
] as const

export function GuideSteps({ buttonClassName }: { buttonClassName?: string }) {
  const t = useTicks()
  // "Resume" only once a step is done: before that there is nothing to resume.
  const started = t.role || t.model || t.agent
  return (
    <div className="flex flex-col gap-3">
      <p className="text-sm text-muted-foreground">{copy.card.intro}</p>
      {/* Each row opens the guide at its own step; the button below opens it at the first one not done. */}
      <ul className="-mx-2 flex max-w-xl flex-col">
        {ROWS.map(([id, s]) => (
          <li key={id}>
            <Button
              variant="ghost"
              className={cn(
                'h-auto w-full justify-start gap-3 px-2 py-1 text-left font-normal whitespace-normal',
                buttonClassName,
              )}
              onClick={() => openGuide(id)}
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
              <span className="text-xs text-muted-foreground">
                {t[id] ? copy.card.done : copy.card.todo}
              </span>
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
