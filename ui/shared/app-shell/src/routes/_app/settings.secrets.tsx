import { createFileRoute } from '@tanstack/react-router'
import { SecretsPage } from '@nasiko/features/settings/SecretsPage'
import { SettingsLayout } from '@nasiko/features/settings/SettingsLayout'

export const Route = createFileRoute('/_app/settings/secrets')({ component: SecretsRoute })

function SecretsRoute() {
  return (
    <SettingsLayout>
      <SecretsPage />
    </SettingsLayout>
  )
}
