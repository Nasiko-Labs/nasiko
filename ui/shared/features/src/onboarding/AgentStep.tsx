/**
 * The Agent and key step's first half: the agents every new workspace is given (user request 2026-11-12: listed, not
 * picked, and Continue deploys nothing), then the Deploy page's own Registry, GitHub and Upload tabs (that order, user
 * request 2026-11-05). A started deploy stays in the guide (`onStarted`); the background follower toasts when its build
 * finishes (plans/feat-deploy.md eng R8).
 *
 * The Starter pack is offered only where the edition's `starterPack` slot says so (user request 2026-11-12: the MT layer
 * offers it on the Free plan, so an upgraded workspace starts clean, on the Registry tab, and brings its own agents).
 */
import { useQuery } from '@tanstack/react-query'
import { Link } from '@tanstack/react-router'
import { Bot, ChevronRight, CircleCheck, ExternalLink } from 'lucide-react'
import { useState } from 'react'
import { Alert, AlertDescription } from '@nasiko/ui/primitives/alert'
import { withButtonStyle } from '@nasiko/ui/primitives/button'
import { Skeleton } from '@nasiko/ui/primitives/skeleton'
import { Tabs, TabsContent, TabsList, TabsTrigger } from '@nasiko/ui/primitives/tabs'
import { GithubTab } from '@nasiko/features/deploy/GithubTab'
import { RegistryTab } from '@nasiko/features/deploy/RegistryTab'
import type { DeploySearch } from '@nasiko/features/deploy/search'
import { type DeployStarted, UploadTab } from '@nasiko/features/deploy/UploadTab'
import { meQuery } from '@nasiko/data-access/auth'
import { useSlots } from '@nasiko/app-core/edition/edition-context'
import { copy } from './copy'
import { SEEDED_AGENTS } from './logic'
import { SectionHeading } from './parts'
/** The query a guide with no `starterPack` slot holds instead: never run (`enabled: false`). */
const NO_SLOT_QUERY = {
  queryKey: ['onboarding', 'starter-pack', 'no-slot'],
  queryFn: () => Promise.resolve(null),
}

/** Whether the Starter pack tab is offered: yes without a slot, else the slot's answer, null while it's coming. */
function useStarterPackOffered(): boolean | null {
  const slot = useSlots().starterPack
  const q = useQuery<unknown>({ ...(slot?.query ?? NO_SLOT_QUERY), enabled: !!slot, retry: false })
  if (!slot) return true
  if (q.isPending) return null
  return slot.offered(q.isError ? undefined : q.data)
}

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

/** The agents every new workspace is given, shown as a list: nothing to pick (user request 2026-11-12). */
function Giveaway() {
  return (
    <section aria-label={copy.agent.seededLabel} className="flex flex-col gap-3">
      <p className="text-sm text-muted-foreground">{copy.agent.giveaway(SEEDED_AGENTS.length)}</p>
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
