/**
 * Biggest senders (plans/feat-optimization-page.md R1C, R7A, R7C; eng C4, C5): the requests that carried the most
 * history in the window, or in the chart bar you picked (its chip names the interval and clears it). Each row is the
 * chat, its agent and what that message carried: ~without → ~sent, plus "Token optimization off" for an agent
 * with it off. Your own chats open the trace at the request (`?trace=`, `?span=` when the server sends it); another user's chat
 * is "Another user's chat" with the agent and counts, no title and no link (C4). Loads and fails on its own (R2C).
 */
import { Link } from '@tanstack/react-router'
import { X } from 'lucide-react'
import { Panel, PanelError } from '@nasiko/ui/composites/panel'
import { Badge } from '@nasiko/ui/primitives/badge'
import { Button } from '@nasiko/ui/primitives/button'
import { Skeleton } from '@nasiko/ui/primitives/skeleton'
import { AgentLink } from '@nasiko/features/agents/components/AgentLink'
import { seriesAt } from '@nasiko/ui/lib/chart'
import { fmtUtcDayTime } from '@nasiko/ui/lib/format'
import type { Proposed } from '../api'
import { copy } from '../copy'
import { tokensShort } from '../lead'
import type { TopRequest } from '../types'
import { Outline } from './Outline'

const SENT = seriesAt(0)
const SAVED = seriesAt(1)

export function BiggestSenders({
  result,
  pending,
  error,
  onRetry,
  period,
  slice,
  onClear,
  nameOf,
  noAgents,
}: {
  result: Proposed<TopRequest[]> | undefined
  pending: boolean
  error: unknown
  onRetry: () => void
  period: string
  /** The picked bar's label (C5), or null for the whole window. */
  slice: string | null
  onClear: () => void
  /** The agent's display name (rows carry the raw trace name). */
  nameOf: (agentId: string, raw: string) => string
  /** R2E(2): you can access no agents, so the block shows what will show here. */
  noAgents: boolean
}) {
  const body = () => {
    if (!result && pending)
      return (
        <div aria-hidden className="flex flex-col gap-3">
          {[0, 1, 2].map((i) => (
            <Skeleton key={i} className="h-5 w-full max-w-lg" />
          ))}
        </div>
      )
    if (!result) return <PanelError error={error} onRetry={onRetry} what={copy.senders.what} />
    if (result.kind === 'absent' || noAgents) return <Outline small>{copy.senders.outline}</Outline>
    if (!result.data.length)
      return (
        <p className="m-0 text-sm text-muted-foreground">
          {slice ? copy.senders.emptySlice(slice) : copy.senders.empty}
        </p>
      )
    return (
      <ul className="m-0 -mx-4 flex list-none flex-col p-0">
        {result.data.map((r, i) => (
          <li
            // A redacted row has no trace id (C4), and two can match on agent, start and size: the position breaks the tie.
            // eslint-disable-next-line @eslint-react/no-array-index-key -- the list is one server ranking, replaced whole, never reordered in place
            key={r.trace_id ?? `${r.agent_id}-${r.started_at}-${r.pool_tokens}-${i}`}
            className="border-t border-border px-4 py-2.5 text-sm first:border-t-0"
          >
            <SentBar pool={r.pool_tokens} sent={r.sent_tokens} />
            <Chat r={r} />
            <span className="text-muted-foreground">
              {' · '}
              <AgentLink id={r.agent_id} name={r.agent_name}>
                {nameOf(r.agent_id, r.agent_name)}
              </AgentLink>{' '}
              <span className="tabular-nums">
                {copy.senders.row(tokensShort(r.pool_tokens), tokensShort(r.sent_tokens))}
              </span>
              {r.compress_enabled ? null : ` · ${copy.senders.off}`}
              {/* One chat can send several of the biggest requests: the time tells them apart (QA ISSUE-001). */}
              {` · ${copy.senders.at(fmtUtcDayTime(r.started_at))}`}
            </span>
          </li>
        ))}
      </ul>
    )
  }
  return (
    <Panel
      title={copy.senders.title}
      subtitle={`${slice ?? period} · ${copy.senders.perMessage}`}
      labelledBy="optimization-senders-h"
      actions={
        slice ? (
          <Badge variant="outline" className="gap-1 pr-0.5">
            {copy.lead.slice.selected(slice)}
            <Button
              variant="ghost"
              size="icon"
              className="size-5 pointer-coarse:size-11"
              aria-label={copy.lead.slice.clearLabel(slice)}
              onClick={onClear}
            >
              <X aria-hidden />
            </Button>
          </Badge>
        ) : null
      }
    >
      {body()}
    </Panel>
  )
}

function Chat({ r }: { r: TopRequest }) {
  if (!r.own) return <span className="font-medium">{copy.senders.otherChat}</span>
  const title = r.chat_title || copy.senders.untitled
  if (!r.session_id || !r.trace_id) return <span className="font-medium">{title}</span>
  return (
    <Link
      to="/sessions/$sessionId"
      params={{ sessionId: r.session_id }}
      search={{ trace: r.trace_id, span: r.span_id ?? undefined }}
      className="font-medium underline underline-offset-4 hover:text-primary-text"
    >
      {title}
    </Link>
  )
}

/** What this message carried against what it could have: sent solid, the rest in the chart's Saved colour. */
function SentBar({ pool, sent }: { pool: number; sent: number }) {
  if (pool <= 0) return null
  return (
    <span
      aria-hidden
      className="mb-1.5 flex h-1.5 w-full max-w-sm gap-0.5 overflow-hidden rounded-full"
    >
      <span
        className="h-full rounded-l-full"
        style={{ width: `${(Math.min(sent, pool) / pool) * 100}%`, backgroundColor: SENT.fill }}
      />
      <span className="h-full flex-1 rounded-r-full" style={{ backgroundColor: SAVED.fill }} />
    </span>
  )
}
