/**
 * The router's default pending and error UI (plan §8 Phase 3). A route that throws while rendering or loading
 * shows this instead of TanStack's bare error; a 403 reads as "No access", never as a crash. A route whose code
 * chunk is gone (a tab older than the running build) gets the stale-build recovery instead (lib/staleBuild.ts).
 */
import { useQueryErrorResetBoundary } from '@tanstack/react-query'
import { useRouter, type ErrorComponentProps } from '@tanstack/react-router'
import { useEffect } from 'react'
import { PageLoader } from '@nasiko/ui/composites/page-loader'
import { StateCard, type StateTone } from '@nasiko/ui/composites/state-card'
import { Button } from '@nasiko/ui/primitives/button'
import { ApiError } from '@nasiko/data-access/client'
import {
  isChunkLoadError,
  recoverFromChunkError,
  useStaleRecovery,
  type StaleRecovery,
} from '@nasiko/app-core/staleBuild'
import { copy } from '@nasiko/app-core/shell/copy'

const STALE_TONE: Record<StaleRecovery, StateTone> = {
  checking: 'info',
  reloading: 'info',
  held: 'warning',
  exhausted: 'warning',
  offline: 'warning',
  network: 'error',
  failed: 'error',
}

/** The button each outcome offers. Both reload: React and the browser keep a failed import, so only a reload retries it. */
const STALE_ACTION: Record<StaleRecovery, 'reload' | 'tryAgain' | null> = {
  checking: null,
  reloading: null,
  held: 'reload',
  exhausted: 'reload',
  offline: null,
  network: 'tryAgain',
  failed: 'tryAgain',
}

export function RouteError({ error, reset }: ErrorComponentProps) {
  const router = useRouter()
  const queryReset = useQueryErrorResetBoundary()
  const chunk = isChunkLoadError(error)
  const recovery = useStaleRecovery() ?? 'checking'
  // A query error thrown to the boundary stays thrown until its boundary resets.
  useEffect(() => queryReset.reset(), [queryReset])
  useEffect(() => {
    if (chunk) void recoverFromChunkError()
  }, [error, chunk])
  if (chunk) {
    const action = STALE_ACTION[recovery]
    return (
      <div className="mx-auto mt-10 max-w-lg px-4" data-stale-build={recovery}>
        <StateCard
          tone={STALE_TONE[recovery]}
          title={copy.staleBuild[recovery].title}
          fix={copy.staleBuild[recovery].body}
          action={
            action ? (
              <Button
                size="sm"
                variant="outline"
                onClick={() => {
                  window.location.reload()
                }}
              >
                {copy.staleBuild[action]}
              </Button>
            ) : null
          }
        />
      </div>
    )
  }
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
