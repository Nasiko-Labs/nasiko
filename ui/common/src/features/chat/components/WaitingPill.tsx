/** The Waiting mark (v1c DS2, DS9): an amber count pill, never colour alone (the count is its text). */
import { cn } from '@/lib/utils'

export function WaitingPill({ n, className }: { n: number; className?: string }) {
  return (
    <span
      data-mark="waiting"
      className={cn(
        'inline-flex h-4 min-w-4 items-center justify-center rounded-full bg-warning/15 px-1 text-3xs font-semibold text-warning tabular-nums',
        className,
      )}
    >
      {n}
    </span>
  )
}
