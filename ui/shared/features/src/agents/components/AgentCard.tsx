// Ported from the legacy ui/ on development (3aac84f66) common/src/features/agents/components/AgentCard.tsx (split out
// of CatalogPage.tsx, d2e0d4bcf, so GenUI's agent cards reuse it). Local edits: the React CatalogPage's card as it
// was (`Card render={<Link …/>}` through the navigation port, optional props admit `undefined`); Prettier formatting.
import Link from "@nasiko/app-core/navigation";
import { memo } from "react";
import { Badge } from "@nasiko/ui/primitives";
import { Card } from "@nasiko/ui/primitives";
import { copy } from "../copy";
import { displayStatus, isHarness } from "../status";
import { CARD_SKILLS } from "../tuning";
import type { Agent } from "../types";
import { AgentMark, StatusBadge } from "./bits";

/** Memoized: every search keystroke re-renders the page, and the catalog can hold hundreds of cards. */
export const AgentCard = memo(function AgentCard({
  agent,
  meId,
  ownerName,
  reason,
}: {
  agent: Agent;
  meId?: string | undefined;
  ownerName?: string | undefined;
  reason?: string | undefined;
}) {
  const harness = isHarness(agent);
  const name = agent.display_name || agent.name;
  const mine = agent.owner_id === meId;
  // eslint-disable-next-line @typescript-eslint/no-unnecessary-condition -- defensive read of server data (the lab's)
  const skills = agent.skills ?? [];
  const extra = skills.length - CARD_SKILLS;
  const owner = mine ? copy.you : ownerName;
  return (
    // One link, no nested interactive elements (plan §7.1).
    <Card
      render={<Link href={`/agents/${encodeURIComponent(agent.id)}`} />}
      className="h-full gap-2 p-4 transition-[color,border-color,box-shadow] hover:border-primary/40 hover:shadow-lg focus-visible:outline-2 focus-visible:outline-ring"
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
      {/* The Overview's health filter says why each agent is listed (overview design 15A; QA ISSUE-001). */}
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
  );
});
