/** An open chat's header (§7.3, §6.10): identity, title (inline rename), totals, View session and the ⋯ menu. */
import { Link, useNavigate } from '@tanstack/react-router'
import { Activity, MoreHorizontal } from 'lucide-react'
import { useEffect, useRef, useState, type ReactNode } from 'react'
import { TopBar } from '@nasiko/app-core/shell/TopBar'
import { PrevNext, type StepVia } from '@nasiko/ui/composites/prev-next'
import { Button, asButton } from '@nasiko/ui/primitives/button'
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuLabel,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from '@nasiko/ui/primitives/dropdown-menu'
import { Input } from '@nasiko/ui/primitives/input'
import { Tooltip, TooltipContent, TooltipTrigger } from '@nasiko/ui/primitives/tooltip'
import { ApiError } from '@nasiko/data-access/client'
import { siblingsAround } from '@nasiko/app-core/siblings'
import { useRenameChat } from '../api'
import { copy } from '../copy'
import { announce } from '../announce'
import { fmtCostPerOp, fmtTokens } from '@nasiko/ui/lib/format'
import { useRailOrder } from '../railOrder'
import { chatTotals } from '../format'
import type { ChatIdentity } from '../identity'
import type { ChatMessage, ChatSessionRow } from '../types'
import { IdentityChip, IdentityIcon } from './ChatIdentity'

// quirk: §10.11 — the server caps nothing; this is the UI's own cap.
const TITLE_MAX = 80
const TITLE_COUNTER_FROM = 70

export function SessionHeader({
  sessionId,
  row,
  loading,
  readOnly,
  identity,
  railButton,
  replies,
  partial,
  canExport,
  onExport,
  deleteBlocked,
  onDelete,
}: {
  sessionId: string
  row: ChatSessionRow | undefined
  /** The chat's details are still loading (the title says so). */
  loading: boolean
  readOnly: boolean
  identity: ChatIdentity | null
  railButton: ReactNode
  replies: ChatMessage[]
  /** Older pages exist: totals cover the loaded messages only. */
  partial: boolean
  canExport: boolean
  onExport(): void
  // quirk: §10.6 — delete fails on any request row (FK without cascade), so it is disabled up front.
  deleteBlocked: boolean
  onDelete(): void
}) {
  const [editing, setEditing] = useState(false)
  /** Set while Rename is chosen from the menu, so the closing menu doesn't pull focus off the input. */
  const renaming = useRef(false)
  return (
    // The app's top bar holds the chat's header: identity and title on the left, its actions, then Previous/Next.
    <TopBar
      pinActions
      root={
        <Link to="/chat" search={{}}>
          {copy.chatsNav}
        </Link>
      }
      trail={[
        <div key="chat" className="flex min-w-0 items-center gap-2">
          {railButton}
          {identity ? <IdentityIcon kind={identity.kind} name={identity.name} size={20} /> : null}
          {editing && row ? (
            <RenameTitle sessionId={sessionId} title={row.title} onDone={() => setEditing(false)} />
          ) : (
            <Button
              type="button"
              variant="ghost"
              className="h-auto min-w-0 shrink justify-start px-0 py-0 text-left text-sm font-medium hover:bg-transparent hover:underline dark:hover:bg-transparent pointer-coarse:min-h-11"
              title={copy.rename}
              onClick={() => row && !readOnly && setEditing(true)}
            >
              <span className="truncate">
                {row?.title ?? (loading ? copy.loadingChat : copy.chatsNav)}
              </span>
            </Button>
          )}
          {/* The chip hides below 480 px of pane width; the icon keeps the kind (DS5). */}
          {identity ? (
            <IdentityChip identity={identity} className="hidden @min-[480px]:inline-flex" />
          ) : null}
        </div>,
      ]}
      actions={
        <>
          <Link
            to="/sessions/$sessionId"
            params={{ sessionId }}
            search={{}}
            {...asButton({
              variant: 'ghost',
              size: 'sm',
              className: 'hidden @min-[560px]:inline-flex pointer-coarse:min-h-11',
            })}
          >
            <Activity aria-hidden />
            {copy.viewSession}
          </Link>
          <DropdownMenu>
            <DropdownMenuTrigger
              render={
                <Button
                  size="icon-sm"
                  variant="ghost"
                  className="pointer-coarse:size-11"
                  aria-label={copy.moreChatActions}
                />
              }
            >
              <MoreHorizontal aria-hidden />
            </DropdownMenuTrigger>
            <DropdownMenuContent
              align="end"
              className="w-64"
              onCloseAutoFocus={(e) => {
                // RenameTitle focuses itself; don't send focus back to the ⋯ trigger.
                if (renaming.current) {
                  e.preventDefault()
                  renaming.current = false
                }
              }}
            >
              {/* The totals live here, not in the bar: each reply shows its own (one cost readout per place). */}
              {replies.length ? (
                <>
                  <DropdownMenuLabel className="font-normal">
                    <HeaderTotals replies={replies} partial={partial} stacked />
                  </DropdownMenuLabel>
                  <DropdownMenuSeparator />
                </>
              ) : null}
              <DropdownMenuItem
                className="@min-[560px]:hidden pointer-coarse:min-h-11"
                render={<Link to="/sessions/$sessionId" params={{ sessionId }} search={{}} />}
              >
                {copy.viewSession}
              </DropdownMenuItem>
              <DropdownMenuItem
                className="pointer-coarse:min-h-11"
                disabled={!row || readOnly}
                onClick={() => {
                  renaming.current = true
                  setEditing(true)
                }}
              >
                {copy.rename}
              </DropdownMenuItem>
              <DropdownMenuItem
                className="pointer-coarse:min-h-11"
                disabled={!canExport}
                onClick={onExport}
              >
                {copy.exportMarkdown}
              </DropdownMenuItem>
              <DropdownMenuItem
                disabled={deleteBlocked}
                className="text-destructive pointer-coarse:min-h-11"
                onClick={onDelete}
              >
                {deleteBlocked ? (
                  <span className="flex flex-col">
                    <span>{copy.delete}</span>
                    <span className="text-xs text-muted-foreground">{copy.deleteBlocked}</span>
                  </span>
                ) : (
                  copy.delete
                )}
              </DropdownMenuItem>
            </DropdownMenuContent>
          </DropdownMenu>
        </>
      }
      stepper={<ChatSiblings sessionId={sessionId} />}
    />
  )
}

/**
 * Previous/Next through the rail's chats as shown (railOrder.ts). Pushes, like a rail click; dev aids carry over.
 * Below 480 px of pane width the buttons hide (the rail is the way there), but J/K still work: PrevNext stays mounted.
 */
function ChatSiblings({ sessionId }: { sessionId: string }) {
  const navigate = useNavigate()
  const ids = useRailOrder((s) => s.ids)
  const around = siblingsAround(ids, sessionId)
  if (!around) return null
  const go = (id: string | undefined, via: StepVia) => {
    if (id)
      void navigate({
        from: '/chat/$sessionId',
        to: '/chat/$sessionId',
        params: { sessionId: id },
        search: (s) => ({ mock: s.mock, debug: s.debug }),
        // A click lands in the composer; J/K keeps focus out of it, so the next key steps again.
        state: { chatFocus: via === 'key' ? 'none' : 'composer' },
      })
  }
  return (
    <PrevNext
      noun="chat"
      index={around.index}
      count={around.count}
      onPrev={(via) => go(around.prev, via)}
      onNext={(via) => go(around.next, via)}
      restoreFocus={false}
      className="hidden @min-[480px]:flex"
    />
  )
}

/**
 * Tokens and cost over the loaded replies (§7.3, E6): hidden before the first reply; labelled
 * "Usage for loaded messages" while older pages exist; unknown cost is "—", estimates get "~".
 */
function HeaderTotals({
  replies,
  partial,
  className,
  stacked = false,
}: {
  replies: ChatMessage[]
  partial: boolean
  className?: string
  stacked?: boolean
}) {
  if (!replies.length) return null
  const t = chatTotals(replies)
  // Replies with only a duration (agents that report no tokens) have nothing to total.
  if (!t.tokens && t.cost === null) return null
  const tilde = t.estimated ? '~' : ''
  const cost =
    t.cost === null ? (
      <Tooltip>
        <TooltipTrigger
          render={
            <span
              // eslint-disable-next-line jsx-a11y/no-noninteractive-tabindex -- focusable only so keyboard users can open the cost-unknown tooltip; it has no action, so no button role
              tabIndex={0}
            />
          }
        >
          —
        </TooltipTrigger>
        <TooltipContent>{copy.costUnknown}</TooltipContent>
      </Tooltip>
    ) : (
      <span>
        {tilde}
        {fmtCostPerOp(t.cost)}
      </span>
    )
  return (
    <div
      className={`${stacked ? 'flex flex-col gap-0.5' : 'items-center gap-2'} text-xs text-muted-foreground tabular-nums ${className ?? ''}`}
      data-testid="chat-totals"
    >
      {partial ? <span>{copy.usageLoaded}</span> : null}
      <span>
        {copy.tokens(`${tilde}${fmtTokens(t.tokens)}`)} · {cost}
      </span>
      {t.withoutUsage ? <span>{copy.turnsWithoutUsage(t.withoutUsage)}</span> : null}
    </div>
  )
}

/** Inline rename (§6.10): Enter or blur saves, Esc cancels, empty reverts, 80-char cap. */
function RenameTitle({
  sessionId,
  title,
  onDone,
}: {
  sessionId: string
  title: string
  onDone(): void
}) {
  const rename = useRenameChat(sessionId)
  const [value, setValue] = useState(title)
  const [error, setError] = useState<string | null>(null)
  const done = useRef(false)
  const input = useRef<HTMLInputElement>(null)
  // A blur saves only once the input has really held focus: opened from the ⋯ menu, the menu's
  // focus trap can take focus back before it closes, and that must not count as "done".
  const armed = useRef(false)
  useEffect(() => {
    const id = requestAnimationFrame(() => input.current?.focus())
    return () => cancelAnimationFrame(id)
  }, [])
  const save = async () => {
    if (done.current) return
    done.current = true
    const next = value.trim()
    if (!next || next === title) return onDone()
    try {
      await rename.mutateAsync(next)
      onDone()
    } catch (err) {
      done.current = false
      setValue(title)
      const text =
        err instanceof ApiError && err.serverMessage
          ? copy.serverSaid(err.serverMessage)
          : copy.titleEmpty
      setError(text)
      announce(text)
    }
  }
  return (
    <div className="flex min-w-0 items-center gap-2">
      <Input
        ref={input}
        aria-label={copy.titleLabel}
        value={value}
        maxLength={TITLE_MAX}
        onChange={(e) => setValue(e.target.value)}
        onFocus={() => {
          armed.current = true
        }}
        onBlur={() => {
          if (armed.current) void save()
        }}
        onKeyDown={(e) => {
          if (e.key === 'Enter') {
            e.preventDefault()
            void save()
          }
          if (e.key === 'Escape') {
            done.current = true
            onDone()
          }
        }}
        className="h-8 w-64 max-w-full pointer-coarse:h-11"
      />
      {value.length >= TITLE_COUNTER_FROM ? (
        <span className="text-xs text-muted-foreground tabular-nums">
          {copy.titleCounter(value.length, TITLE_MAX)}
        </span>
      ) : null}
      {error ? <span className="text-xs text-destructive">{error}</span> : null}
    </div>
  )
}
