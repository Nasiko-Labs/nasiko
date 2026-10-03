/**
 * Chat (plan §7): the rail plus either a new chat (`/chat`, `NewChatView.tsx`) or an existing one
 * (`/chat/$sessionId`, `SessionView.tsx`). Turns live in the registry (§6.2). The rail is the app sidebar's drill-in
 * panel (`SidebarPanel`); with the sidebar collapsed it sits beside the chat from 1024 px, in a sheet below that.
 */
import { useQuery } from '@tanstack/react-query'
import { MessagesSquare } from 'lucide-react'
import { use, useCallback, useMemo, useRef, useState } from 'react'
import { SidebarPanel } from '@/app/shell/SidebarPanel'
import { SidebarPanelContext } from '@/app/shell/panelSlot'
import { DotBackground } from '@/components/aceternity/dot-background'
import { PageLoader } from '@/components/shared/page-loader'
import { Button } from '@/components/ui/button'
import { Sheet, SheetContent, SheetTitle } from '@/components/ui/sheet'
import { useSidebar } from '@/components/ui/sidebar'
import { ErrorState } from '@/features/observability/StateCard'
import { meQuery } from '@/lib/api/auth'
import { useMediaQuery } from '@/lib/useMediaQuery'
import { ChatRail } from './components/ChatRail'
import { ModalAnnouncer } from './components/StatusAnnouncer'
import { copy } from './copy'
import { useChatRegistry, useSignals } from './registry'
import { useWaiting } from './waiting'
import { WaitingPill } from './components/WaitingPill'
import { agentParamText, isAuto, type ChatSearch } from './search'
import type { ViewProps } from './components/PageParts'
import { SessionViewMemo } from './SessionView'
import { NewChatViewMemo } from './NewChatView'

export function ChatPage({ sessionId, search }: { sessionId?: string; search: ChatSearch }) {
  const me = useQuery(meQuery)
  // Wait for the real user: a registry keyed by a placeholder would abort every live turn
  // whenever /api/me briefly reloads (a 401 clears the query cache).
  if (me.isError)
    return (
      <div className="mx-auto mt-12 w-full max-w-md px-4">
        <ErrorState error={me.error} onRetry={() => void me.refetch()} />
      </div>
    )
  if (!me.data) return <PageLoader label={copy.loadingChat} />
  return (
    <ChatLayout
      userId={me.data.sub}
      username={me.data.username}
      isSuperuser={!!me.data.is_superuser}
      sessionId={sessionId}
      search={search}
    />
  )
}

function ChatLayout({
  userId,
  username,
  isSuperuser,
  sessionId,
  search,
}: {
  userId: string
  username?: string
  isSuperuser: boolean
  sessionId?: string
  search: ChatSearch
}) {
  const registry = useChatRegistry(userId)
  // In the sidebar while it can hold the rail; otherwise inline from 1024 px, a sheet below that (v1c DS5).
  const inSidebar = use(SidebarPanelContext).target !== null
  const { isMobile, setOpenMobile } = useSidebar()
  const wide = useMediaQuery('(min-width: 1024px)')
  const [railOpen, setRailOpen] = useState(false)
  const newChatRef = useRef<HTMLAnchorElement>(null)
  const carry = { mock: search.mock, debug: search.debug }
  // The Waiting queue polls here, so it runs on every chat route even with the phone sheet closed (v1c §5.9).
  const waiting = useWaiting(userId, isSuperuser)
  const rail = (
    <ChatRail
      ref={newChatRef}
      activeId={sessionId}
      search={carry}
      username={username}
      userId={userId}
      waiting={waiting}
      onNavigate={() => {
        setRailOpen(false)
        setOpenMobile(false)
      }}
    />
  )
  // Polls and signal changes re-render the rail, not the open chat: the views are memoised on stable props, and
  // the phone trigger subscribes to the signals itself (review, performance).
  const waitingChats = waiting.match.chats.length
  const openRail = useCallback(() => setRailOpen(true), [])
  const railButton = useMemo(
    () =>
      wide || inSidebar ? null : <RailTrigger waitingChats={waitingChats} onOpen={openRail} />,
    [wide, inSidebar, waitingChats, openRail],
  )
  const props: ViewProps = useMemo(
    () => ({ userId, registry, search, railButton, newChatRef, username }),
    [userId, registry, search, railButton, username],
  )
  return (
    <SidebarPanel
      panel={
        <>
          {rail}
          {/* The phone sheet is modal: the page's announcer is hidden while it is open. */}
          {isMobile ? <ModalAnnouncer /> : null}
        </>
      }
    >
      {() => (
        <div className="flex h-full min-h-0">
          {inSidebar ? null : wide ? (
            <aside className="w-60 shrink-0 border-r border-border">{rail}</aside>
          ) : (
            <Sheet open={railOpen} onOpenChange={setRailOpen}>
              <SheetContent side="left" className="w-[85vw] max-w-80 p-0">
                <SheetTitle className="sr-only">{copy.chats}</SheetTitle>
                {/* The sheet's close button sits top-right; start the list below it. */}
                <div className="h-full min-h-0 pt-10">{rail}</div>
                <ModalAnnouncer />
              </SheetContent>
            </Sheet>
          )}
          {/* isolate: the dots' -z-10 stays inside the section, above the page background. */}
          <section className="@container relative isolate flex min-w-0 flex-1 flex-col">
            <DotBackground className="-z-10" />
            {sessionId ? (
              <SessionViewMemo key={sessionId} sessionId={sessionId} {...props} />
            ) : (
              <NewChatViewMemo
                key={
                  search.agent !== undefined
                    ? `agent:${agentParamText(search.agent)}`
                    : isAuto(search)
                      ? 'auto'
                      : 'choose'
                }
                {...props}
              />
            )}
          </section>
        </div>
      )}
    </SidebarPanel>
  )
}

/** The phone's Chats trigger (v1c §5.8, DP7): the waiting count, and a dot for unseen replies, in its name too. */
function RailTrigger({ waitingChats, onOpen }: { waitingChats: number; onOpen(): void }) {
  const newReplies = useSignals()?.unseenChats ?? 0
  return (
    <Button
      size="icon-sm"
      variant="ghost"
      className="relative pointer-coarse:size-11"
      aria-label={copy.chatsTrigger(waitingChats, newReplies)}
      onClick={onOpen}
    >
      <MessagesSquare aria-hidden />
      {waitingChats ? (
        <WaitingPill n={waitingChats} className="absolute -top-0.5 -right-0.5" />
      ) : newReplies ? (
        <span
          aria-hidden
          className="absolute top-1 right-1 size-2 rounded-full bg-primary"
          data-testid="trigger-dot"
        />
      ) : null}
      {waitingChats && newReplies ? (
        <span
          aria-hidden
          className="absolute right-0.5 bottom-0.5 size-2 rounded-full bg-primary"
          data-testid="trigger-dot"
        />
      ) : null}
    </Button>
  )
}
