import { createFileRoute } from '@tanstack/react-router'
import { AppearancePage } from '@nasiko/features/settings/AppearancePage'
import { SettingsLayout } from '@nasiko/features/settings/SettingsLayout'

// Every user's own preference: no superuser gate, unlike the workspace sections.
export const Route = createFileRoute('/_app/account/appearance')({ component: AppearanceRoute })

function AppearanceRoute() {
  return (
    <SettingsLayout account>
      <AppearancePage />
    </SettingsLayout>
  )
}
