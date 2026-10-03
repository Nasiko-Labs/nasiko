import { RotateCw } from 'lucide-react'
import { Button } from '@/components/ui/button'
import { copy } from '../copy'
import { routerError } from '../errors'

/** A section's failed read: problem, cause, action and Retry (plan §4.8, §4.9). */
export function SectionError({
  error,
  onRetry,
  title,
}: {
  error: unknown
  onRetry: () => void
  title?: string
}) {
  const v = routerError(error)
  return (
    <div className="flex flex-wrap items-start justify-between gap-3 rounded-lg border border-border bg-card p-4 text-sm">
      <div className="space-y-0.5">
        <p className="font-medium text-destructive">{title ?? v.problem}</p>
        {title ? <p className="text-muted-foreground">{v.problem}</p> : null}
        {v.cause ? <p className="text-muted-foreground">{v.cause}</p> : null}
        {v.action ? <p className="text-muted-foreground">{v.action}</p> : null}
      </div>
      <Button size="sm" variant="outline" onClick={onRetry} className="pointer-coarse:min-h-11">
        <RotateCw className="size-3.5" aria-hidden /> {copy.retry}
      </Button>
    </div>
  )
}
