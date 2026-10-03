/**
 * The Build page's one progress indicator (design review 14: stock shadcn `Progress`, four labelled segments; 13: the
 * only motion is a segment filling on the `standard` preset; 16: on phones only the current stage's label shows).
 */
import { Progress } from '@/components/ui/progress'
import { cn } from '@/lib/utils'
import { copy } from '../copy'
import type { Stage } from '../steps'

const SR_STATE: Record<Stage['state'], string> = {
  done: copy.build.done,
  current: copy.build.inProgress,
  pending: copy.build.pending,
  failed: copy.status.failed,
  warning: copy.status.notRunning,
}
const FILL: Record<Stage['state'], number> = {
  done: 100,
  current: 50,
  pending: 0,
  failed: 100,
  warning: 100,
}
const TONE: Record<Stage['state'], string> = {
  done: '',
  current: '[&>[data-slot=progress-indicator]]:motion-safe:animate-pulse',
  pending: '',
  failed: '[&>[data-slot=progress-indicator]]:bg-destructive',
  warning: '[&>[data-slot=progress-indicator]]:bg-warning',
}

export function ProgressSegments({
  stages,
  sub,
}: {
  stages: readonly Stage[]
  sub: (s: Stage) => string | null
}) {
  const currentIndex = stages.findIndex(
    (s) => s.state === 'current' || s.state === 'failed' || s.state === 'warning',
  )
  const current = stages[currentIndex] ?? null
  return (
    <div className="@container/steps flex flex-col gap-2">
      <ol
        aria-label={copy.build.stepsLabel}
        className="grid gap-2"
        style={{ gridTemplateColumns: `repeat(${stages.length}, minmax(0, 1fr))` }}
      >
        {stages.map((s) => (
          <li
            key={s.id}
            aria-current={s.state === 'current' ? 'step' : undefined}
            className="flex min-w-0 flex-col gap-2"
            data-state={s.state}
          >
            <Progress
              value={FILL[s.state]}
              aria-hidden
              className={cn(
                'h-1.5 bg-muted transition-none [&>[data-slot=progress-indicator]]:duration-200 [&>[data-slot=progress-indicator]]:ease-out [&>[data-slot=progress-indicator]]:motion-reduce:transition-none',
                TONE[s.state],
              )}
            />
            <div className="hidden min-w-0 @[520px]/steps:block">
              <p
                className={cn(
                  'truncate text-sm',
                  s.state === 'pending' ? 'text-muted-foreground' : 'font-medium',
                  s.state === 'failed' && 'text-destructive',
                )}
              >
                {s.label}
              </p>
              {/* A failed or not-running stage keeps its name; the state is the sub-line, in words. */}
              <p
                className={cn(
                  'truncate text-xs tabular-nums',
                  s.state === 'failed'
                    ? 'text-destructive'
                    : s.state === 'warning'
                      ? 'text-warning'
                      : 'text-muted-foreground',
                )}
              >
                {s.state === 'failed'
                  ? copy.status.failed
                  : s.state === 'warning'
                    ? copy.status.notRunning
                    : sub(s)}
              </p>
            </div>
            <span className="sr-only">{`${s.label}: ${SR_STATE[s.state]}`}</span>
          </li>
        ))}
      </ol>
      {/* Phones: one line for the current stage (design review 16). */}
      {current ? (
        <p className="text-xs text-muted-foreground tabular-nums @[520px]/steps:hidden" aria-hidden>
          {current.label} · {copy.build.stepOf(currentIndex + 1, stages.length)}
          {sub(current) ? ` · ${sub(current)}` : ''}
        </p>
      ) : null}
    </div>
  )
}
