/**
 * What connecting a coding agent sends to this server, shown wherever the CLI's connect steps are: on a hosted server
 * only usage (tokens, cost, models, timings), on a Nasiko running on the viewer's computer chat history too if they say
 * yes. The server says which (`useCodingAgentPrivacy`); one too old to say gets no notice.
 */
import { ShieldCheck } from 'lucide-react'
import { Alert, AlertDescription, AlertTitle } from '@nasiko/ui/primitives/alert'
import { cn } from '@nasiko/ui/lib/utils'
import { copy as pageCopy } from '../copy'
import { useCodingAgentPrivacy } from '../privacy'

const copy = pageCopy.chatHistory

export function ChatHistoryNotice({ className }: { className?: string }) {
  const policy = useCodingAgentPrivacy()
  if (!policy.data) return null
  const local = policy.data.chat_history
  return (
    <Alert role="note" data-testid="chat-history-notice" className={cn('p-3', className)}>
      <ShieldCheck aria-hidden />
      <AlertTitle>{local ? copy.localTitle : copy.hostedTitle}</AlertTitle>
      <AlertDescription>
        <p className="max-w-prose">{local ? copy.localBody : copy.hostedBody}</p>
      </AlertDescription>
    </Alert>
  )
}
