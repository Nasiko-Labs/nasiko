/**
 * The first run's "Works before your first agent" panel (redesign 2026-10-21, in place of Quick actions and the one-line
 * Harnesses card): four cells that need no agent, each a link to the page that owns it with a live status line, hairlines
 * between them in one card. The Orchestrator answers before any agent is deployed; a model, MCP servers and coding
 * harnesses can all be connected first. Each cell reads its own source and fails on its own; a failure never reads as
 * "none yet".
 */
import { Link } from '@tanstack/react-router'
import {
  ArrowRight,
  Plug,
  Route as RouteIcon,
  SquareTerminal,
  Waypoints,
  type LucideIcon,
} from 'lucide-react'
import type { ReactNode } from 'react'
import { Card } from '@nasiko/ui/primitives/card'
import { Skeleton } from '@nasiko/ui/primitives/skeleton'
import { useConnectors, useToolkits } from '@nasiko/features/mcp/api'
import { useConfigs } from '@nasiko/features/router/api'
import { fmtMoney } from '@nasiko/ui/lib/format'
import { cn } from '@nasiko/ui/lib/utils'
import type { HarnessSummary } from '../api'
import { copy } from '../copy'
import { SourceFailed } from './Card'

type Tone = 'ready' | 'idle' | null

/** One line of status: a dot (never the only signal: the words say the same) and the sentence. */
function Status({ tone, children }: { tone: Tone; children: ReactNode }) {
  return (
    <p className="flex items-baseline gap-2 text-sm text-pretty text-muted-foreground">
      {tone ? (
        <span
          aria-hidden
          className={cn(
            'size-1.5 shrink-0 translate-y-[-1px] rounded-full',
            tone === 'ready' ? 'bg-success' : 'bg-muted-foreground/40',
          )}
        />
      ) : null}
      <span>{children}</span>
    </p>
  )
}

const pendingLine = <Skeleton className="h-4 w-40 motion-reduce:animate-none" />

/**
 * A cell: the title is the link, stretched over the whole cell, so the cell is one target while a Retry inside it
 * stays its own button above the stretch.
 */
function Cell({
  Icon,
  link,
  testId,
  children,
}: {
  Icon: LucideIcon
  link: ReactNode
  testId?: string
  children: ReactNode
}) {
  return (
    <div
      data-testid={testId}
      className="group relative flex flex-col gap-3 bg-card p-4 transition-colors hover:bg-muted/40 motion-reduce:transition-none"
    >
      <div className="flex items-center justify-between">
        <span
          aria-hidden
          className="flex size-8 items-center justify-center rounded-md bg-muted text-muted-foreground transition-colors group-hover:text-foreground motion-reduce:transition-none"
        >
          <Icon className="size-4" />
        </span>
        <ArrowRight
          aria-hidden
          className="size-4 -translate-x-1 text-muted-foreground opacity-0 transition group-hover:translate-x-0 group-hover:opacity-100 motion-reduce:transition-none"
        />
      </div>
      <div className="flex flex-col gap-1">
        <h3 className="text-sm font-medium">{link}</h3>
        {children}
      </div>
    </div>
  )
}

/** The stretched link every cell's title is. */
const STRETCH =
  'outline-none after:absolute after:inset-0 after:content-[""] focus-visible:after:ring-2 focus-visible:after:ring-ring focus-visible:after:ring-inset'

function ModelCell() {
  const configs = useConfigs()
  const n = configs.data?.length ?? 0
  return (
    <Cell
      Icon={Waypoints}
      link={
        <Link to="/router" className={STRETCH}>
          {copy.setUpNow.model}
        </Link>
      }
    >
      {configs.isPending ? (
        pendingLine
      ) : configs.isError ? (
        <Status tone={null}>{copy.setUpNow.unknown}</Status>
      ) : (
        <Status tone={n ? 'ready' : 'idle'}>
          {n ? copy.setUpNow.models(n) : copy.setUpNow.noModel}
        </Status>
      )}
    </Cell>
  )
}

function ToolsCell() {
  const connectors = useConnectors()
  const toolkits = useToolkits()
  // Custom servers are the caller's own list; platform toolkits may be unconfigured, so they only add when they answer.
  const servers = [
    ...(connectors.data?.created_by_you ?? []),
    ...(connectors.data?.shared_with_you ?? []),
    ...(toolkits.data?.toolkits ?? []),
  ]
  const connected = servers.filter((s) => s.is_connected).length
  return (
    <Cell
      Icon={Plug}
      link={
        <Link to="/mcp" search={{} as never} className={STRETCH}>
          {copy.setUpNow.tools}
        </Link>
      }
    >
      {connectors.isPending || toolkits.isPending ? (
        pendingLine
      ) : connectors.isError ? (
        <Status tone={null}>{copy.setUpNow.unknown}</Status>
      ) : (
        <Status tone={connected ? 'ready' : 'idle'}>
          {connected
            ? copy.setUpNow.serversOn(connected)
            : servers.length
              ? copy.setUpNow.serversOff(servers.length)
              : copy.setUpNow.noServers}
        </Status>
      )}
    </Cell>
  )
}

/** The Harnesses cell, from the page's harness summary (the same read and states as the KPI tile). */
export function HarnessCell({ data, days }: { data: HarnessSummary; days: number }) {
  // Priced cost only, as the tile sums it.
  const total = data.harnesses
    .filter((h) => !h.unpriced && (h.cost ?? 0) > 0)
    .reduce((n, h) => n + (h.cost ?? 0), 0)
  return (
    <Cell
      Icon={SquareTerminal}
      testId="overview-harnesses"
      link={
        <Link to="/harnesses" search={{} as never} className={STRETCH}>
          {copy.harnesses.title}
        </Link>
      }
    >
      {data.isPending ? (
        pendingLine
      ) : data.error ? (
        // Above the stretched link, so Retry is its own target.
        <div className="relative z-10">
          <SourceFailed what={copy.harnesses.what} onRetry={data.retry} />
        </div>
      ) : data.notVisible ? (
        <Status tone={null}>{copy.harnesses.notVisible}</Status>
      ) : !data.connected ? (
        <Status tone="idle">{copy.setUpNow.noHarnesses}</Status>
      ) : (
        <Status tone="ready">
          {copy.harnesses.connected(data.connected)}
          {data.ownOnly ? ` (${copy.kpi.yourUsage})` : ''}
          {total > 0 ? ` · ${copy.harnesses.lineCost(fmtMoney(total), days)}` : ''}
        </Status>
      )}
    </Cell>
  )
}

export function SetUpNow({ harnesses, days }: { harnesses: HarnessSummary; days: number }) {
  return (
    <section
      aria-labelledby="overview-setup-now"
      data-testid="overview-actions"
      data-tour="overview-actions"
      className="flex flex-col gap-3"
    >
      <h2 id="overview-setup-now" className="text-sm font-semibold">
        {copy.setUpNow.title}
      </h2>
      {/* Hairlines are the 1 px gap over the border colour: they follow the columns at every width. */}
      <Card className="grid gap-px overflow-hidden bg-border p-0 @[560px]/overview:grid-cols-2 @[1100px]/overview:grid-cols-4">
        <Cell
          Icon={RouteIcon}
          link={
            <Link to="/chat" search={{ auto: 1 } as never} className={STRETCH}>
              {copy.actions.orchestrator}
            </Link>
          }
        >
          <Status tone={null}>{copy.setUpNow.orchestrator}</Status>
        </Cell>
        <ModelCell />
        <ToolsCell />
        <HarnessCell data={harnesses} days={days} />
      </Card>
    </section>
  )
}
