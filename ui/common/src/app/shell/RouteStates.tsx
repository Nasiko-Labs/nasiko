/**
 * The router's default pending and error UI (plan §8 Phase 3). A route that throws while rendering or loading
 * shows this instead of TanStack's bare error; a 403 reads as "No access", never as a crash.
 */
import { useQueryErrorResetBoundary } from '@tanstack/react-query'
import { useRouter, type ErrorComponentProps } from '@tanstack/react-router'
import { useEffect } from 'react'
import { PageLoader } from '@/components/shared/page-loader'
import { StateCard } from '@/components/shared/state-card'
import { Button } from '@/components/ui/button'
import { ApiError } from '@/lib/api/client'
import { copy } from './copy'

export function RouteError({ error, reset }: ErrorComponentProps) {
  const router = useRouter()
  const queryReset = useQueryErrorResetBoundary()
  // A query error thrown to the boundary stays thrown until its boundary resets.
  useEffect(() => queryReset.reset(), [queryReset])
  if (error instanceof ApiError && error.isForbidden) {
    return (
      <div className="mx-auto mt-10 max-w-lg px-4">
        <StateCard
          tone="warning"
          title={copy.routeError.noAccess}
          fix={copy.routeError.noAccessBody}
        />
      </div>
    )
  }
  const retry = () => {
    reset()
    void router.invalidate()
  }
  return (
    <div className="mx-auto mt-10 max-w-lg px-4">
      <StateCard
        tone="error"
        title={copy.routeError.title}
        fix={copy.routeError.body}
        action={
          <Button size="sm" variant="outline" onClick={retry}>
            {copy.routeError.tryAgain}
          </Button>
        }
      >
        {import.meta.env.DEV && error instanceof Error ? (
          <code className="text-xs break-all">{error.message}</code>
        ) : null}
      </StateCard>
    </div>
  )
}

export function RoutePending() {
  return <PageLoader label={copy.routeError.loading} />
}
