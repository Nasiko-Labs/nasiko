import { createFileRoute, redirect } from '@tanstack/react-router'
import { SettingsLayout } from '@/features/settings/SettingsLayout'
import { SettingsPage } from '@/features/settings/SettingsPage'
import { settingsSearchSchema, type SettingsSearch } from '@/features/settings/search'
import { meQuery } from '@/lib/api/auth'
import { useSetSearch } from '@/lib/search'

export const Route = createFileRoute('/_app/settings/')({
  validateSearch: settingsSearchSchema,
  // The workspace sections are superuser-gated on the API: everyone else manages their secrets (legacy settings-page.js).
  beforeLoad: async ({ context }) => {
    const me = await context.queryClient.ensureQueryData(meQuery)
    if (!me.is_superuser) throw redirect({ to: '/settings/secrets', replace: true })
  },
  component: SettingsRoute,
})

function SettingsRoute() {
  const setSearch = useSetSearch<SettingsSearch>(Route.fullPath, true)
  return (
    <SettingsLayout>
      <SettingsPage search={Route.useSearch()} setSearch={setSearch} />
    </SettingsLayout>
  )
}
