/**
 * "Open chat" from Sessions and the trace page (v1c §5.10, DS10): a Sessions row id is a chat id, so a
 * probe asks whether it's one of the viewer's chats. While it asks, a reserved slot holds a disabled
 * link so nothing moves; a 404 removes it (someone else's session, or a deleted chat); other failures
 * say so with Retry and Copy details (DX4).
 */
import { Link } from '@tanstack/react-router'
import { MessagesSquare, RotateCw } from 'lucide-react'
import { Button, buttonVariants } from '@/components/ui/button'
import { copy } from '@/features/observability/copy'
import { ApiError } from '@/lib/api/client'
import { cn } from '@/lib/utils'
import { probePath, useChatProbe } from '../api'
import { CopyText } from './turnParts'

/** The anchor Copy details points to (docs/chat.md). */
const PROBE_DOC = 'docs/chat.md#errors-probe'

export function OpenChatLink({
  sessionId,
  enabled = true,
  className,
}: {
  sessionId: string
  enabled?: boolean
  className?: string
}) {
  const probe = useChatProbe(sessionId, enabled)
  if (probe.state === 'absent') return null
  if (probe.state === 'probing') {
    return (
      // The same box as the link it becomes, so nothing moves when the probe answers.
      <span
        aria-disabled="true"
        className={cn(
          buttonVariants({ variant: 'outline', size: 'sm' }),
          'pointer-events-none opacity-60 pointer-coarse:min-h-11',
          className,
        )}
        data-testid="open-chat-slot"
      >
        <MessagesSquare className="size-3.5" aria-hidden /> {copy.openChat}
      </span>
    )
  }
  if (probe.state === 'error') {
    const status = probe.error instanceof ApiError ? probe.error.status : 'none'
    const details = [
      `status: ${status}`,
      `path: ${probePath(sessionId)}`,
      `docs: ${PROBE_DOC}`,
    ].join('\n')
    return (
      <span
        role="note"
        className={cn(
          'inline-flex flex-wrap items-center gap-2 text-xs text-muted-foreground',
          className,
        )}
        data-testid="open-chat-error"
      >
        <span title={copy.probeFailedCause}>{copy.probeFailed}</span>
        <Button
          type="button"
          size="xs"
          variant="outline"
          className="pointer-coarse:min-h-11"
          onClick={probe.retry}
        >
          <RotateCw aria-hidden /> {copy.retryProbe}
        </Button>
        <CopyText text={details} label={copy.copyProbeDetails} size="xs" variant="ghost" />
      </span>
    )
  }
  return (
    <Button
      asChild
      variant="outline"
      size="sm"
      className={cn('pointer-coarse:min-h-11', className)}
    >
      <Link to="/chat/$sessionId" params={{ sessionId }} search={{}}>
        <MessagesSquare className="size-3.5" aria-hidden /> {copy.openChat}
      </Link>
    </Button>
  )
}
