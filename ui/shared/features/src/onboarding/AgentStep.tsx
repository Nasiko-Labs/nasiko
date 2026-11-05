/**
 * The Agent and key step's first half: agents already running, to chat with once there is a key (Chat waits for it:
 * every model call is billed to the user's own key), then the Deploy page's own Registry, GitHub and Upload tabs (that order, user request
 * 2026-11-05). A started deploy stays in the guide (`onStarted`); the background follower toasts
 * when its build finishes (plans/feat-deploy.md eng R8).
 */
import { useQuery } from '@tanstack/react-query'
import { Link } from '@tanstack/react-router'
import { Bot, ChevronRight, CircleCheck, KeyRound } from 'lucide-react'
import { useState } from 'react'
import { Alert, AlertDescription } from '@nasiko/ui/primitives/alert'
import { Button, withButtonStyle } from '@nasiko/ui/primitives/button'
import { Skeleton } from '@nasiko/ui/primitives/skeleton'
import { Tabs, TabsContent, TabsList, TabsTrigger } from '@nasiko/ui/primitives/tabs'
import { useAgentsDirectory } from '@nasiko/features/agents/api'
import { exampleTexts } from '@nasiko/features/agents/normalize'
import { isHarness } from '@nasiko/features/agents/status'
import type { Agent } from '@nasiko/features/agents/types'
import { GithubTab } from '@nasiko/features/deploy/GithubTab'
import { RegistryTab } from '@nasiko/features/deploy/RegistryTab'
import type { DeploySearch } from '@nasiko/features/deploy/search'
import { type DeployStarted, UploadTab } from '@nasiko/features/deploy/UploadTab'
import { meQuery } from '@nasiko/data-access/auth'
import { copy } from './copy'
import { SectionHeading } from './parts'

/** Enough to choose from without scrolling the guide. */
const MAX_RUNNING = 4

export function AgentStep({
  started,
  onStarted,
  hasKey,
  onChat,
  onLeave,
}: {
  started: DeployStarted | null
  /** The user has a model key (connected above, or an LLM config already): until then Chat is off. */
  hasKey: boolean
  onStarted: (to: DeployStarted) => void
  /** Chat with a running agent: the guide closes and Chat opens on it. */
  onChat: (agentId: string) => void
  /** Following a link out of the guide closes it first. */
  onLeave: () => void
}) {
  const me = useQuery(meQuery).data
  const dir = useAgentsDirectory()
  const running = (dir.data ?? [])
    .filter((a) => a.status === 'running' && !isHarness(a))
    .slice(0, MAX_RUNNING)
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
      ) : !me || dir.isPending ? (
        <Skeleton className="h-48 w-full" />
      ) : (
        // Running agents first: chatting with one is the quickest start. A failed list just leaves the deploy tabs.
        <Tabs defaultValue={running.length ? 'try' : 'registry'} className="gap-4">
          {/* Sticks to the top of the guide's scrolling body (px-6 py-8 md:px-10, so -top-8), as on the Deploy page. */}
          <div className="sticky -top-8 z-20 -mx-6 bg-background px-6 py-2 md:-mx-10 md:px-10">
            <TabsList>
              {running.length ? <TabsTrigger value="try">{copy.agent.try}</TabsTrigger> : null}
              <TabsTrigger value="registry">{copy.agent.registry}</TabsTrigger>
              <TabsTrigger value="github">{copy.agent.github}</TabsTrigger>
              <TabsTrigger value="upload">{copy.agent.upload}</TabsTrigger>
            </TabsList>
          </div>
          {running.length ? (
            <TabsContent value="try" className="flex flex-col gap-3">
              {hasKey ? null : (
                <p className="flex items-center gap-2 text-sm text-muted-foreground">
                  <KeyRound aria-hidden className="size-4 shrink-0" />
                  {copy.agent.needsKey}
                </p>
              )}
              <Running agents={running} hasKey={hasKey} onChat={onChat} />
            </TabsContent>
          ) : null}
          <TabsContent value="registry">
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

/** Each running agent with its first skill example. */
function Running({
  agents,
  hasKey,
  onChat,
}: {
  agents: readonly Agent[]
  hasKey: boolean
  onChat: (agentId: string) => void
}) {
  return (
    <ul aria-label={copy.agent.tryLabel} className="grid gap-3 sm:grid-cols-2">
      {agents.map((a) => {
        const name = a.display_name?.trim() || a.name
        const example = a.skills.flatMap(exampleTexts)[0]
        return (
          <li key={a.id} className="flex flex-col gap-3 rounded-xl border bg-card p-4">
            <div className="flex items-start gap-3">
              <span className="flex size-9 shrink-0 items-center justify-center rounded-lg bg-muted">
                <Bot aria-hidden className="size-4" />
              </span>
              <div className="min-w-0">
                <p className="truncate text-sm font-medium">{name}</p>
                <p className="line-clamp-2 text-xs text-muted-foreground">
                  {example ? `“${example}”` : copy.agent.noExample}
                </p>
              </div>
            </div>
            <div className="mt-auto flex justify-end">
              <Button
                type="button"
                size="sm"
                variant="outline"
                disabled={!hasKey}
                aria-label={copy.agent.chatWith(name)}
                onClick={() => onChat(a.id)}
              >
                {copy.agent.chat}
              </Button>
            </div>
          </li>
        )
      })}
    </ul>
  )
}
