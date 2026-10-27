import { Link, type HistoryState } from '@tanstack/react-router'
import { memo } from 'react'
import { Badge } from '@nasiko/ui/primitives/badge'
import { Card } from '@nasiko/ui/primitives/card'
import { copy } from '../copy'
import { displayStatus, isHarness } from '../status'
import { CARD_SKILLS } from '../tuning'
import type { Agent } from '../types'
import { AgentMark, StatusBadge } from './bits'

/** Memoized: every search keystroke re-renders the page, and the catalog can hold hundreds of cards. */
export const AgentCard = memo(function AgentCard({
  agent,
  meId,
  ownerName,
  reason,
  linkState,
}: {
  agent: Agent
  meId?: string
  ownerName?: string
  reason?: string
  /** The list this card was opened from, for the detail's Previous/Next (lib/siblings.ts). */
  linkState?: () => HistoryState
}) {
  const harness = isHarness(agent)
  const name = agent.display_name || agent.name
  const mine = agent.owner_id === meId
  const skills = agent.skills ?? []
  const extra = skills.length - CARD_SKILLS
  const owner = mine ? copy.you : ownerName
  return (
    // One link, no nested interactive elements (plan §7.1).
    <Card
      className="h-full gap-2 p-4 transition-[color,border-color,box-shadow] hover:border-primary/40 hover:shadow-lg focus-visible:outline-2 focus-visible:outline-ring"
      render={
        <Link to="/agents/$agentId" params={{ agentId: agent.id }} search={{}} state={linkState} />
      }
    >
      <div className="flex items-start gap-3">
        <AgentMark name={name} iconUrl={agent.icon_url} size={36} />
        <div className="min-w-0 flex-1">
          <div className="truncate font-medium">{name}</div>
          <div className="truncate font-mono text-xs text-muted-foreground">{agent.name}</div>
        </div>
        {harness ? null : (
          <StatusBadge display={displayStatus(agent.status, false)} raw={agent.status} />
        )}
      </div>
      <p
        className="line-clamp-1 text-sm text-muted-foreground"
        title={agent.description ?? undefined}
      >
        {agent.description || copy.noDescription}
      </p>
      {/* The health filter says why each agent is listed (overview design 15A; QA ISSUE-001). */}
      {reason ? (
        <p className="line-clamp-1 text-xs font-medium" data-testid="health-reason">
          {copy.healthReason(reason)}
        </p>
      ) : null}
      {skills.length ? (
        <div className="flex flex-wrap gap-1">
          {skills.slice(0, CARD_SKILLS).map((s) => (
            <Badge key={s.id} variant="secondary" className="font-normal">
              {s.name}
            </Badge>
          ))}
          {extra > 0 ? (
            <Badge variant="outline" className="font-normal">
              +{extra}
            </Badge>
          ) : null}
        </div>
      ) : null}
      <div className="mt-auto flex flex-wrap items-center gap-2 pt-1 text-xs text-muted-foreground">
        {harness ? (
          <Badge variant="outline">{copy.harnessChip}</Badge>
        ) : (
          <Badge variant="outline">{mine ? copy.yours : copy.availableToYou}</Badge>
        )}
        {owner && !mine ? <span>{owner}</span> : null}
      </div>
    </Card>
  )
})
