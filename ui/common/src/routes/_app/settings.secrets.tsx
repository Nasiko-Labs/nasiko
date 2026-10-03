import { createFileRoute } from '@tanstack/react-router'
import { SecretsPage } from '@/features/settings/SecretsPage'
import { SettingsLayout } from '@/features/settings/SettingsLayout'

export const Route = createFileRoute('/_app/settings/secrets')({ component: SecretsRoute })

function SecretsRoute() {
  return (
    <SettingsLayout>
      <SecretsPage />
    </SettingsLayout>
  )
}
