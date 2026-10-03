/**
 * Marks a feature that is new and still being validated.
 *
 * Used on the token-optimisation switches and on the savings figures they produce. These change
 * what a model receives and what we claim they saved, so someone deciding whether to switch one on
 * deserves to know the behaviour is young — and that the answer is reversible.
 *
 * One component rather than a sentence written three times: a disclaimer that is worded slightly
 * differently in each place reads as three different degrees of caution, when the intent is one.
 *
 * Deliberately a quiet badge and a short line, not a warning banner. These features are safe — the
 * worst case is a smaller saving than expected, not a broken agent — and dressing that up as a
 * hazard would stop people trying the thing we want them to try.
 */
import { Badge } from '@/components/ui/badge'
import { Tooltip, TooltipContent, TooltipTrigger } from '@/components/ui/tooltip'
import { cn } from '@/lib/utils'

export const BETA_EXPLAINER =
  'This is new and still being tuned. It is safe to turn on or off at any time, and takes effect on the next message — but the savings may be smaller than the figures suggest while we validate them.'

/** Pill for a section header. Pair with {@link BetaLine} where there is room for a sentence. */
export function BetaBadge({ className }: { className?: string }) {
  return (
    <Tooltip>
      <TooltipTrigger asChild>
        <Badge
          variant="outline"
          className={cn('border-dashed text-muted-foreground', className)}
          // The tooltip carries the detail; the badge alone must still say what it means to a
          // screen reader that never sees a hover.
          aria-label={`Beta. ${BETA_EXPLAINER}`}
        >
          Beta
        </Badge>
      </TooltipTrigger>
      <TooltipContent className="max-w-xs">{BETA_EXPLAINER}</TooltipContent>
    </Tooltip>
  )
}

/** The same caveat as a sentence, for places with no header to hang a badge on. */
export function BetaLine({ className }: { className?: string }) {
  return <p className={cn('text-xs text-muted-foreground', className)}>{BETA_EXPLAINER}</p>
}
