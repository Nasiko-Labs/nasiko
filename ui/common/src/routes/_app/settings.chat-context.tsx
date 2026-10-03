import { createFileRoute } from '@tanstack/react-router'
import { ChatContextPage } from '@/features/settings/ChatContextPage'
import { SettingsLayout } from '@/features/settings/SettingsLayout'

// Every user's own preference: no superuser gate, unlike the workspace sections.
export const Route = createFileRoute('/_app/settings/chat-context')({ component: ChatContextRoute })

function ChatContextRoute() {
  return (
    <SettingsLayout>
      <ChatContextPage />
    </SettingsLayout>
  )
}
