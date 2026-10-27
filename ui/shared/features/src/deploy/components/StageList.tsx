/**
 * A build's stages as a vertical run log: one row per stage, an icon for its state, a hairline joining the icons, and
 * a sub-line only where there is something to say (a live clock, a stage time, a failure). The same on phones.
 */
import { CircleCheck, CircleDashed, CircleX, Loader2, TriangleAlert } from 'lucide-react'
import { cn } from '@nasiko/ui/lib/utils'
import { copy } from '../copy'
import type { Stage } from '../steps'

const SR_STATE: Record<Stage['state'], string> = {
  done: copy.build.done,
  current: copy.build.inProgress,
  pending: copy.build.pending,
  failed: copy.status.failed,
  warning: copy.status.notRunning,
}

function StageIcon({ state }: { state: Stage['state'] }) {
  const cls = 'size-4 shrink-0'
  switch (state) {
    case 'done':
      return <CircleCheck className={cn(cls, 'text-success')} aria-hidden />
    case 'current':
      return <Loader2 className={cn(cls, 'text-info motion-safe:animate-spin')} aria-hidden />
    case 'failed':
      return <CircleX className={cn(cls, 'text-destructive')} aria-hidden />
    case 'warning':
      return <TriangleAlert className={cn(cls, 'text-warning')} aria-hidden />
    default:
      return <CircleDashed className={cn(cls, 'text-muted-foreground')} aria-hidden />
  }
}

export function StageList({
  stages,
  sub,
}: {
  stages: readonly Stage[]
  sub: (s: Stage) => string | null
}) {
  return (
    <ol aria-label={copy.build.stepsLabel} className="flex flex-col">
      {stages.map((s, i) => {
        // A failed or not-running stage keeps its name; the state is the sub-line, in words.
        const line =
          s.state === 'failed'
            ? copy.status.failed
            : s.state === 'warning'
              ? copy.status.notRunning
              : sub(s)
        return (
          <li
            key={s.id}
            aria-current={s.state === 'current' ? 'step' : undefined}
            data-state={s.state}
            className="relative grid grid-cols-[1rem_1fr] gap-x-3 pb-4 last:pb-0"
          >
            {i < stages.length - 1 ? (
              <span
                aria-hidden
                className={cn(
                  'absolute top-6 bottom-1 left-2 w-px -translate-x-1/2',
                  s.state === 'done' ? 'bg-success/50' : 'bg-muted-foreground/30',
                )}
              />
            ) : null}
            <span className="flex h-5 items-center">
              <StageIcon state={s.state} />
            </span>
            <div className="min-w-0">
              <p
                className={cn(
                  'text-sm leading-5',
                  s.state === 'pending' ? 'text-muted-foreground' : 'font-medium',
                )}
              >
                {s.label}
                <span className="sr-only">{`: ${SR_STATE[s.state]}`}</span>
              </p>
              {line ? (
                <p
                  className={cn(
                    'text-xs tabular-nums',
                    s.state === 'failed'
                      ? 'text-destructive'
                      : s.state === 'warning'
                        ? 'text-warning'
                        : 'text-muted-foreground',
                  )}
                >
                  {line}
                </p>
              ) : null}
            </div>
          </li>
        )
      })}
    </ol>
  )
}
