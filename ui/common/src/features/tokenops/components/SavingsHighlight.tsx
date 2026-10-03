/**
 * The savings headline, always visible.
 *
 * Optimisation used to live only inside a collapsed section near the bottom of the page: you had to
 * scroll past four panels and then expand one to learn that the platform had saved you anything.
 * That is the wrong weight for the one number on this page that is good news, so the headline sits
 * directly under the month hero and the detail stays in the section below.
 *
 * Deliberately two figures, not four. The full four-up breakdown is thirty pixels further down; a
 * banner that repeats it is just noise above the thing it duplicates.
 */
import { TrendingDown } from 'lucide-react'
import { Button } from '@/components/ui/button'
import { fmtMoney, fmtPct, fmtTokens } from '@/lib/format'
import type { OptimisationView } from '../optimisation'
import { isUnconfigured } from '../optimisation'

export function SavingsHighlight({
  data,
  onSeeDetail,
}: {
  data: OptimisationView | undefined
  /** Opens (and scrolls to) the full section. */
  onSeeDetail: () => void
}) {
  // Nothing on: the nudge belongs in the section's own empty state, not as a banner claiming a
  // saving of zero. A page that opens with "£0 saved" reads as a broken feature.
  if (!data || isUnconfigured(data)) return null

  const pct = data.savedPct == null ? null : fmtPct(data.savedPct)
  return (
    <section
      aria-labelledby="savings-highlight-title"
      className="flex flex-wrap items-center justify-between gap-x-6 gap-y-3 rounded-lg border border-success/30 bg-success/5 px-4 py-3"
    >
      <div className="flex min-w-0 items-center gap-3">
        <span
          className="flex size-9 shrink-0 items-center justify-center rounded-full bg-success/15 text-success"
          aria-hidden
        >
          <TrendingDown className="size-5" />
        </span>
        <div className="min-w-0">
          <h3 id="savings-highlight-title" className="text-sm font-medium">
            Optimisation saved you {fmtMoney(data.costSaved)}
            {pct ? <> · {pct} fewer tokens</> : null}
          </h3>
          <p className="text-xs text-muted-foreground">
            {fmtTokens(data.tokensSaved)} tokens never sent
            {data.topCategory ? <> — mostly {data.topCategory.label.toLowerCase()}</> : null}.
            Across {data.optimisedCount} of {data.totalAgents} agents.
          </p>
        </div>
      </div>
      <Button variant="outline" size="sm" onClick={onSeeDetail}>
        See the breakdown <span aria-hidden>↓</span>
      </Button>
    </section>
  )
}
