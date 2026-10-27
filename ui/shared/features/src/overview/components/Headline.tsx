/**
 * The headline band (plans/feat-overview.md §10; design review 3A, 10A): a plain sentence, no icon and no status chip,
 * numbers standing out by weight, never colour. Below 640 px of content only the attention sentence shows (the money
 * one when nothing needs attention), so a phone opens on what needs you. Not a live region (design review 14A).
 */
import type { OverviewNarrative } from '@nasiko/features/narrative/overview'
import { cn } from '@nasiko/ui/lib/utils'
import { copy } from '../copy'
import { emphasize } from '../emphasize'

export function Headline({
  narrative,
  nothing,
}: {
  narrative: OverviewNarrative
  nothing: boolean
}) {
  const { money, attention } = narrative
  // On a phone the attention sentence wins; when it only says "nothing", the money sentence shows instead.
  const phoneShowsMoney = !attention || nothing
  return (
    <p
      className="max-w-[62ch] text-xl leading-8 text-pretty text-foreground"
      data-testid="overview-headline"
    >
      {!money && !attention ? (
        <span className="text-muted-foreground">{copy.headline.checking}</span>
      ) : null}
      {money ? (
        <span
          className={cn(!phoneShowsMoney && '@max-[640px]/overview:hidden')}
          data-testid="headline-money"
        >
          {emphasize(money)}{' '}
        </span>
      ) : null}
      {attention ? (
        <span
          className={cn(phoneShowsMoney && money && '@max-[640px]/overview:hidden')}
          data-testid="headline-attention"
        >
          {emphasize(attention)}
        </span>
      ) : null}
    </p>
  )
}
