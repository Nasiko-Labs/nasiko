import { createFileRoute } from '@tanstack/react-router'
import { prefetchChat } from '@/features/chat/api'
import { ChatPage } from '@/features/chat/ChatPage'
import { chatSearchSchema } from '@/features/chat/search'

/** A new chat: `?agent=` picks the agent (a UUID or a name, plan §6.11). */
export const Route = createFileRoute('/_app/chat/')({
  validateSearch: chatSearchSchema,
  // Hover or touch intent on a link starts the page's first requests; a direct load fetches from the page (plan §8 Phase 3).
  loader: ({ context, preload }) => {
    if (preload) prefetchChat(context.queryClient)
  },
  component: NewChatRoute,
})

function NewChatRoute() {
  return <ChatPage search={Route.useSearch()} />
}
