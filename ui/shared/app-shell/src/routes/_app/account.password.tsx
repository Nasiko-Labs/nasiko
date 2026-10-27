import { createFileRoute } from '@tanstack/react-router'
import { PasswordPage } from '@nasiko/features/settings/PasswordPage'
import { SettingsLayout } from '@nasiko/features/settings/SettingsLayout'

// Every user's own account: no superuser gate, unlike the workspace sections.
export const Route = createFileRoute('/_app/account/password')({ component: PasswordRoute })

function PasswordRoute() {
  return (
    <SettingsLayout account>
      <PasswordPage />
    </SettingsLayout>
  )
}
