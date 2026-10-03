/**
 * The headline band (plans/feat-overview.md §10; design review 3A, 10A): a plain sentence, no icon and no status chip,
 * numbers standing out by weight, never colour. Below 640 px of content only the attention sentence shows (the money
 * one when nothing needs attention), so a phone opens on what needs you. Not a live region (design review 14A).
 */
import { Fragment } from 'react'
import type { OverviewNarrative } from '@/features/narrative/overview'
import { cn } from '@/lib/utils'
import { copy } from '../copy'

/** Money amounts, ranges and counts in semibold tabular figures. */
const NUMBER = /(\$[\d,.]+(?:[kKmM])?(?:–\$[\d,.]+(?:[kKmM])?)?|\b\d[\d,]*%?)/

function emphasize(text: string) {
  // Keyed by offset and text: split alternates text and numbers, so no two parts share both.
  let at = 0
  return text.split(NUMBER).map((part, k) => {
    const key = `${at}:${part}`
    at += part.length
    return k % 2 ? (
      <span key={key} className="font-semibold tabular-nums">
        {part}
      </span>
    ) : (
      <Fragment key={key}>{part}</Fragment>
    )
  })
}

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
