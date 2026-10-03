/** Pieces both chat views share: their props, the confirm dialog, the send error and the notes. */
import { Link } from '@tanstack/react-router'
import { useEffect, useRef, type ReactNode, type RefObject } from 'react'
import {
  AlertDialog,
  AlertDialogAction,
  AlertDialogCancel,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogTitle,
} from '@/components/ui/alert-dialog'
import { Button } from '@/components/ui/button'
import { cn } from '@/lib/utils'
import { AgentLinkTo } from '@/features/agents/components/bits'
import { TargetBanner } from './NewChat'
import { ModalAnnouncer } from './StatusAnnouncer'
import { copy } from '../copy'
import { CHAT_COLUMN } from './turnStyles'
import type { ChatSearch } from '../search'
import type { TurnRegistry } from '../turnRegistry'
import { tuning } from '../tuning'
import { sendErrorText } from '../errors'

export interface ViewProps {
  userId: string
  registry: TurnRegistry
  search: ChatSearch
  railButton: ReactNode
  newChatRef: RefObject<HTMLAnchorElement | null>
  /** For recorded chats' names (the list prefixes harness names with `<username>-`, §2.2). */
  username?: string
}

export function ConfirmDialog({
  open,
  onOpenChange,
  title,
  body,
  primary,
  secondary,
  onCloseAutoFocus,
}: {
  open: boolean
  onOpenChange(open: boolean): void
  title: string
  body: string
  /** `keepOpen`: the action closes the dialog itself once it succeeds (delete). */
  primary: {
    label: string
    onClick(): void
    destructive?: boolean
    keepOpen?: boolean
    busy?: boolean
  }
  secondary?: { label: string; onClick(): void }
  /** Where focus goes on close; default: back to the trigger. */
  onCloseAutoFocus?: (e: Event) => void
}) {
  return (
    <AlertDialog open={open} onOpenChange={onOpenChange}>
      <AlertDialogContent onCloseAutoFocus={onCloseAutoFocus}>
        <AlertDialogHeader>
          <AlertDialogTitle>{title}</AlertDialogTitle>
          <AlertDialogDescription>{body}</AlertDialogDescription>
        </AlertDialogHeader>
        <AlertDialogFooter>
          <AlertDialogCancel>{copy.cancel}</AlertDialogCancel>
          {secondary ? (
            <Button
              variant="outline"
              onClick={() => {
                onOpenChange(false)
                secondary.onClick()
              }}
            >
              {secondary.label}
            </Button>
          ) : null}
          <AlertDialogAction
            variant={primary.destructive ? 'destructive' : 'default'}
            disabled={primary.busy}
            onClick={(e) => {
              if (primary.keepOpen) e.preventDefault()
              primary.onClick()
            }}
          >
            {primary.label}
          </AlertDialogAction>
        </AlertDialogFooter>
        <ModalAnnouncer />
      </AlertDialogContent>
    </AlertDialog>
  )
}

export function SendError({ error, inline = false }: { error: unknown; inline?: boolean }) {
  const text = sendErrorText(error)
  return text ? (
    <p
      className={cn(inline ? 'mt-1.5 px-1' : CHAT_COLUMN, 'text-sm text-destructive')}
      data-testid="send-error"
    >
      {text}
    </p>
  ) : null
}

export function NotRunningBanner({
  agentId,
  name,
  refetch,
}: {
  agentId: string
  name: string
  refetch(): void
}) {
  // DS17: refetch status on focus and every STATUS_POLL_MS; the banner clears itself.
  // The ref keeps one interval alive across re-renders (callers pass an inline refetch).
  const latest = useRef(refetch)
  useEffect(() => {
    latest.current = refetch
  }, [refetch])
  useEffect(() => {
    const tick = () => latest.current()
    const id = setInterval(tick, tuning.STATUS_POLL_MS)
    window.addEventListener('focus', tick)
    return () => {
      clearInterval(id)
      window.removeEventListener('focus', tick)
    }
  }, [])
  return (
    <TargetBanner
      action={
        <AgentLinkTo
          id={agentId}
          className="inline-flex items-center text-primary-text underline-offset-4 hover:underline pointer-coarse:min-h-11"
        >
          {copy.targetNotRunningAction}
        </AgentLinkTo>
      }
    >
      {copy.targetNotRunning(name)}
    </TargetBanner>
  )
}

/** A harness session recorded under the metadata-only policy (v1c §5.7): nothing to show, and why. */
export function MetadataOnly() {
  return (
    <div
      className="mx-auto mt-8 max-w-md space-y-2 text-center text-sm text-muted-foreground"
      data-testid="metadata-only"
    >
      <p>{copy.metadataOnly}</p>
      <Link
        to="/harnesses"
        search={{}}
        className="inline-flex items-center text-primary-text underline-offset-4 hover:underline pointer-coarse:min-h-11"
      >
        {copy.viewInHarnesses}
      </Link>
    </div>
  )
}

export function ReadOnlyNote({ why }: { why: 'recorded' | 'removed' }) {
  return (
    <div className="shrink-0 pt-2 pb-[max(0.75rem,env(safe-area-inset-bottom))]">
      <div role="note" className={cn(CHAT_COLUMN)}>
        <div className="flex min-h-11 flex-wrap items-center gap-2 rounded-xl border border-border bg-card px-3 py-2.5 text-sm text-muted-foreground shadow-xs">
          {why === 'recorded' ? (
            <>
              {copy.recordedReadOnly}{' '}
              <Link
                to="/harnesses"
                search={{}}
                className="inline-flex items-center text-primary-text underline-offset-4 hover:underline pointer-coarse:min-h-11"
              >
                {copy.viewInHarnesses}
              </Link>
            </>
          ) : (
            copy.agentRemoved
          )}
        </div>
      </div>
    </div>
  )
}
