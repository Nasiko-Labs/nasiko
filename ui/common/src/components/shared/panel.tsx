/**
 * Panel chrome and in-panel states on shadcn `Card`, `Alert`, `Empty` and `Skeleton` (TokenOps plan
 * A2b, shared since plan §8 Phase 1): every panel owns its skeleton, error card (with the request path
 * and Retry) and empty state, so one failed query never blanks the page.
 */
import { AlertTriangle, Lock, RotateCw } from 'lucide-react'
import type { ReactNode, Ref } from 'react'
import { Alert, AlertDescription, AlertTitle } from '@/components/ui/alert'
import { Button } from '@/components/ui/button'
import { Card, CardAction, CardDescription, CardHeader } from '@/components/ui/card'
import { Skeleton } from '@/components/ui/skeleton'
import { ApiError } from '@/lib/api/client'
import { cn } from '@/lib/utils'
import { EmptyState } from './state-card'

export function Panel({
  title,
  subtitle,
  actions,
  children,
  labelledBy,
  focusableTitle,
  className,
  ref,
  titleRef,
}: {
  title: ReactNode
  subtitle?: ReactNode
  actions?: ReactNode
  children: ReactNode
  /** The heading's id; names the section. */
  labelledBy?: string
  /** Lets code move focus to the heading (tabIndex -1) when the panel opens. */
  focusableTitle?: boolean
  className?: string
  ref?: Ref<HTMLElement>
  titleRef?: Ref<HTMLHeadingElement>
}) {
  return (
    <Card asChild className={cn('min-w-0 gap-3 p-4', className)}>
      <section ref={ref} aria-labelledby={labelledBy}>
        <CardHeader className="flex flex-wrap items-start justify-between gap-2 px-0">
          <div className="min-w-0">
            <h2
              ref={titleRef}
              id={labelledBy}
              tabIndex={focusableTitle ? -1 : undefined}
              className="text-sm font-semibold outline-none"
            >
              {title}
            </h2>
            {subtitle ? <CardDescription className="text-xs">{subtitle}</CardDescription> : null}
          </div>
          {actions ? (
            <CardAction className="col-auto row-auto flex flex-wrap items-center gap-2">
              {actions}
            </CardAction>
          ) : null}
        </CardHeader>
        {children}
      </section>
    </Card>
  )
}

export function PanelSkeleton({ height = 180 }: { height?: number }) {
  return (
    // Decorative: not a live region (a page of panels would announce "Loading" once per panel). The page marks its
    // region aria-busy while it loads (plan §8 Phase 9).
    <Skeleton aria-hidden className="w-full rounded-md" style={{ height }} />
  )
}

export function PanelError({
  error,
  onRetry,
  what,
}: {
  error: unknown
  onRetry: () => void
  what: string
}) {
  const api = error instanceof ApiError ? error : null
  // A 403 is an answer, not a failure: nothing to retry until someone grants access.
  if (api?.isForbidden) {
    return (
      <Alert className="gap-y-2 [&>svg]:text-warning">
        <Lock aria-hidden />
        <AlertTitle className="line-clamp-none">No access to {what}</AlertTitle>
        <AlertDescription className="text-xs">
          {api.serverMessage ? `The server said: ${api.serverMessage}. ` : null}Ask an admin or the
          owner for access.
        </AlertDescription>
      </Alert>
    )
  }
  return (
    <Alert className="gap-y-2 border-destructive/30 bg-destructive/5 [&>svg]:text-destructive">
      <AlertTriangle aria-hidden />
      <AlertTitle className="line-clamp-none text-destructive">Couldn't load {what}</AlertTitle>
      <AlertDescription className="gap-2">
        <p className="text-xs">
          {api?.serverMessage ?? (api ? `Request failed (${api.status}).` : 'Unexpected error.')}
          {api ? (
            <>
              {' '}
              <code className="break-all">{api.path}</code>
            </>
          ) : null}
        </p>
        <Button size="sm" variant="outline" onClick={onRetry}>
          <RotateCw className="size-3.5" aria-hidden /> Retry
        </Button>
      </AlertDescription>
    </Alert>
  )
}

export function PanelEmpty({ title, children }: { title: string; children?: ReactNode }) {
  return <EmptyState title={title}>{children}</EmptyState>
}
