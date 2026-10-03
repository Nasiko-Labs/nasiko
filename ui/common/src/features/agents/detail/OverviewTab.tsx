/**
 * Overview (plan §7.3): crash card first when the agent needs attention, then the active
 * version, what it does, skills, how to call it, technical details, the live card (fetched
 * only when opened) and the raw responses. Owner-controlled text renders as text only.
 */
import { useQuery } from '@tanstack/react-query'
import { useState } from 'react'
import { Button } from '@/components/ui/button'
import { Card } from '@/components/ui/card'
import { Skeleton } from '@/components/ui/skeleton'
import { Disclosure } from '@/components/shared/disclosure'
import { RoutingCard } from '@/features/router/components/RoutingCard'
import {
  agentDetailQuery,
  isUnavailable,
  useDeployment,
  useErrorLogs,
  useLiveCard,
  useVersions,
} from '../api'
import { CliCommand, LearnMore, Section, TryItLink } from '../components/bits'
import { CopyButton } from '@/components/shared/copy-button'
import { DEPLOY_CMD, relTime } from '../format'
import { copy } from '../copy'
import { exampleTexts, safeHttpUrl, type AgentView } from '../normalize'
import type { DisplayStatus } from '../status'
import { CRASH_LINES_SHOWN } from '../tuning'
import { DeployAgentButton } from '@/features/deploy/components/DeployAgentButton'
import { copy as deployCopy } from '@/features/deploy/copy'

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
  const versions = useVersions(agent.id, !agent.isHarness)
  const errors = useErrorLogs(agent.id, display === 'attention')
  const active = versions.data?.find((v) => v.is_active)
  const dep = deployment.data
  const depRow = dep && !isUnavailable(dep) ? dep : null
  const docs = safeHttpUrl(agent.documentationUrl)
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

      {agent.isHarness ? null : (
        <Section title={copy.activeVersion} action={<LearnMore href="versions" />}>
          {deployment.isPending || versions.isPending ? (
            <Skeleton className="h-12" />
          ) : isUnavailable(dep) ? (
            <p className="text-sm text-muted-foreground">{copy.deploymentUnavailable}</p>
          ) : display === 'not-deployed' ? (
            <div className="flex flex-col items-start gap-2 text-sm">
              <p className="text-muted-foreground">{copy.notDeployed}.</p>
              <DeployAgentButton name={agent.name} />
              <p className="text-xs text-muted-foreground">{deployCopy.entry.orCli}</p>
              <CliCommand command={DEPLOY_CMD} />
            </div>
          ) : (
            <dl className="grid gap-x-6 gap-y-2 text-sm sm:grid-cols-4">
              <Field label={copy.version} value={active?.version ?? agent.version} mono />
              <Field label={copy.deployed} value={active ? relTime(active.created_at) : '—'} />
              <Field label={copy.image} value={active?.image_tag ?? '—'} mono />
              <Field label={copy.serviceUrl} value={depRow?.service_url ?? '—'} mono />
            </dl>
          )}
        </Section>
      )}

      <RoutingCard
        agent={{
          id: agent.id,
          name: agent.name,
          displayName: agent.displayName,
          ownerId: agent.ownerId,
        }}
      />

      {agent.canManage ? null : (
        <p className="text-sm text-muted-foreground">{copy.ownedBy(ownerLabel)}</p>
      )}

      <div className="grid gap-4 lg:grid-cols-2">
        <Section title={copy.whatItDoes}>
          <p className="text-sm whitespace-pre-wrap">{agent.description || copy.noDescription}</p>
          <h3 className="pt-2 text-sm font-semibold">{copy.skills}</h3>
          {agent.skills.length ? (
            <ul className="space-y-3">
              {agent.skills.map((s) => (
                <li key={s.id} className="text-sm">
                  <div className="font-medium">{s.name}</div>
                  {s.description ? (
                    <div className="text-muted-foreground">{s.description}</div>
                  ) : null}
                  {exampleTexts(s).map((ex) => (
                    <div key={ex} className="mt-1 flex items-center gap-1 text-xs">
                      <span className="rounded bg-muted px-1.5 py-0.5">“{ex}”</span>
                      <CopyButton text={ex} label={copy.copyExample(ex)} />
                    </div>
                  ))}
                </li>
              ))}
            </ul>
          ) : (
            <p className="text-sm text-muted-foreground">{copy.noSkills}</p>
          )}
          {agent.isHarness ? null : (
            <div className="space-y-1 pt-2">
              <h3 className="text-sm font-semibold">{copy.callThisAgent}</h3>
              <TryItLink
                id={agent.id}
                name={agent.displayName}
                display={display}
                harness={agent.isHarness}
                size="xs"
              />
              <CliCommand command={`nasiko chat --agent ${agent.id} "hello"`} />
            </div>
          )}
        </Section>

        <Section title={copy.technical} action={<LearnMore href="registry" />}>
          <dl className="grid grid-cols-[auto_1fr] gap-x-6 gap-y-1.5 text-sm">
            <Row label={copy.tech.streaming} value={yesNo(agent.capabilities.streaming)} />
            <Row label={copy.tech.push} value={yesNo(agent.capabilities.pushNotifications)} />
            <Row
              label={copy.tech.history}
              value={yesNo(agent.capabilities.stateTransitionHistory)}
            />
            <Row label={copy.tech.inputModes} value={agent.inputModes.join(', ') || '—'} />
            <Row label={copy.tech.outputModes} value={agent.outputModes.join(', ') || '—'} />
            <Row label={copy.tech.protocol} value={agent.protocolVersion || '—'} />
            <Row label={copy.tech.transport} value={agent.preferredTransport || '—'} />
            {agent.integrationId ? (
              <Row label={copy.tech.harness} value={agent.integrationId} />
            ) : null}
            <dt className="text-muted-foreground">{copy.tech.documentation}</dt>
            <dd>
              {docs ? (
                <a
                  href={docs}
                  target="_blank"
                  rel="noreferrer noopener"
                  className="text-primary-text underline-offset-4 hover:underline"
                >
                  {docs}
                </a>
              ) : (
                '—'
              )}
            </dd>
          </dl>
        </Section>
      </div>

      {agent.isHarness ? null : <LiveCard id={agent.id} />}
      <ViewRaw id={agent.id} deployment={dep} />
    </div>
  )
}

const yesNo = (v: boolean) => (v ? copy.yes : copy.no)

function Field({ label, value, mono }: { label: string; value: string; mono?: boolean }) {
  return (
    <div className="min-w-0">
      <dt className="text-xs text-muted-foreground">{label}</dt>
      <dd className={mono ? 'truncate font-mono text-xs' : ''} title={value}>
        {value}
      </dd>
    </div>
  )
}

function Row({ label, value }: { label: string; value: string }) {
  return (
    <>
      <dt className="text-muted-foreground">{label}</dt>
      <dd>{value}</dd>
    </>
  )
}

function LiveCard({ id }: { id: string }) {
  const [open, setOpen] = useState(false)
  const card = useLiveCard(id, open)
  return (
    <Card className="gap-0 px-4 py-1">
      <Disclosure
        id="agent-live-card"
        title={copy.liveCard}
        open={open}
        onToggle={() => setOpen((o) => !o)}
      >
        {card.isPending ? (
          <Skeleton className="h-16" />
        ) : card.isError ? (
          <p className="text-sm text-muted-foreground">{copy.liveCardUnavailable}</p>
        ) : (
          <pre className="max-h-80 overflow-auto rounded bg-muted p-3 font-mono text-xs">
            {JSON.stringify(card.data, null, 2)}
          </pre>
        )}
      </Disclosure>
    </Card>
  )
}

/** The unnormalized responses, as text (an escape hatch when the view model looks wrong). */
function ViewRaw({ id, deployment }: { id: string; deployment: unknown }) {
  const raw = useQuery(agentDetailQuery(id)).data
  const [open, setOpen] = useState(false)
  return (
    <Card className="gap-0 px-4 py-1">
      <Disclosure
        id="agent-view-raw"
        title={copy.viewRaw}
        open={open}
        onToggle={() => setOpen((o) => !o)}
      >
        <pre className="max-h-96 overflow-auto rounded bg-muted p-3 font-mono text-xs">
          {JSON.stringify({ detail: raw ?? null, deployment: deployment ?? null }, null, 2)}
        </pre>
      </Disclosure>
    </Card>
  )
}
