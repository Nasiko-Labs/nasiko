import { createFileRoute } from '@tanstack/react-router'
import { SettingsLayout } from '@nasiko/features/settings/SettingsLayout'
import { ShortcutsPage } from '@nasiko/features/settings/ShortcutsPage'

// Every user's own reference: no superuser gate, unlike the workspace sections.
export const Route = createFileRoute('/_app/account/shortcuts')({ component: ShortcutsRoute })

function ShortcutsRoute() {
  return (
    <SettingsLayout account>
      <ShortcutsPage />
    </SettingsLayout>
  )
}
