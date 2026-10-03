/** A step's status by icon and in words (plan §7.4): running, failed, done, or no result (v1c recorded calls). */
import { Check, Loader2, Minus, X } from 'lucide-react'
import { copy } from '../copy'

export function StepIcon({ status }: { status: 'running' | 'ok' | 'error' | 'neutral' }) {
  if (status === 'running')
    return (
      <Loader2
        className="size-3.5 animate-spin motion-reduce:animate-none"
        aria-label={copy.stepRunning}
      />
    )
  if (status === 'error')
    return <X className="size-3.5 text-destructive" aria-label={copy.stepFailed} />
  if (status === 'neutral')
    return <Minus className="size-3.5 text-muted-foreground" aria-label={copy.stepNoResult} />
  return <Check className="size-3.5 text-success" aria-label={copy.stepDone} />
}
