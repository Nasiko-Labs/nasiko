/**
 * The layers on this agent (plans/feat-agent-centric.md PR1c; design "Layer cards"): Routing, Tools and Optimization,
 * one figure each and a link to the page that owns it (Budget became its own section, `HubBudget.tsx`). A fixed three-slot row that hides the slots this viewer
 * can't read and reflows; a not-configured card says so and links to set it up.
 *
 * | Card | Owner / superuser | Others |
 * | Routing | model and config (`RoutingCard`'s rule: routing reads are owner/superuser only) | hidden |
 * | Tools | connectors and tools blocked by rule (configured, never observed blocks) | hidden (managers only) |
 * | Optimization | Token optimization, the owner's history strategy, window tokens saved | Token optimization only |
 */
import { useQuery } from '@tanstack/react-query'
import { Link } from '@tanstack/react-router'
import type { ReactNode } from 'react'
import { Card } from '@nasiko/ui/primitives/card'
import type { AgentView } from '@nasiko/features/agents/normalize'
import { useAgentBlockedTools, useAgentConnectors } from '@nasiko/features/mcp/api'
import { strategyQuery } from '@nasiko/features/optimization/api'
import { copy as optCopy } from '@nasiko/features/optimization/copy'
import { useAgentRouting } from '@nasiko/features/router/api'
import { configPin, routingSentence } from '@nasiko/features/router/routing'
import { meQuery } from '@nasiko/data-access/auth'
import { fmtTokens } from '@nasiko/ui/lib/format'
import { cn } from '@nasiko/ui/lib/utils'
import type { SavedTotal } from '../api'
import { copy } from '../copy'
import { TOOL_CARD_CONNECTORS } from '../tuning'

const LINK =
  'inline-flex items-center text-xs text-primary-text underline-offset-4 hover:underline pointer-coarse:min-h-11'

export function LayerCards({
  agent,
  saved,
  optimizationExtra,
}: {
  agent: AgentView
  /** This agent's tokens saved in the window (the hub's savings read); null when not reported. */
  saved: SavedTotal | null | undefined
  /** Rendered in the Optimization card: a harness owner's Token optimization switch (no Settings tab, eng D10). */
  optimizationExtra?: ReactNode
}) {
  const me = useQuery(meQuery).data
  const owner = !!me?.sub && me.sub === agent.ownerId
  const reader = owner || !!me?.is_superuser
  return (
    // Auto-fit: hidden slots leave no empty columns; the visible cards share the row (design "Layer cards").
    <section
      aria-label={copy.cards.title}
      className="grid grid-cols-[repeat(auto-fit,minmax(13rem,1fr))] gap-3"
    >
      {reader && !agent.isHarness ? <RoutingSlot id={agent.id} /> : null}
      {agent.canManage ? <ToolsSlot id={agent.id} /> : null}
      <OptimizationSlot agent={agent} owner={owner} saved={saved} extra={optimizationExtra} />
    </section>
  )
}

function Slot({
  title,
  children,
  badge,
  wide,
}: {
  title: string
  children: ReactNode
  badge?: ReactNode
  /** Two columns from `sm`: a card that carries a full control (a harness owner's switch). */
  wide?: boolean
}) {
  return (
    <Card className={cn('min-w-0 gap-1.5 p-3', wide && 'sm:col-span-2')}>
      <h3 className="m-0 flex items-center gap-2 text-xs font-medium text-muted-foreground">
        {title}
        {badge}
      </h3>
      {children}
    </Card>
  )
}

const Figure = ({ children }: { children: ReactNode }) => (
  <p className="m-0 truncate text-base font-semibold tabular-nums">{children}</p>
)
const Line = ({ children }: { children: ReactNode }) => (
  <p className="m-0 text-xs text-muted-foreground">{children}</p>
)

function RoutingSlot({ id }: { id: string }) {
  const routing = useAgentRouting(id, true)
  // A failed or forbidden read hides the card, as RoutingCard does.
  if (routing.isError) return null
  const r = routing.data
  const config = r?.llm_config ?? null
  // The router's own rules (routing.ts): an agent pin, else the config's pin, else its model; tiered configs have no
  // one model, so the figure is the routing sentence's short form (review: Codex).
  const pin = r?.pinned_model ?? (config ? configPin(config) : null)
  const figure = pin ?? config?.model ?? (config ? routingSentence(config, null).short : null)
  return (
    <Slot title={copy.cards.routing}>
      {routing.isPending ? (
        <Line>…</Line>
      ) : config ? (
        <>
          <Figure>{figure ?? '—'}</Figure>
          <Line>
            {config.name}
            {pin ? ` · ${copy.cards.routingPinned}` : ''}
          </Line>
          <Link to="/router" className={LINK}>
            {copy.cards.routingOpen}
          </Link>
        </>
      ) : (
        <>
          <Line>{copy.cards.routingNone}</Line>
          <Link to="/router" className={LINK}>
            {copy.cards.routingSetUp}
          </Link>
        </>
      )}
    </Slot>
  )
}

function ToolsSlot({ id }: { id: string }) {
  const connectors = useAgentConnectors(id, true)
  const enabled = (connectors.data ?? []).filter((c) => c.enabled)
  const checked = enabled.slice(0, TOOL_CARD_CONNECTORS).map((c) => c.connector_id)
  const blocked = useAgentBlockedTools(id, checked, checked.length > 0)
  if (connectors.isError) return null
  return (
    <Slot title={copy.cards.tools}>
      {connectors.isPending ? (
        <Line>…</Line>
      ) : enabled.length ? (
        <>
          <Figure>{copy.cards.toolsConnected(enabled.length)}</Figure>
          {!blocked.pending && !blocked.failed ? (
            <Line>{copy.cards.toolsBlocked(blocked.blocked, enabled.length > checked.length)}</Line>
          ) : null}
        </>
      ) : (
        <Line>{copy.cards.toolsNone}</Line>
      )}
      <Link to="/agents/$agentId" params={{ agentId: id }} search={{ tab: 'mcp' }} className={LINK}>
        {enabled.length ? copy.cards.toolsOpen : copy.cards.toolsSetUp}
      </Link>
    </Slot>
  )
}

function OptimizationSlot({
  agent,
  owner,
  saved,
  extra,
}: {
  agent: AgentView
  owner: boolean
  saved: SavedTotal | null | undefined
  extra?: ReactNode
}) {
  // The history strategy is the owner's own preference (it applies to their chats), so only the owner reads it.
  const strategy = useQuery({ ...strategyQuery, enabled: owner })
  return (
    <Slot title={copy.cards.optimization} wide={!!extra}>
      <Figure>{agent.compressEnabled ? copy.cards.compressOn : copy.cards.compressOff}</Figure>
      {owner && strategy.data ? (
        <Line>{copy.cards.historyBy(optCopy.strategies[strategy.data.strategy].name)}</Line>
      ) : null}
      {owner && saved ? (
        <Line>
          <span title={copy.lanes.savedTooltip}>
            {copy.cards.savedIn(fmtTokens(saved.saved), saved.basis)}
          </span>
        </Line>
      ) : null}
      {extra}
      <Link to="/optimization" className={LINK}>
        {copy.cards.optimizationOpen}
      </Link>
    </Slot>
  )
}
