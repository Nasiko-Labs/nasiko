/**
 * `/chat` before the first message (v1c §5.4, L-D2): a hero for the target, the composer, then the
 * suggestions below it and the recent chats. Modelled on the legacy chat and orchestrator pages' main
 * section. Target problems (plan §6.11) render as banners; an ambiguous `?agent=` name gets a chooser.
 */
import { Link } from '@tanstack/react-router'
import { MessagesSquare, RotateCw } from 'lucide-react'
import type { ReactNode } from 'react'
import { Button } from '@/components/ui/button'
import { Skeleton } from '@/components/ui/skeleton'
import { relTime } from '@/features/agents/format'
import type { Agent } from '@/features/agents/types'
import { cn } from '@/lib/utils'
import { copy } from '../copy'
import { agentLabel, statusLabel } from '../format'
import { focusIfIdle } from '../focus'
import type { ChatIdentity } from '../identity'
import type { ChatCarry } from '../search'
import type { ChatSessionRow } from '../types'
import { IdentityIcon, StatusDot } from './ChatIdentity'
import type { TargetChoice } from './TargetPicker'
import { LIFTED, TOUCH } from './turnStyles'

/** The target list: the Orchestrator plus up to 5 running agents (6 rows), or 6 agents without it. */
const TARGET_ROWS = 6
const MAX_RECENT = 3

/** The hero by target (§5.4). */
export function NewChatHero({
  target,
  description,
}: {
  target: ChatIdentity | null
  description?: string | null
}) {
  // An agent's own description says more than "Ask me anything" (user review, 2026-09-28).
  const [title, subline] = !target
    ? [copy.heroChooseTitle, copy.heroChooseSubline]
    : target.kind === 'orchestrator'
      ? [copy.heroOrchestratorTitle, copy.heroOrchestratorSubline]
      : [target.name, description?.trim() || copy.heroAgentSubline]
  return (
    <div className="flex flex-col items-center gap-2 text-center" data-testid="new-chat-hero">
      {target ? (
        <IdentityIcon kind={target.kind} name={target.name} size={32} />
      ) : (
        <MessagesSquare aria-hidden className="size-8 text-primary-text" />
      )}
      <h1 className="max-w-full truncate text-2xl font-semibold">{title}</h1>
      <p className="max-w-prose text-sm text-muted-foreground">{subline}</p>
      {target?.kind === 'agent' && target.status && target.status !== 'running' ? (
        <p className="flex items-center gap-1.5 text-xs text-muted-foreground">
          <StatusDot running={false} />
          <span aria-hidden>{statusLabel(target.status)}</span>
        </p>
      ) : null}
    </div>
  )
}

/** Example chips under the composer: a chip fills the composer and focuses it; it doesn't send. */
export function ExampleChips({
  examples,
  onExample,
}: {
  examples: string[]
  onExample(text: string): void
}) {
  if (!examples.length) return null
  return (
    <ul className="flex flex-wrap justify-center gap-2" aria-label={copy.examples}>
      {examples.map((ex) => (
        <li key={ex}>
          <Button
            type="button"
            size="sm"
            variant="outline"
            className="h-auto max-w-80 rounded-full py-1.5 text-left whitespace-normal pointer-coarse:min-h-11"
            onClick={() => onExample(ex)}
          >
            {ex}
          </Button>
        </li>
      ))}
    </ul>
  )
}

/**
 * "Choose where to send" (§5.4, DS-T4): compact rows in a lifted panel, 2 columns from 640 px. A row
 * sets the target, as the picker does.
 */
export function TargetList({
  agents,
  withOrchestrator,
  directory,
  onChoose,
  id,
}: {
  agents: Agent[]
  withOrchestrator: boolean
  directory: { isPending: boolean; isError: boolean; retry(): void }
  onChoose(choice: TargetChoice): void
  id?: string
}) {
  const shown = agents.slice(0, withOrchestrator ? TARGET_ROWS - 1 : TARGET_ROWS)
  const row =
    'h-auto w-full justify-start gap-3 px-2 py-2 text-left font-normal whitespace-normal hover:bg-muted focus-visible:ring-offset-card pointer-coarse:min-h-11'
  return (
    <section id={id} className={cn(LIFTED, 'p-3')} aria-labelledby="choose-where">
      <h2 id="choose-where" className="px-2 pb-2 text-sm font-medium">
        {copy.chooseTarget}
      </h2>
      {directory.isPending ? (
        <div className="grid grid-cols-1 gap-1 sm:grid-cols-2" aria-busy>
          <span className="sr-only">{copy.loading}</span>
          {[0, 1, 2].map((i) => (
            <Skeleton key={i} className="h-12 motion-reduce:animate-none" />
          ))}
        </div>
      ) : (
        <ul className="grid grid-cols-1 gap-x-3 gap-y-1 sm:grid-cols-2">
          {withOrchestrator ? (
            <li className="min-w-0">
              <Button
                type="button"
                variant="ghost"
                className={row}
                onClick={() => onChoose({ kind: 'routed' })}
              >
                <IdentityIcon kind="orchestrator" name={copy.orchestratorName} size={32} />
                <span className="min-w-0 flex-1">
                  <span className="block truncate text-sm font-medium">
                    {copy.orchestratorName}
                  </span>
                  <span className="block truncate text-xs text-muted-foreground">
                    {copy.identityOrchestratorSubline}
                  </span>
                </span>
              </Button>
            </li>
          ) : null}
          {shown.map((a) => (
            <li key={a.id} className="min-w-0">
              <Button
                type="button"
                variant="ghost"
                className={row}
                onClick={() => onChoose({ kind: 'agent', agent: a })}
                title={a.description ?? undefined}
              >
                <IdentityIcon kind="agent" name={agentLabel(a)} size={32} />
                <span className="min-w-0 flex-1">
                  <span className="block truncate text-sm font-medium">{agentLabel(a)}</span>
                  {a.description ? (
                    <span className="block truncate text-xs text-muted-foreground">
                      {a.description}
                    </span>
                  ) : null}
                </span>
                <span className="flex shrink-0 items-center gap-1.5 text-xs text-muted-foreground">
                  <StatusDot running={a.status === 'running'} />
                  <span aria-hidden>{statusLabel(a.status)}</span>
                </span>
              </Button>
            </li>
          ))}
        </ul>
      )}
      {!directory.isPending && directory.isError && !agents.length ? (
        <div className="flex flex-wrap items-center gap-2 px-2 pt-2 text-sm">
          {copy.agentsFailed}{' '}
          <Button
            size="sm"
            variant="outline"
            className="pointer-coarse:min-h-11"
            onClick={directory.retry}
          >
            <RotateCw aria-hidden /> {copy.retry}
          </Button>
        </div>
      ) : !directory.isPending && !agents.length ? (
        <div className="space-y-1 px-2 pt-2 text-sm">
          <p className="font-medium">{copy.noRunningAgents}</p>
          <p className="text-muted-foreground">{copy.noRunningAgentsHint}</p>
        </div>
      ) : null}
      <div className="px-2 pt-2">
        <Link
          to="/agents"
          search={{}}
          className={cn(
            'inline-flex items-center text-sm text-primary-text underline-offset-4 hover:underline',
            TOUCH,
          )}
        >
          {copy.allAgents}
        </Link>
      </div>
    </section>
  )
}

/**
 * "Pick up where you left off" (§5.4): up to 3 recent chats for the target, in a lifted panel. When the panel is
 * already filtered to one target (`sameTarget`), rows show only the time: the name would repeat on every row.
 */
export function RecentChats({
  rows,
  identityOf,
  carry,
  sameTarget = false,
}: {
  rows: ChatSessionRow[]
  identityOf(r: ChatSessionRow): ChatIdentity
  carry: ChatCarry
  sameTarget?: boolean
}) {
  if (!rows.length) return null
  return (
    <section className={cn(LIFTED, 'p-3')} aria-labelledby="recent-chats">
      <h2 id="recent-chats" className="px-2 pb-2 text-sm font-medium">
        {copy.recentWithAgent}
      </h2>
      <ul className="divide-y divide-border">
        {rows.slice(0, MAX_RECENT).map((r) => {
          const id = identityOf(r)
          return (
            <li key={r.session_id}>
              <Link
                to="/chat/$sessionId"
                params={{ sessionId: r.session_id }}
                search={carry}
                className="flex items-center gap-3 rounded-md px-2 py-2 text-sm outline-none hover:bg-muted focus-visible:ring-2 focus-visible:ring-ring focus-visible:ring-offset-2 focus-visible:ring-offset-card pointer-coarse:min-h-11"
              >
                <IdentityIcon kind={id.kind} name={id.name} size={20} />
                <span className="min-w-0 flex-1 truncate" title={r.title}>
                  {r.title}
                </span>
                <span className="shrink-0 text-xs text-muted-foreground">
                  {sameTarget
                    ? relTime(r.updated_at)
                    : `${id.railLabel} · ${relTime(r.updated_at)}`}
                </span>
              </Link>
            </li>
          )
        })}
      </ul>
    </section>
  )
}

/** Several agents share an `?agent=` name (plan §6.11): pick one. */
export function AgentChooser({
  agents,
  title = copy.chooseTitle,
  hint = copy.chooseHint,
  carry,
}: {
  agents: Agent[]
  title?: string
  hint?: string
  carry: ChatCarry
}) {
  return (
    <div className="mx-auto flex max-w-3xl flex-col items-center gap-4 px-4 py-12 text-center">
      <div className="flex size-10 items-center justify-center rounded-full bg-muted">
        <MessagesSquare className="size-5" aria-hidden />
      </div>
      <div className="space-y-1">
        <h1 className="text-lg font-semibold">{title}</h1>
        {hint ? <p className="text-sm text-muted-foreground">{hint}</p> : null}
      </div>
      <ul className="flex flex-wrap justify-center gap-2">
        {agents.map((a, i) => (
          <li key={a.id}>
            {/* The first choice takes focus (§7.7). */}
            <Link
              to="/chat"
              search={{ ...carry, agent: a.id }}
              ref={i === 0 ? focusIfIdle : undefined}
              className="inline-flex items-center gap-2 rounded-full border border-border px-3 py-1.5 text-sm outline-none hover:bg-muted focus-visible:ring-2 focus-visible:ring-ring focus-visible:ring-offset-2 focus-visible:ring-offset-background pointer-coarse:min-h-11"
              title={a.description ?? undefined}
            >
              <StatusDot running={a.status === 'running'} />{' '}
              <span className="max-w-[24ch] truncate">{agentLabel(a)}</span>
            </Link>
          </li>
        ))}
      </ul>
      <Link
        to="/agents"
        search={{}}
        className="inline-flex items-center text-sm text-primary-text underline-offset-4 hover:underline pointer-coarse:min-h-11"
      >
        {copy.allAgents}
      </Link>
    </div>
  )
}

/** A problem with `?agent=` or the chat's agent: text, optional action, never a routed fallback. */
export function TargetBanner({ children, action }: { children: ReactNode; action?: ReactNode }) {
  return (
    <div
      role="note"
      className="mx-auto mt-4 flex w-full max-w-3xl flex-wrap items-center gap-3 rounded-md border border-warning/40 bg-warning/5 px-3 py-2 text-sm"
    >
      <span>{children}</span>
      {action}
    </div>
  )
}
