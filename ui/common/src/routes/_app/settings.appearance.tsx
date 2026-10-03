import { createFileRoute } from '@tanstack/react-router'
import { AppearancePage } from '@/features/settings/AppearancePage'
import { SettingsLayout } from '@/features/settings/SettingsLayout'

// Every user's own preference: no superuser gate, unlike the workspace sections.
export const Route = createFileRoute('/_app/settings/appearance')({ component: AppearanceRoute })

function AppearanceRoute() {
  return (
    <SettingsLayout>
      <AppearancePage />
    </SettingsLayout>
  )
}
