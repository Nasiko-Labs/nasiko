/**
 * The Agent and key step's first half: the agents every new workspace is given (user request 2026-11-12: listed, not
 * picked, and Continue deploys nothing), then the Deploy page's own Registry, GitHub and Upload tabs (that order, user
 * request 2026-11-05). A started deploy stays in the guide (`onStarted`); the background follower toasts when its build
 * finishes (plans/feat-deploy.md eng R8).
 *
 * The Starter pack is offered only where the edition's `starterPack` slot says so (user request 2026-11-12: the MT layer
 * offers it on the Free plan, so an upgraded workspace starts clean, on the Registry tab, and brings its own agents).
 *
 * Every deploy started from the tabs is followed live under them (NAS-1450): a clock while it runs, then its outcome
 * (guideDeploys.ts).
 */
import { useQuery } from '@tanstack/react-query'
import { Link } from '@tanstack/react-router'
import { Bot, ChevronRight, CircleCheck, ExternalLink, Gift } from 'lucide-react'
import { useEffect, useState } from 'react'
import { Alert, AlertDescription } from '@nasiko/ui/primitives/alert'
import { asButton, withButtonStyle } from '@nasiko/ui/primitives/button'
import { Skeleton } from '@nasiko/ui/primitives/skeleton'
import { Tabs, TabsContent, TabsList, TabsTrigger } from '@nasiko/ui/primitives/tabs'
import { cn } from '@nasiko/ui/lib/utils'
import { useSlots } from '@nasiko/app-core/edition/edition-context'
import { useNow } from '@nasiko/app-core/useNow'
import { StageIcon } from '@nasiko/features/deploy/components/StageList'
import { copy as deployCopy } from '@nasiko/features/deploy/copy'
import { GithubTab } from '@nasiko/features/deploy/GithubTab'
import { RegistryTab } from '@nasiko/features/deploy/RegistryTab'
import type { DeploySearch } from '@nasiko/features/deploy/search'
import { fmtElapsed, type Stage } from '@nasiko/features/deploy/steps'
import { type DeployStarted, UploadTab } from '@nasiko/features/deploy/UploadTab'
import { meQuery } from '@nasiko/data-access/auth'
import { type SeededDeploy, useSeededDeploys, useStarterPackOffered } from './api'
import { watchGuideDeploys } from './guideDeploys'
import { copy } from './copy'
import { SEEDED_AGENTS } from './logic'
import { SectionHeading } from './parts'
export function AgentStep({
  started,
  onStarted,
  onLeave,
}: {
  started: DeployStarted | null
  onStarted: (to: DeployStarted) => void
  /** Following a link out of the guide closes it first. */
  onLeave: () => void
}) {
  const me = useQuery(meQuery).data
  const offered = useStarterPackOffered()
  // The Deploy page keeps the GitHub tab's picks in its URL; inside the guide they are local.
  const [search, setSearchState] = useState<DeploySearch>({ method: 'github' })
  const setSearch = (patch: Partial<DeploySearch>) => setSearchState((s) => ({ ...s, ...patch }))
  // Every deploy started from these tabs in this page (the store outlives the dialog), each followed live.
  useEffect(() => {
    if (me) watchGuideDeploys(me.sub)
  }, [me])
  const deploys = useSeededDeploys()
  const progress = deploys.size ? <SeededProgress deploys={deploys} onLeave={onLeave} /> : null
  return (
    <section className="flex flex-col gap-4">
      <SectionHeading
        title={copy.agent.title}
        intro={offered === false ? copy.agent.introOwn : copy.agent.intro}
      />
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
      ) : !me || offered === null ? (
        <Skeleton className="h-48 w-full" />
      ) : (
        <Tabs defaultValue={offered ? 'seeded' : 'registry'} className="gap-4">
          {/* Sticks to the top of the guide's scrolling body (px-6 py-8 md:px-10, so -top-8), as on the Deploy page. */}
          <div className="sticky -top-8 z-20 -mx-6 bg-background px-6 py-2 md:-mx-10 md:px-10">
            <TabsList>
              {offered ? <TabsTrigger value="seeded">{copy.agent.seeded}</TabsTrigger> : null}
              <TabsTrigger value="registry">{copy.agent.registry}</TabsTrigger>
              <TabsTrigger value="github">{copy.agent.github}</TabsTrigger>
              <TabsTrigger value="upload">{copy.agent.upload}</TabsTrigger>
            </TabsList>
          </div>
          {offered ? (
            <TabsContent value="seeded">
              <Giveaway />
            </TabsContent>
          ) : null}
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
      {progress}
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

/** A deploy's phase as the build page's stage icon. */
const STAGE: Record<SeededDeploy['phase'], Stage['state']> = {
  deploying: 'current',
  running: 'done',
  notRunning: 'warning',
  failed: 'failed',
}
const LINK = asButton({ variant: 'link', size: 'sm', className: 'h-auto px-0' })

/** Each prebuilt agent the guide deployed: a clock while it deploys (the import says nothing until it ends, as on the
 *  Registry tab), then Running, not started, or why it failed, with where to look. The finish is announced by its toast,
 *  so the rows aren't a live region (the clock would be read out every second). */
function SeededProgress({
  deploys,
  onLeave,
}: {
  deploys: ReadonlyMap<string, SeededDeploy>
  onLeave: () => void
}) {
  const live = [...deploys.values()].some((d) => d.phase === 'deploying')
  // A seconds clock, as the Registry tab's.
  const now = useNow(live ? 1_000 : null)
  return (
    <div className="flex flex-col gap-2">
      <ul
        aria-label={copy.agent.progressLabel}
        className="flex flex-col divide-y rounded-xl border bg-card"
      >
        {[...deploys].map(([name, d]) => (
          <li
            key={name}
            data-phase={d.phase}
            className="flex flex-wrap items-center gap-x-3 gap-y-1 px-4 py-3"
          >
            <StageIcon state={STAGE[d.phase]} />
            <span className="flex min-w-0 flex-1 flex-col">
              <span className="text-sm font-medium">{d.title}</span>
              <span
                className={cn(
                  'text-xs text-muted-foreground tabular-nums',
                  d.phase === 'failed' && 'text-destructive',
                )}
              >
                {d.phase === 'deploying'
                  ? copy.agent.progress.deploying(
                      fmtElapsed(now - d.startedAt),
                      deployCopy.registry.usual,
                    )
                  : d.phase === 'failed'
                    ? copy.agent.progress.failed(d.reason)
                    : copy.agent.progress[d.phase]}
              </span>
            </span>
            {d.phase === 'running' || d.phase === 'notRunning' ? (
              <Link
                to="/agents/$agentId"
                params={{ agentId: d.agentId }}
                search={{}}
                onClick={onLeave}
                {...LINK}
              >
                {copy.agent.openAgent}
              </Link>
            ) : d.phase === 'failed' ? (
              <Link to="/agents" onClick={onLeave} {...LINK}>
                {copy.agent.openAgents}
              </Link>
            ) : null}
          </li>
        ))}
      </ul>
      {live ? <p className="text-xs text-muted-foreground">{copy.agent.keepGoing}</p> : null}
    </div>
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

/** The agents every new workspace is given, shown as a list: nothing to pick (user request 2026-11-12). The layer's
 *  note (MT: the Free plan's daily allowance) follows the intro, tinted so it stands out (user request 2026-11-16). */
function Giveaway() {
  const note = useSlots().starterPack?.note
  return (
    <section aria-label={copy.agent.seededLabel} className="flex flex-col gap-3">
      <p className="text-sm text-muted-foreground">{copy.agent.giveaway(SEEDED_AGENTS.length)}</p>
      {note ? (
        <Alert role="note" className="border-success/40 bg-success/5 [&>svg]:text-success">
          <Gift aria-hidden />
          <AlertDescription className="text-foreground">{note}</AlertDescription>
        </Alert>
      ) : null}
      <ul className="grid gap-3 sm:grid-cols-2">
        {SEEDED_AGENTS.map((a) => (
          <li key={a.name} className="flex h-full items-start gap-3 rounded-xl border bg-card p-4">
            <span className="flex size-9 shrink-0 items-center justify-center rounded-lg bg-muted">
              <Bot aria-hidden className="size-4" />
            </span>
            <span className="flex min-w-0 flex-col gap-1">
              <span className="text-sm font-medium">{a.title}</span>
              <span className="text-xs text-muted-foreground">{a.line}</span>
              <span className="text-xs text-muted-foreground tabular-nums">
                {copy.agent.record(a.runs, a.success)}
              </span>
            </span>
          </li>
        ))}
      </ul>
    </section>
  )
}
