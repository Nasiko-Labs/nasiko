import { createFileRoute } from '@tanstack/react-router'
import { prefetchChat } from '@nasiko/features/chat/api'
import { ChatPage } from '@nasiko/features/chat/ChatPage'
import { chatSearchSchema } from '@nasiko/features/chat/search'

/** One chat. A sibling of /chat (not nested), so the page owns its full-height layout. */
export const Route = createFileRoute('/_app/chat/$sessionId')({
  validateSearch: chatSearchSchema,
  // Hover or touch intent on a link starts the page's first requests; a direct load fetches from the page (plan §8 Phase 3).
  loader: ({ context, params, preload }) => {
    if (preload) prefetchChat(context.queryClient, params.sessionId)
  },
  component: ChatRoute,
})

function ChatRoute() {
  const { sessionId } = Route.useParams()
  return <ChatPage sessionId={sessionId} search={Route.useSearch()} />
}
