/**
 * Overview: the agent hub (plans/feat-agent-centric.md PR1, design "Hub section order"): the crash card when the agent
 * needs attention (field-presence rule unchanged), a deploy prompt when it isn't deployed, then the hub itself (Needs
 * you, the sentence, the timeline, recent runs, layer cards). Today's facts moved to the Details tab (PR1c).
 */
import { Button } from '@nasiko/ui/primitives/button'
import { Skeleton } from '@nasiko/ui/primitives/skeleton'
import { HubTimeline } from '@nasiko/features/hub/components/HubTimeline'
import { isUnavailable, useDeployment, useErrorLogs } from '../api'
import { Section } from '../components/bits'
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
}: {
  agent: AgentView
  display: DisplayStatus
  raw?: string
  ownerLabel: string
  onViewLogs: () => void
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
