import type { QueryClient } from '@tanstack/react-query'
import { createRootRouteWithContext, Outlet, useRouterState } from '@tanstack/react-router'
import { deferred } from '@/app/deferred'

// The root route isn't code-split: its not-found page and the presenter card load on demand, not with every page.
const NotFound = deferred(() => import('@/app/shell/NotFound').then((m) => m.NotFound))
const PresenterCard = deferred(() =>
  import('@/app/shell/PresenterCard').then((m) => m.PresenterCard),
)

export const Route = createRootRouteWithContext<{ queryClient: QueryClient }>()({
  component: RootLayout,
  // Unknown URLs render outside the shell, so give them a way back in (review: red team).
  notFoundComponent: () => <NotFound />,
})

/**
 * The navigation lives in the signed-in shell (`_app` → `AppShell`, plans/feat-app-shell.md), so
 * /login renders bare. The mode badge (MOCK DATA / LIVE) moved to the sidebar's status row.
 */
function RootLayout() {
  // Presenter mode is `?demo=1` (PresenterCard checks the value and the step itself).
  const demo = useRouterState({ select: (st) => 'demo' in st.location.search })
  return (
    <>
      <Outlet />
      {demo ? <PresenterCard /> : null}
    </>
  )
}
