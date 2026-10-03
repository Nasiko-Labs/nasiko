import { createFileRoute } from '@tanstack/react-router'
import { PasswordPage } from '@/features/settings/PasswordPage'
import { SettingsLayout } from '@/features/settings/SettingsLayout'

// Every user's own account: no superuser gate, unlike the workspace sections.
export const Route = createFileRoute('/_app/settings/password')({ component: PasswordRoute })

function PasswordRoute() {
  return (
    <SettingsLayout>
      <PasswordPage />
    </SettingsLayout>
  )
}
