/**
 * Overview: the agent hub (plans/feat-agent-centric.md PR1, design "Hub section order"): a summary of what the agent is
 * (NAS-1451), the crash card when the agent needs attention (field-presence rule unchanged), a deploy prompt when it
 * isn't deployed, then the hub itself (Needs you, the sentence, the timeline, recent runs, layer cards). Today's facts
 * moved to the Details tab (PR1c); the summary links there.
 */
import { useQuery } from '@tanstack/react-query'
import { Button } from '@nasiko/ui/primitives/button'
import { Skeleton } from '@nasiko/ui/primitives/skeleton'
import { meQuery } from '@nasiko/data-access/auth'
import { HubTimeline } from '@nasiko/features/hub/components/HubTimeline'
import { useAgentRouting } from '@nasiko/features/router/api'
import { copy as routerCopy } from '@nasiko/features/router/copy'
import { keySource, routingSentence, sourceLabel } from '@nasiko/features/router/routing'
import { isUnavailable, useDeployment, useErrorLogs } from '../api'
import { Description, Section } from '../components/bits'
import { copy } from '../copy'
import { relTime } from '../format'
import type { AgentView } from '../normalize'
import type { DisplayStatus } from '../status'
import { CRASH_LINES_SHOWN } from '../tuning'
import { NotDeployedPrompt } from './NotDeployedPrompt'
import { TokenOptimization } from './TokenOptimization'

export function OverviewTab({
  agent,
  display,
  raw,
  ownerLabel,
  onViewLogs,
  onViewDetails,
}: {
  agent: AgentView
  display: DisplayStatus
  raw?: string
  ownerLabel: string
  onViewLogs: () => void
  onViewDetails: () => void
}) {
  const deployment = useDeployment(agent.id, !agent.isHarness)
  const errors = useErrorLogs(agent.id, display === 'attention')
  const dep = deployment.data
  const depRow = dep && !isUnavailable(dep) ? dep : null
  // Needs attention covers `crashed` and `failed`; a failed deploy isn't a crash.
  const failed = raw === 'failed'
  const cardTitle = failed ? copy.failedTitle : copy.crashTitle

  return (
    <div className="space-y-4">
      <Summary agent={agent} onViewDetails={onViewDetails} />

      {display === 'attention' ? (
        <section
          aria-label={cardTitle}
          className="space-y-2 rounded-lg border border-warning/50 bg-warning/5 p-4 text-sm"
        >
          <h2 className="font-semibold">{cardTitle}</h2>
          {depRow?.crash_reason || depRow?.crashed_at || depRow?.last_logs ? (
            <div className="space-y-1">
              {depRow.crash_reason ? <p className="font-medium">{depRow.crash_reason}</p> : null}
              {depRow.crashed_at ? (
                <p className="text-muted-foreground">{relTime(depRow.crashed_at)}</p>
              ) : null}
              {depRow.last_logs ? (
                <pre className="max-h-40 overflow-auto rounded bg-muted p-2 font-mono text-xs whitespace-pre-wrap">
                  {depRow.last_logs}
                </pre>
              ) : null}
            </div>
          ) : errors.isPending ? (
            <Skeleton className="h-12" />
          ) : (
            <div className="space-y-1">
              <p className="text-muted-foreground">{failed ? copy.failedOss : copy.crashOss}</p>
              {errors.data?.length ? (
                <ul className="space-y-0.5 font-mono text-xs">
                  {errors.data.slice(0, CRASH_LINES_SHOWN).map((l) => (
                    <li key={`${l.timestamp}|${l.message}`}>
                      {l.timestamp.slice(11, 19)} {l.message}
                    </li>
                  ))}
                </ul>
              ) : (
                <p className="text-xs">{copy.crashNoLines}</p>
              )}
            </div>
          )}
          <Button variant="link" onClick={onViewLogs} className="h-auto p-0 text-sm">
            {copy.viewAllLogs}
          </Button>
        </section>
      ) : null}

      {display === 'not-deployed' && !agent.isHarness ? (
        <Section title={copy.activeVersion}>
          <NotDeployedPrompt name={agent.name} />
        </Section>
      ) : null}

      <HubTimeline
        agent={agent}
        ownerLabel={ownerLabel}
        // A harness has no Settings tab: its owner's Token optimization switch sits on the Optimization card (eng D10).
        optimizationExtra={
          agent.isHarness && agent.canManage ? <TokenOptimization agent={agent} /> : undefined
        }
      />
    </div>
  )
}

/** What the agent does (its description, three lines until Show more) and, when it has one, how its model calls are routed. */
function Summary({ agent, onViewDetails }: { agent: AgentView; onViewDetails: () => void }) {
  return (
    <Section
      title={copy.overviewSummary}
      action={
        <Button variant="link" onClick={onViewDetails} className="h-auto p-0 text-sm">
          {copy.allDetails} →
        </Button>
      }
    >
      <Description text={agent.description || copy.noDescription} lines={3} />
      {/* The Routing layer card's rule: a harness's routing lives on its own page. */}
      {agent.isHarness ? null : <RoutingFacts agent={agent} />}
    </Section>
  )
}

/**
 * Laid out as Details' facts (label, then value): the routing sentence with its config and source beneath, then whose
 * key pays. Shown only when routing is configured, and only to who may read it (`RoutingCard`'s rule: the owner or a
 * superuser).
 */
function RoutingFacts({ agent }: { agent: AgentView }) {
  const me = useQuery(meQuery).data
  const owner = !!me?.sub && me.sub === agent.ownerId
  const reader = owner || !!me?.is_superuser
  const r = useAgentRouting(agent.id, reader).data
  if (!reader || !r || (!r.llm_config && !r.pinned_model)) return null
  const key = keySource(r.llm_config)
  const from = sourceLabel(r.source, owner)
  return (
    <dl className="grid grid-cols-[auto_minmax(0,1fr)] gap-x-4 gap-y-2 border-t pt-3 text-sm">
      <dt className="text-muted-foreground">{routerCopy.cardTitle}</dt>
      <dd className="min-w-0">
        {routingSentence(r.llm_config, r.pinned_model).full}
        <span className="block text-xs text-muted-foreground">
          {r.llm_config ? `${r.llm_config.name} · ${from}` : from}
        </span>
      </dd>
      <dt className="text-muted-foreground">{routerCopy.colKey}</dt>
      <dd className="min-w-0">
        {key.kind === 'user' ? routerCopy.yourKey(key.secret) : routerCopy.platformKey}
      </dd>
    </dl>
  )
}
