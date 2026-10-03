/**
 * One Overview card (design review 11A, 14A) on shadcn `Card`: an h2 title that Retry can focus, and a link to the
 * page that owns the card. Each card loads and fails on its own. The chrome mirrors the shared `Panel`; this one adds
 * the owning-page link, the header aside and a per-card container.
 */
import { Link } from '@tanstack/react-router'
import { AlertTriangle, RotateCw } from 'lucide-react'
import type { ReactNode, RefObject } from 'react'
import { Alert, AlertDescription, AlertTitle } from '@/components/ui/alert'
import { Button } from '@/components/ui/button'
import { Card as UiCard, CardAction, CardContent, CardHeader } from '@/components/ui/card'
import { Skeleton } from '@/components/ui/skeleton'
import { cn } from '@/lib/utils'
import { copy } from '../copy'

/** A touch-sized target on coarse pointers (/ship review). */
export const TOUCH = 'pointer-coarse:min-h-11'

export function Card({
  id,
  title,
  to,
  linkLabel,
  linkSearch,
  linkHash,
  meta,
  aside,
  titleRef,
  titleClassName,
  className,
  children,
}: {
  id: string
  title: string
  /** The owning page ("TokenOps →"). */
  to?: '/tokenops' | '/agents' | '/harnesses' | '/sessions' | '/chat' | '/router'
  linkLabel?: string
  /** Search params for the owning-page link (Sessions opens on the card's own window). */
  linkSearch?: Record<string, unknown>
  /** A section of the owning page (the router's budgets). */
  linkHash?: string
  meta?: ReactNode
  /** Shown where the page link would be (Needs you's "Checked …"). */
  aside?: ReactNode
  titleRef?: RefObject<HTMLHeadingElement | null>
  /** A louder title for the card that leads the page (Needs you). */
  titleClassName?: string
  className?: string
  children: ReactNode
}) {
  return (
    // A container per card, so rows can adapt to the card's own width (/ship review).
    <UiCard asChild className={cn('@container/card min-w-0 gap-3 p-4', className)}>
      <section aria-labelledby={id} data-testid={id}>
        <CardHeader className="flex min-h-6 items-center justify-between gap-2 px-0">
          {/* The meta wraps under the title in a narrow column rather than squeezing it. */}
          <div className="flex min-w-0 flex-wrap items-baseline gap-x-2">
            <h2
              id={id}
              ref={titleRef}
              tabIndex={-1}
              className={cn(
                'shrink-0 rounded-sm text-sm font-semibold whitespace-nowrap outline-none focus-visible:ring-2 focus-visible:ring-ring focus-visible:ring-offset-2 focus-visible:ring-offset-background',
                titleClassName,
              )}
            >
              {title}
            </h2>
            {meta ? <span className="truncate text-xs text-muted-foreground">{meta}</span> : null}
          </div>
          {to && linkLabel ? (
            <CardAction className="col-auto row-auto shrink-0 self-center">
              <Button asChild variant="link" size="sm" className={cn('h-auto px-0 text-xs', TOUCH)}>
                <Link to={to} search={linkSearch as never} hash={linkHash}>
                  {linkLabel} <span aria-hidden>→</span>
                </Link>
              </Button>
            </CardAction>
          ) : aside ? (
            <CardAction className="col-auto row-auto shrink-0 self-center text-xs text-muted-foreground tabular-nums">
              {aside}
            </CardAction>
          ) : null}
        </CardHeader>
        <CardContent className="flex min-h-0 flex-1 flex-col px-0">{children}</CardContent>
      </section>
    </UiCard>
  )
}

export function CardSkeleton({ rows = 3 }: { rows?: number }) {
  return (
    <div className="flex flex-col gap-2" aria-busy="true">
      <span className="sr-only">{copy.loading}</span>
      {Array.from({ length: rows }, (_, i) => (
        <Skeleton key={i} className="h-8 w-full motion-reduce:animate-none" />
      ))}
    </div>
  )
}

/** A whole card's failure: what failed, and Retry. Retry keeps focus on the card title (design review 14A). */
export function CardError({
  what,
  onRetry,
  titleRef,
}: {
  what: string
  onRetry: () => void
  titleRef?: RefObject<HTMLHeadingElement | null>
}) {
  return (
    // Not a live region: when the server is down every card fails at once (/ship review of the shadcn conversion).
    <Alert
      role={undefined}
      className="gap-y-2 border-destructive/30 bg-destructive/5 [&>svg]:text-destructive"
    >
      <AlertTriangle aria-hidden />
      <AlertTitle className="line-clamp-none font-normal">{copy.couldntLoad(what)}</AlertTitle>
      <AlertDescription>
        <Button
          size="sm"
          variant="outline"
          className={TOUCH}
          onClick={() => {
            onRetry()
            titleRef?.current?.focus()
          }}
        >
          <RotateCw className="size-3.5" aria-hidden /> {copy.retry}
        </Button>
      </AlertDescription>
    </Alert>
  )
}

/** One failed source inside a card that still shows the rest (design review 5A). */
export function SourceFailed({ what, onRetry }: { what: string; onRetry: () => void }) {
  return (
    <div className="flex items-center gap-2 text-xs text-muted-foreground">
      <AlertTriangle className="size-3.5 shrink-0 text-warning" aria-hidden />
      <span>{copy.couldntCheck(what)}</span>
      <Button
        variant="link"
        size="sm"
        className={cn('h-auto px-0 text-xs', TOUCH)}
        onClick={onRetry}
      >
        {copy.retry}
      </Button>
    </div>
  )
}
