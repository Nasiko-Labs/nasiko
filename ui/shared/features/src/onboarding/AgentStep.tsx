/**
 * The Agent and key step's first half: prebuilt agents to deploy (up to MAX_SEEDED, the one with the best record
 * always included; meeting 2026-11-06, in place of chatting with a running one), then the Deploy page's own Registry,
 * GitHub and Upload tabs (that order, user request 2026-11-05). A started deploy stays in the guide (`onStarted`); the
 * background follower toasts when its build finishes (plans/feat-deploy.md eng R8).
 *
 * The guide's Continue deploys, not a button on the tab (user request 2026-11-09): the Prebuilt picks, or the mandatory
 * agent from any other tab, so the guide owns the open tab and the picks. An agent that already exists is shown as
 * deployed and never deployed again.
 */
import { useQuery } from '@tanstack/react-query'
import { Link } from '@tanstack/react-router'
import { Bot, ChevronRight, CircleCheck, ExternalLink } from 'lucide-react'
import { useId, useState } from 'react'
import { Alert, AlertDescription } from '@nasiko/ui/primitives/alert'
import { Badge } from '@nasiko/ui/primitives/badge'
import { asButton, withButtonStyle } from '@nasiko/ui/primitives/button'
import { Checkbox } from '@nasiko/ui/primitives/checkbox'
import { Skeleton } from '@nasiko/ui/primitives/skeleton'
import { Tabs, TabsContent, TabsList, TabsTrigger } from '@nasiko/ui/primitives/tabs'
import { cn } from '@nasiko/ui/lib/utils'
import { GithubTab } from '@nasiko/features/deploy/GithubTab'
import { RegistryTab } from '@nasiko/features/deploy/RegistryTab'
import type { DeploySearch } from '@nasiko/features/deploy/search'
import { type DeployStarted, UploadTab } from '@nasiko/features/deploy/UploadTab'
import { meQuery } from '@nasiko/data-access/auth'
import { copy } from './copy'
import { type AgentTab, MANDATORY, MAX_SEEDED, SEEDED_AGENTS, togglePick } from './logic'
import { SectionHeading } from './parts'
/** The mandatory agent leads the list it's always part of. */
const OFFERED = [
  ...SEEDED_AGENTS.filter((a) => a.name === MANDATORY),
  ...SEEDED_AGENTS.filter((a) => a.name !== MANDATORY),
]

export function AgentStep({
  started,
  onStarted,
  seeded,
  tab,
  onTab,
  picks,
  onPicks,
  existing,
  onLeave,
}: {
  started: DeployStarted | null
  onStarted: (to: DeployStarted) => void
  /** The prebuilt agents deployed from this guide, null until then (kept by the guide across steps). */
  seeded: readonly string[] | null
  /** The open tab: on Prebuilt, the guide's Continue deploys `picks`. */
  tab: AgentTab
  onTab: (tab: AgentTab) => void
  /** The Prebuilt picks not yet deployed, the mandatory agent always among them. */
  picks: readonly string[]
  onPicks: (picks: string[]) => void
  /** Agent names that already exist: shown as deployed, never picked. */
  existing: ReadonlySet<string>
  /** Following a link out of the guide closes it first. */
  onLeave: () => void
}) {
  const me = useQuery(meQuery).data
  // The Deploy page keeps the GitHub tab's picks in its URL; inside the guide they are local.
  const [search, setSearchState] = useState<DeploySearch>({ method: 'github' })
  const setSearch = (patch: Partial<DeploySearch>) => setSearchState((s) => ({ ...s, ...patch }))
  return (
    <section className="flex flex-col gap-4">
      <SectionHeading title={copy.agent.title} intro={copy.agent.intro} />
      {started ? (
        <Alert role="status">
          <CircleCheck aria-hidden />
          <AlertDescription className="flex flex-wrap items-center gap-x-3 gap-y-1">
            <span>{copy.agent.started}</span>
            {withButtonStyle(
              'buildId' in started ? (
                <Link to="/builds/$buildId" params={{ buildId: started.buildId }} onClick={onLeave}>
                  {copy.agent.openBuild}
                </Link>
              ) : (
                <Link
                  to="/agents/$agentId"
                  params={{ agentId: started.agentId }}
                  search={{}}
                  onClick={onLeave}
                >
                  {copy.agent.openAgent}
                </Link>
              ),
              { variant: 'link', size: 'sm', className: 'h-auto px-0' },
            )}
          </AlertDescription>
        </Alert>
      ) : !me ? (
        <Skeleton className="h-48 w-full" />
      ) : (
        <Tabs value={tab} onValueChange={(v) => onTab(v as AgentTab)} className="gap-4">
          {/* Sticks to the top of the guide's scrolling body (px-6 py-8 md:px-10, so -top-8), as on the Deploy page. */}
          <div className="sticky -top-8 z-20 -mx-6 bg-background px-6 py-2 md:-mx-10 md:px-10">
            <TabsList>
              <TabsTrigger value="seeded">{copy.agent.seeded}</TabsTrigger>
              <TabsTrigger value="registry">{copy.agent.registry}</TabsTrigger>
              <TabsTrigger value="github">{copy.agent.github}</TabsTrigger>
              <TabsTrigger value="upload">{copy.agent.upload}</TabsTrigger>
            </TabsList>
          </div>
          <TabsContent value="seeded">
            {seeded ? (
              <Alert role="status">
                <CircleCheck aria-hidden />
                <AlertDescription className="flex flex-wrap items-center gap-x-3 gap-y-1">
                  <span>{copy.agent.deploying(seeded)}</span>
                  <Link
                    to="/agents"
                    onClick={onLeave}
                    {...asButton({ variant: 'link', size: 'sm', className: 'h-auto px-0' })}
                  >
                    {copy.agent.openAgents}
                  </Link>
                </AlertDescription>
              </Alert>
            ) : (
              <Seeded picks={picks} onPicks={onPicks} existing={existing} />
            )}
          </TabsContent>
          <TabsContent value="registry" className="flex flex-col gap-4">
            <RegistryHow />
            <RegistryTab userId={me.sub} onStarted={onStarted} />
          </TabsContent>
          <TabsContent value="github">
            <GithubTab search={search} setSearch={setSearch} onStarted={onStarted} />
          </TabsContent>
          <TabsContent value="upload">
            <UploadTab userId={me.sub} prefill={{}} onStarted={onStarted} />
          </TabsContent>
        </Tabs>
      )}
      <div className="flex flex-wrap items-center gap-2 rounded-xl border bg-muted/40 px-4 py-3 text-xs text-muted-foreground">
        <span className="font-medium text-foreground">{copy.agent.lifecycle}</span>
        {copy.agent.stages.map((s, i) => (
          <span key={s} className="flex items-center gap-2">
            {i ? <ChevronRight aria-hidden className="size-3.5" /> : null}
            {s}
          </span>
        ))}
      </div>
    </section>
  )
}

/** Where a registry reference comes from: the public registry's Install section (user request 2026-11-09). */
function RegistryHow() {
  const how = copy.agent.registryHow
  return (
    <section
      aria-label={how.label}
      className="flex flex-col gap-2 rounded-xl border bg-muted/40 px-4 py-3 text-sm"
    >
      <p className="font-medium">{how.title}</p>
      <ol className="flex list-decimal flex-col gap-1 pl-5 text-muted-foreground">
        <li>
          {how.open}{' '}
          <a
            href={how.siteUrl}
            target="_blank"
            rel="noreferrer"
            className="inline-flex items-center gap-1 font-medium text-foreground underline underline-offset-4"
          >
            {how.site}
            <ExternalLink aria-hidden className="size-3" />
          </a>{' '}
          {how.openRest}
        </li>
        <li>
          {how.copyStep} <code className="font-mono text-xs text-foreground">{how.pull}</code>
          {', like '}
          <code className="font-mono text-xs break-all text-foreground">{how.example}</code>.
        </li>
        <li>{how.paste}</li>
      </ol>
    </section>
  )
}

/** The prebuilt agents as checkboxes, the mandatory one checked and locked; the guide's Continue deploys them. */
function Seeded({
  picks,
  onPicks,
  existing,
}: {
  picks: readonly string[]
  onPicks: (picks: string[]) => void
  existing: ReadonlySet<string>
}) {
  const id = useId()
  const full = picks.length >= MAX_SEEDED
  return (
    <fieldset className="flex flex-col gap-3">
      <legend className="sr-only">{copy.agent.seededLabel}</legend>
      <div className="flex flex-wrap items-baseline justify-between gap-x-3 gap-y-1 text-sm">
        <p className="text-muted-foreground">{copy.agent.seededLead(MAX_SEEDED)}</p>
        <p aria-live="polite" className="text-xs text-muted-foreground tabular-nums">
          {copy.agent.picked(picks.length, MAX_SEEDED)}
          {full ? ` · ${copy.agent.full}` : null}
        </p>
      </div>
      <ul className="grid gap-3 sm:grid-cols-2">
        {OFFERED.map((a, i) => {
          const cid = `${id}-seeded-${i}`
          const mandatory = a.name === MANDATORY
          const deployed = existing.has(a.name)
          const on = picks.includes(a.name)
          // Deployed already: nothing to pick. Full: a new pick swaps out the latest one.
          const locked = !mandatory && !on && deployed
          return (
            <li key={a.name}>
              {/* The Role step's card, a checkbox in place of its radio. */}
              <label
                htmlFor={cid}
                className={cn(
                  'flex h-full items-start gap-3 rounded-xl border bg-card p-4 has-[[data-checked]]:border-foreground/50 has-[[data-checked]]:bg-accent',
                  mandatory
                    ? 'cursor-default'
                    : locked
                      ? 'cursor-not-allowed'
                      : 'cursor-pointer hover:bg-accent/50',
                )}
              >
                <Checkbox
                  id={cid}
                  checked={on}
                  disabled={mandatory || locked}
                  // Locked on, not greyed out: every pick deploys it.
                  className={cn('mt-0.5', mandatory && 'disabled:opacity-100')}
                  onCheckedChange={() => onPicks(togglePick(picks, a.name, MANDATORY))}
                />
                <span className="flex size-9 shrink-0 items-center justify-center rounded-lg bg-muted">
                  <Bot aria-hidden className="size-4" />
                </span>
                <span className={cn('flex min-w-0 flex-col gap-1', locked && 'opacity-60')}>
                  <span className="flex flex-wrap items-center gap-x-2 gap-y-1 text-sm font-medium">
                    {a.title}
                    {mandatory ? <Badge variant="info">{copy.agent.included}</Badge> : null}
                    {deployed ? (
                      <Badge variant="secondary">{copy.agent.alreadyDeployed}</Badge>
                    ) : null}
                  </span>
                  <span className="text-xs text-muted-foreground">{a.line}</span>
                  <span className="text-xs text-muted-foreground tabular-nums">
                    {copy.agent.record(a.runs, a.success)}
                  </span>
                </span>
              </label>
            </li>
          )
        })}
      </ul>
    </fieldset>
  )
}
