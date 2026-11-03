/**
 * Details (plans/feat-agent-centric.md PR1c "Where today's Overview tab goes"): the facts the Overview carried before
 * the agent hub took it over. What the agent does (description, skills, how to call it) is the main column, with the
 * facts (routing, active version, technical details) in a narrow column beside it; the live card (fetched only when
 * opened) and the raw responses last. Owner-controlled text renders as text only.
 */
import { useQuery } from '@tanstack/react-query'
import { useId, useState, type ReactNode } from 'react'
import { Button } from '@nasiko/ui/primitives/button'
import { Card } from '@nasiko/ui/primitives/card'
import { Skeleton } from '@nasiko/ui/primitives/skeleton'
import { cn } from '@nasiko/ui/lib/utils'
import { Disclosure } from '@nasiko/ui/composites/disclosure'
import { RoutingCard } from '@nasiko/features/router/components/RoutingCard'
import { agentDetailQuery, isUnavailable, useDeployment, useLiveCard, useVersions } from '../api'
import { CliCommand, LearnMore, Section } from '../components/bits'
import { CopyButton } from '@nasiko/ui/composites/copy-button'
import { relTime } from '../format'
import { copy } from '../copy'
import { exampleTexts, safeHttpUrl, type AgentView } from '../normalize'
import type { DisplayStatus } from '../status'
import { CLAMP_DESCRIPTION_CHARS, CLAMP_SKILL_CHARS, EXAMPLES_SHOWN, SKILLS_SHOWN } from '../tuning'
import { NotDeployedPrompt } from './NotDeployedPrompt'

export function DetailsTab({
  agent,
  display,
  ownerLabel,
}: {
  agent: AgentView
  display: DisplayStatus
  ownerLabel: string
}) {
  const deployment = useDeployment(agent.id, !agent.isHarness)
  const versions = useVersions(agent.id, !agent.isHarness)
  const active = versions.data?.find((v) => v.is_active)
  const dep = deployment.data
  const depRow = dep && !isUnavailable(dep) ? dep : null
  const docs = safeHttpUrl(agent.documentationUrl)

  return (
    <div className="space-y-4">
      {agent.canManage ? null : (
        <p className="text-sm text-muted-foreground">{copy.ownedBy(ownerLabel)}</p>
      )}

      {/* The story in the main column, the facts beside it (stacked below lg). Routing heads the facts so a long
          description never pushes "Change routing" out of reach. */}
      <div className="grid items-start gap-4 lg:grid-cols-[minmax(0,1fr)_20rem]">
        <div className="flex min-w-0 flex-col gap-4">
          <WhatItDoes agent={agent} />
          {/* The escape hatches, one card: Disclosure draws the rule between them. */}
          <Card className="gap-0 px-4 py-1">
            {agent.isHarness ? null : <LiveCard id={agent.id} />}
            <ViewRaw id={agent.id} deployment={dep} />
          </Card>
        </div>

        <div data-tour="agent-facts" className="flex min-w-0 flex-col gap-4">
          <RoutingCard
            agent={{
              id: agent.id,
              name: agent.name,
              displayName: agent.displayName,
              ownerId: agent.ownerId,
            }}
          />
          <Card className="gap-0 p-0">
            {agent.isHarness ? null : (
              <Facts title={copy.activeVersion} action={<LearnMore href="versions" />}>
                {deployment.isPending || versions.isPending ? (
                  <Skeleton className="h-24" />
                ) : isUnavailable(dep) ? (
                  <p className="text-sm text-muted-foreground">{copy.deploymentUnavailable}</p>
                ) : display === 'not-deployed' ? (
                  <NotDeployedPrompt name={agent.name} />
                ) : (
                  <dl className={FACTS}>
                    <Row label={copy.version} value={active?.version ?? agent.version} mono />
                    <Row label={copy.deployed} value={active ? relTime(active.created_at) : '—'} />
                    <Row label={copy.image} value={active?.image_tag ?? '—'} mono />
                    <Row label={copy.serviceUrl} value={depRow?.service_url ?? '—'} mono />
                  </dl>
                )}
              </Facts>
            )}
            <Facts title={copy.technical} action={<LearnMore href="registry" />}>
              <dl className={FACTS}>
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
                {docs ? (
                  <>
                    <dt className="text-muted-foreground">{copy.tech.documentation}</dt>
                    <dd className="min-w-0 break-all">
                      <a
                        href={docs}
                        target="_blank"
                        rel="noreferrer noopener"
                        className="text-primary-text underline-offset-4 hover:underline"
                      >
                        {docs}
                      </a>
                    </dd>
                  </>
                ) : null}
              </dl>
            </Facts>
          </Card>
        </div>
      </div>
    </div>
  )
}

/**
 * Description, skills and the call command. Long cards (an agent's own prose can run to paragraphs) start trimmed:
 * the description to a few lines, the first skills, each skill's first examples; one toggle shows it all.
 */
function WhatItDoes({ agent }: { agent: AgentView }) {
  const [expanded, setExpanded] = useState(false)
  const long = (t: string | null | undefined, max: number) => (t?.length ?? 0) > max
  const trimmable =
    long(agent.description, CLAMP_DESCRIPTION_CHARS) ||
    agent.skills.length > SKILLS_SHOWN ||
    agent.skills.some(
      (s) => long(s.description, CLAMP_SKILL_CHARS) || exampleTexts(s).length > EXAMPLES_SHOWN,
    )
  const trim = trimmable && !expanded
  const skills = trim ? agent.skills.slice(0, SKILLS_SHOWN) : agent.skills
  return (
    <Section title={copy.whatItDoes}>
      <p className={cn('max-w-prose text-sm whitespace-pre-wrap', trim && 'line-clamp-4')}>
        {agent.description || copy.noDescription}
      </p>
      <h3 className="pt-2 text-sm font-semibold">
        {copy.skills}
        {agent.skills.length ? (
          <span className="ml-1.5 font-normal text-muted-foreground tabular-nums">
            {agent.skills.length}
          </span>
        ) : null}
      </h3>
      {agent.skills.length ? (
        <ul className="divide-y rounded-md border">
          {skills.map((s) => {
            const examples = exampleTexts(s)
            return (
              <li key={s.id} className="space-y-1 px-3 py-2.5 text-sm">
                <div className="font-medium">{s.name}</div>
                {/* Cards often repeat the name as the description; say it once. */}
                {s.description && !sameText(s.description, s.name) ? (
                  <div className={cn('max-w-prose text-muted-foreground', trim && 'line-clamp-2')}>
                    {s.description}
                  </div>
                ) : null}
                {examples.length ? (
                  <div className="flex flex-wrap gap-x-3 gap-y-1">
                    {(trim ? examples.slice(0, EXAMPLES_SHOWN) : examples).map((ex) => (
                      <div key={ex} className="flex items-center gap-1 text-xs">
                        <span className="rounded bg-muted px-1.5 py-0.5">“{ex}”</span>
                        <CopyButton text={ex} label={copy.copyExample(ex)} />
                      </div>
                    ))}
                  </div>
                ) : null}
              </li>
            )
          })}
        </ul>
      ) : (
        <p className="text-sm text-muted-foreground">{copy.noSkills}</p>
      )}
      {trimmable ? (
        <Button
          variant="link"
          size="sm"
          aria-expanded={expanded}
          onClick={() => setExpanded((e) => !e)}
          className="h-auto self-start p-0 text-sm pointer-coarse:min-h-11"
        >
          {expanded
            ? copy.showLess
            : agent.skills.length > SKILLS_SHOWN
              ? copy.showAllSkills(agent.skills.length)
              : copy.showMore}
        </Button>
      ) : null}
      {agent.isHarness ? null : (
        <div className="mt-2 space-y-1.5 border-t pt-3">
          <h3 className="text-sm font-semibold">{copy.callThisAgent}</h3>
          <CliCommand command={`nasiko chat --agent ${agent.id} "hello"`} />
        </div>
      )}
    </Section>
  )
}

const yesNo = (v: boolean) => (v ? copy.yes : copy.no)

/** "Analyse a funnel" and "Analyse a funnel." say the same thing. */
const sameText = (a: string, b: string) => {
  const norm = (t: string) =>
    t
      .trim()
      .replace(/[.!]+$/, '')
      .toLowerCase()
  return norm(a) === norm(b)
}

const FACTS = 'grid grid-cols-[auto_minmax(0,1fr)] gap-x-4 gap-y-2 text-sm'

/** One labelled group in the facts column; groups share a card and are split by a rule. */
function Facts({
  title,
  action,
  children,
}: {
  title: string
  action?: ReactNode
  children: ReactNode
}) {
  const id = useId()
  return (
    <section aria-labelledby={id} className="space-y-3 border-t p-4 first:border-t-0">
      <div className="flex items-center justify-between gap-2">
        <h2 id={id} className="text-sm font-semibold">
          {title}
        </h2>
        {action}
      </div>
      {children}
    </section>
  )
}

function Row({ label, value, mono }: { label: string; value: string; mono?: boolean }) {
  return (
    <>
      <dt className="text-muted-foreground">{label}</dt>
      <dd className={mono ? 'min-w-0 font-mono text-xs leading-5 break-words' : 'min-w-0'}>
        {value}
      </dd>
    </>
  )
}

function LiveCard({ id }: { id: string }) {
  const [open, setOpen] = useState(false)
  const card = useLiveCard(id, open)
  return (
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
        <JsonBlock value={card.data} label={copy.copyLiveCard} className="max-h-80" />
      )}
    </Disclosure>
  )
}

/** The unnormalized responses, as text (an escape hatch when the view model looks wrong). */
function ViewRaw({ id, deployment }: { id: string; deployment: unknown }) {
  const raw = useQuery(agentDetailQuery(id)).data
  const [open, setOpen] = useState(false)
  return (
    <Disclosure
      id="agent-view-raw"
      title={copy.viewRaw}
      open={open}
      onToggle={() => setOpen((o) => !o)}
    >
      <JsonBlock
        value={{ detail: raw ?? null, deployment: deployment ?? null }}
        label={copy.copyRaw}
        className="max-h-96"
      />
    </Disclosure>
  )
}

/** Pretty-printed JSON with a copy button pinned to its top-right corner. */
function JsonBlock({
  value,
  label,
  className,
}: {
  value: unknown
  label: string
  className?: string
}) {
  const text = JSON.stringify(value, null, 2)
  return (
    <div className="relative mb-3">
      <pre className={cn('overflow-auto rounded bg-muted p-3 pr-12 font-mono text-xs', className)}>
        {text}
      </pre>
      <CopyButton text={text} label={label} className="absolute top-1.5 right-1.5" />
    </div>
  )
}
