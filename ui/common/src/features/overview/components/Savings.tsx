/**
 * Token optimisation on the Overview: one big number and one thing to do about it.
 *
 * The last block in the summary card's left column, under the range's narrative and month bar, and
 * tinted by state. It belongs with the other figures for that range rather than floating between
 * the card and the detail grid; the tint and the headline-sized number are what keep it from
 * reading as a footnote, which is how it started.
 *
 * Deliberately not a small TokenOps. That page answers "where did every token go" with four tables;
 * this answers "is it working, and what would help" in one figure and one sentence, then hands off.
 * Repeating the breakdown here would give the reader the same work twice and a reason to skip both.
 *
 * Two states, and the second is the reason this renders at all when nothing is on:
 *
 * - **Saving** (success tint) — what it saved, and the biggest remaining win.
 * - **Not saving** (warning tint) — what is being spent that *could* be trimmed. Framed as money on
 *   the table rather than "$0 saved": the first is a reason to act, the second reads as a broken
 *   feature and teaches the opposite of the intended lesson.
 *
 * Neither renders on a workspace with no spend to talk about — there, the nudge belongs in the
 * setup guide, not as a permanent band on the busiest screen in the product.
 */
import { ArrowRight, Scissors, TrendingDown } from 'lucide-react'
import { Link } from '@tanstack/react-router'
import { Button } from '@/components/ui/button'
import type { OptimisationView } from '@/features/tokenops/optimisation'
import { fmtMoney, fmtPct, fmtTokens } from '@/lib/format'
import { cn } from '@/lib/utils'
import { copy as overviewCopy } from '../copy'

const copy = overviewCopy.savings

/**
 * The one sentence worth adding to the number.
 *
 * Ordered by what the reader can act on: an unoptimised agent costing real money beats naming the
 * layer that happened to win, because only one of the two is a next step.
 */
function insight(v: OptimisationView): string {
  const top = v.topUnoptimised
  if (top && top.spend_usd > 0) return copy.couldSave(top.agent_name, fmtMoney(top.spend_usd))
  if (v.topCategory) return copy.mostly(v.topCategory.label.toLowerCase())
  return copy.allOn
}

export function Savings({
  view,
  isPending,
}: {
  view: OptimisationView | undefined
  isPending: boolean
}) {
  if (isPending || !view) return null

  const saving = view.tokensSaved > 0
  // Nothing saved and nothing being wasted either: a brand-new workspace has no story here yet.
  if (!saving && view.unoptimisedSpend <= 0) return null

  return (
    <section
      aria-labelledby="overview-savings-title"
      data-testid="overview-savings"
      data-state={saving ? 'saving' : 'idle'}
      className={cn(
        'flex flex-wrap items-center justify-between gap-x-6 gap-y-3 rounded-lg border p-4',
        saving ? 'border-success/30 bg-success/8' : 'border-warning/30 bg-warning/8',
      )}
    >
      <div className="flex min-w-0 items-center gap-4">
        <span
          aria-hidden
          className={cn(
            'flex size-10 shrink-0 items-center justify-center rounded-full',
            saving ? 'bg-success/15 text-success' : 'bg-warning/15 text-warning',
          )}
        >
          {saving ? <TrendingDown className="size-5" /> : <Scissors className="size-5" />}
        </span>

        <div className="min-w-0">
          <p className="text-xs font-medium tracking-wide text-muted-foreground uppercase">
            {saving ? copy.eyebrow : copy.eyebrowOff}
          </p>
          <h2
            id="overview-savings-title"
            className="mt-0.5 flex flex-wrap items-baseline gap-x-2 text-2xl font-semibold tabular-nums"
          >
            {saving ? (
              <>
                {fmtMoney(view.costSaved)}
                <span className="text-sm font-normal text-muted-foreground">
                  {copy.savedSuffix}
                  {view.savedPct == null ? null : ` · ${fmtPct(view.savedPct)} fewer tokens`}
                </span>
              </>
            ) : (
              <>
                {fmtMoney(view.unoptimisedSpend)}
                <span className="text-sm font-normal text-muted-foreground">{copy.idleSuffix}</span>
              </>
            )}
          </h2>
          <p className="mt-1 max-w-prose text-sm text-muted-foreground">
            {saving ? (
              <>
                {copy.neverSent(fmtTokens(view.tokensSaved))} {insight(view)}
              </>
            ) : (
              copy.idleLine
            )}
          </p>
        </div>
      </div>

      <Button asChild variant={saving ? 'outline' : 'default'} size="sm" className="shrink-0">
        {saving ? (
          // `open=optimise` expands the section and tells TokenOps to scroll to it.
          //
          // `resetScroll={false}` is what makes that stick: the router has `scrollRestoration`
          // on, so by default it scrolls a new page to the top *after* render — undoing the
          // section scroll. TokenOps' own "See the breakdown" button never hit this because it
          // does not navigate, which is exactly why the two behaved differently.
          <Link to="/tokenops" search={{ open: 'optimise' }} resetScroll={false}>
            {copy.detail} <ArrowRight aria-hidden className="size-4" />
          </Link>
        ) : (
          <Link to="/agents">
            {copy.idleCta} <ArrowRight aria-hidden className="size-4" />
          </Link>
        )}
      </Button>
    </section>
  )
}
