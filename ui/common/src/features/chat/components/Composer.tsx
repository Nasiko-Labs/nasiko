/**
 * The message box (plan §6.5, §7.7; v1c §5.5): a card holding an auto-growing textarea, with the target
 * chip, the key hint and Send inside it. Locked means Send is disabled; the textarea stays editable and
 * Enter keeps the draft. IME composition Enter is ignored. While a turn streams, Stop receiving takes
 * Send's slot (direct chats only). While a request is pending, Go to request and Start a new chat show
 * inside the card. `/` focuses the box when nothing else takes typing.
 */
import { ArrowUp, Square } from 'lucide-react'
import {
  useEffect,
  useImperativeHandle,
  useRef,
  type KeyboardEvent,
  type ReactNode,
  type Ref,
} from 'react'
import { Button } from '@/components/ui/button'
import { Textarea } from '@/components/ui/textarea'
import { cn } from '@/lib/utils'
import { copy } from '../copy'
import { tuning } from '../tuning'
import { CHAT_COLUMN } from './turnStyles'

export type ComposerMode =
  | { kind: 'ready' }
  /**
   * `blocked`: Send stays focusable and `aria-disabled`, and activating it (or Enter) calls this instead
   * of sending: the no-target new chat opens the TargetPicker (DS3), a stopped agent announces why (E16).
   */
  | { kind: 'locked'; reason?: string; newChat?: boolean; blocked?(): void }
  | { kind: 'streaming' }
  | { kind: 'paused' }

export interface ComposerHandle {
  focus(): void
}

const typingTarget = (el: Element | null) =>
  !!el &&
  (el instanceof HTMLInputElement ||
    el instanceof HTMLTextAreaElement ||
    el instanceof HTMLSelectElement ||
    (el as HTMLElement).isContentEditable)

export function Composer({
  agentName,
  label,
  placeholder,
  note,
  inline = false,
  targetSlot,
  value,
  onChange,
  onSend,
  onStop,
  onGoToRequest,
  onNewChat,
  mode,
  ref,
}: {
  ref?: Ref<ComposerHandle>
  agentName?: string
  /** Overrides "Message <agent>" (routed: "Ask the Orchestrator", v1c §5.1). */
  label?: string
  placeholder?: string
  /** A note under the card while it applies (the routed hint, §5.2). */
  note?: string
  /** Inside a page body (the new chat's centred layout) rather than docked at the bottom. */
  inline?: boolean
  /** The new chat's target chip, at the card's bottom left (§5.4). */
  targetSlot?: ReactNode
  value: string
  onChange(v: string): void
  onSend(): void
  onStop(): void
  onGoToRequest(): void
  onNewChat(): void
  mode: ComposerMode
}) {
  const box = useRef<HTMLTextAreaElement>(null)
  useImperativeHandle(ref, () => ({ focus: () => box.current?.focus() }), [])

  useEffect(() => {
    const onKey = (e: globalThis.KeyboardEvent) => {
      if (
        e.key !== '/' ||
        e.metaKey ||
        e.ctrlKey ||
        e.altKey ||
        typingTarget(document.activeElement)
      )
        return
      // An open dialog, sheet or menu (shadcn's, found by role) owns the keyboard.
      if (document.querySelector('[role=dialog][data-state=open], [role=alertdialog], [role=menu]'))
        return
      e.preventDefault()
      box.current?.focus()
    }
    window.addEventListener('keydown', onKey)
    return () => window.removeEventListener('keydown', onKey)
  }, [])

  const tooLong = value.length > tuning.MESSAGE_MAX_CHARS
  const canSend = mode.kind === 'ready' && !!value.trim() && !tooLong
  const blocked = mode.kind === 'locked' ? mode.blocked : undefined
  const submit = () => {
    if (canSend) onSend()
    else if (blocked) blocked()
  }
  const onKeyDown = (e: KeyboardEvent<HTMLTextAreaElement>) => {
    if (e.key !== 'Enter' || e.shiftKey || e.nativeEvent.isComposing || e.keyCode === 229) return
    e.preventDefault()
    submit()
  }
  const reason = tooLong ? (
    <span className="text-destructive">{copy.tooLongInline}</span>
  ) : mode.kind === 'locked' && mode.reason ? (
    mode.reason
  ) : null

  return (
    <form
      className={
        inline ? '' : 'shrink-0 bg-background pt-2 pb-[max(0.75rem,env(safe-area-inset-bottom))]'
      }
      onSubmit={(e) => {
        e.preventDefault()
        submit()
      }}
    >
      <div className={inline ? 'w-full' : CHAT_COLUMN}>
        <label htmlFor="chat-composer" className="sr-only">
          {label ?? copy.composerLabel(agentName)}
        </label>
        <div className="rounded-xl border border-border bg-card shadow-xs focus-within:border-ring focus-within:ring-2 focus-within:ring-ring focus-within:ring-offset-2 focus-within:ring-offset-background">
          <Textarea
            id="chat-composer"
            ref={box}
            value={value}
            placeholder={placeholder ?? label ?? copy.composerLabel(agentName)}
            onChange={(e) => onChange(e.target.value)}
            onKeyDown={onKeyDown}
            rows={1}
            aria-describedby={
              reason ? 'chat-composer-hint chat-composer-keys' : 'chat-composer-keys'
            }
            aria-invalid={tooLong || undefined}
            className="field-sizing-content max-h-44 min-h-11 resize-none rounded-xl border-0 bg-transparent px-3 pt-3 shadow-none focus-visible:ring-0 focus-visible:ring-offset-0 dark:bg-transparent"
          />
          {(mode.kind === 'locked' && mode.newChat) || mode.kind === 'paused' ? (
            <div className="flex flex-wrap items-center gap-2 px-3 pt-1">
              {mode.kind === 'paused' ? (
                <Button
                  type="button"
                  size="sm"
                  className="pointer-coarse:min-h-11"
                  onClick={onGoToRequest}
                >
                  {copy.goToRequest}
                </Button>
              ) : null}
              <Button
                type="button"
                size="sm"
                variant="outline"
                className="pointer-coarse:min-h-11"
                onClick={onNewChat}
              >
                {copy.startNewChat}
              </Button>
            </div>
          ) : null}
          <div className="flex min-h-11 items-center gap-2 px-2 pb-2">
            {targetSlot}
            <span
              id="chat-composer-keys"
              className="ml-auto hidden truncate text-xs text-muted-foreground md:inline"
            >
              {copy.composerHint}
            </span>
            {mode.kind === 'streaming' ? (
              <Button
                type="button"
                size="sm"
                variant="outline"
                onClick={onStop}
                className="ml-auto shrink-0 md:ml-0 pointer-coarse:min-h-11"
              >
                <Square aria-hidden /> {copy.stopReceiving}
              </Button>
            ) : (
              // Paused keeps Send in place, disabled (§7.7): sending is blocked, not gone.
              <Button
                type="submit"
                disabled={!canSend && !blocked}
                aria-disabled={!canSend && blocked ? true : undefined}
                aria-label={copy.send}
                className={cn(
                  'ml-auto size-8 shrink-0 rounded-lg p-0 md:ml-0 pointer-coarse:size-11',
                  !canSend && blocked && 'opacity-50',
                )}
              >
                <ArrowUp aria-hidden />
              </Button>
            )}
          </div>
        </div>
        {reason ? (
          <p id="chat-composer-hint" className="mt-1.5 px-1 text-xs text-muted-foreground">
            {reason}
          </p>
        ) : null}
        {note ? (
          <p className="mt-1.5 px-1 text-xs text-muted-foreground" data-testid="composer-note">
            {note}
          </p>
        ) : null}
      </div>
    </form>
  )
}
