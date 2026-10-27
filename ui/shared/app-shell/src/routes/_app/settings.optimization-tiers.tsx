import { createFileRoute, redirect } from '@tanstack/react-router'
import { TiersPage } from '@nasiko/features/optimization/TiersPage'
import { SettingsLayout } from '@nasiko/features/settings/SettingsLayout'
import { meQuery } from '@nasiko/data-access/auth'

// A workspace page (plans/feat-context-optimization.md §4): superuser only, like the other workspace sections;
// everyone else sees what their tier does in Your settings on /optimization (plans/feat-optimization-page.md P2).
export const Route = createFileRoute('/_app/settings/optimization-tiers')({
  beforeLoad: async ({ context }) => {
    const me = await context.queryClient.ensureQueryData(meQuery)
    if (!me.is_superuser) throw redirect({ to: '/optimization', hash: 'settings', replace: true })
  },
  component: TiersRoute,
})

function TiersRoute() {
  return (
    <SettingsLayout>
      <TiersPage />
    </SettingsLayout>
  )
}
