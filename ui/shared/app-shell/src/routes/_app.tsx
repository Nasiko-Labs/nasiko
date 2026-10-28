import { skipToken, useQuery } from '@tanstack/react-query'
import { createFileRoute, redirect, useRouterState } from '@tanstack/react-router'
import { useSyncExternalStore } from 'react'
import { useSlots } from '@nasiko/app-core/edition/edition-context'
import { AppShell } from '@nasiko/app-shell/shell/AppShell'
import { BuildToasts } from '@nasiko/features/deploy/components/BuildToasts'
import { copy as deployCopy } from '@nasiko/features/deploy/copy'
import { followedCount, subscribeFollower } from '@nasiko/features/deploy/follower'
import { useGuideHiddenNav } from '@nasiko/features/onboarding/api'
import { TourHost } from '@nasiko/features/tour/TourHost'
import { meQuery } from '@nasiko/data-access/auth'
import { TooltipProvider } from '@nasiko/ui/primitives/tooltip'
import { ApiError } from '@nasiko/data-access/client'
import { isSignedOutLocally } from '@nasiko/app-core/session'

/**
 * Auth guard for everything under `_app/` (plan A17, A18). `/api/me` answers first; a
 * 401 sends the user to /login with an allow-listed relative redirect. Other errors
 * (e.g. server down) fall through so the page can render its own message, and the sidebar's
 * account row says "Account unavailable" (plans/feat-app-shell.md eng D7).
 */
export const Route = createFileRoute('/_app')({
  beforeLoad: async ({ context, location }) => {
    // After a failed logout the cookie may still work: this browser stays signed out (review D1).
    if (isSignedOutLocally()) {
      throw redirect({
        to: '/login',
        search: { redirect: location.href, expired: undefined, signout: 'failed' },
      })
    }
    try {
      await context.queryClient.ensureQueryData(meQuery)
    } catch (err) {
      if (err instanceof ApiError && err.status === 401) {
        throw redirect({
          to: '/login',
          search: { redirect: location.href, expired: undefined, signout: undefined },
        })
      }
    }
  },
  component: AppLayout,
})

const NONE: readonly string[] = []
/** The shell plus the feature pieces it shows on every page (the shell itself never imports a feature). */
function AppLayout() {
  const building = useSyncExternalStore(subscribeFollower, followedCount)
  const guideHidden = useGuideHiddenNav()
  // A layer's own hidden items (EE: Access control for members).
  const slots = useSlots()
  const { navHidden, shellEnd: ShellEnd, shellSplit } = slots
  const layer = useQuery({ queryKey: ['nav-hidden'], queryFn: skipToken, ...navHidden?.query })
  const layerHidden = navHidden ? navHidden.hidden(layer.data) : NONE
  const path = useRouterState({ select: (s) => s.location.pathname })
  // Budgets are Enterprise only: an OSS build has no item to hide, and EE hides its own while the probe is out.
  const budgetsHidden = slots.budgets.useNavHidden()
  return (
    // Here, not in mount.tsx: this layout is its own chunk, so the tooltip code stays out of the shell.
    <TooltipProvider>
      <AppShell
        split={shellSplit}
        end={
          <>
            <BuildToasts />
            <TourHost />
            {/* A layer's chrome beside every page (EE: the Weave dock). */}
            {ShellEnd ? <ShellEnd path={path} /> : null}
          </>
        }
        hidden={[...guideHidden, ...layerHidden, ...budgetsHidden]}
        badges={
          building
            ? { '/agents': { count: building, label: deployCopy.toast.agentsBadge(building) } }
            : undefined
        }
      />
    </TooltipProvider>
  )
}
