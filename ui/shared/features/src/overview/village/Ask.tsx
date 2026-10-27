/**
 * The chat box at the village's bottom centre (Ans, 2026-10-17: the query box there becomes a chat box): a question
 * for the Orchestrator about the fleet. Sending opens a new routed chat (`/chat?auto=1`) with the question as its draft
 * (chat's own `new:routed` draft key), so the person reads it once more and sends it there, in the full composer with
 * its model and mode controls. Nothing is sent from the canvas itself.
 */
import { useQuery } from '@tanstack/react-query'
import { useNavigate } from '@tanstack/react-router'
import { ArrowUp, Sparkles } from 'lucide-react'
import { useState } from 'react'
import { Button } from '@nasiko/ui/primitives/button'
import { Input } from '@nasiko/ui/primitives/input'
import { newChatDraftKey, writeDraft } from '@nasiko/features/chat/drafts'
import { meQuery } from '@nasiko/data-access/auth'
import { cn } from '@nasiko/ui/lib/utils'
import { villageCopy as copy } from './copy'
import { CARD } from './styles'

export function CanvasAsk({ className }: { className?: string }) {
  const me = useQuery(meQuery)
  const navigate = useNavigate()
  const [text, setText] = useState('')
  const ready = text.trim().length > 0 && !!me.data
  const send = () => {
    if (!ready || !me.data) return
    writeDraft(me.data.sub, newChatDraftKey({ routed: true }), text.trim())
    setText('')
    void navigate({ to: '/chat', search: { auto: 1 } })
  }
  return (
    <form
      aria-label={copy.ask.label}
      className={cn(CARD, 'flex h-11 items-center gap-1.5 pr-1.5 pl-3', className)}
      onSubmit={(e) => {
        e.preventDefault()
        send()
      }}
    >
      <Sparkles aria-hidden className="size-4 shrink-0 text-muted-foreground" />
      <Input
        value={text}
        onChange={(e) => setText(e.target.value)}
        placeholder={copy.ask.placeholder}
        aria-label={copy.ask.label}
        className="h-9 flex-1 border-0 bg-transparent px-1 shadow-none focus-visible:ring-0 dark:bg-transparent"
      />
      <Button
        type="submit"
        size="icon"
        className="size-8 shrink-0"
        aria-label={copy.ask.send}
        disabled={!ready}
      >
        <ArrowUp aria-hidden />
      </Button>
    </form>
  )
}
