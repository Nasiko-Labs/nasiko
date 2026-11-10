/**
 * The agent detail's MCP tab for someone the agent is shared with (NAS-1461). Its tools run on the chatting user's
 * own connections, never the owner's, so this lists the servers the agent uses with where the viewer stands, and
 * links each one they have to connect to its server page (whose Overview holds the credential / OAuth panel).
 * Nothing about the owner's connections is shown: the server answers only the viewer's own standing.
 */
import { Link } from '@tanstack/react-router'
import { RotateCw } from 'lucide-react'
import { Badge } from '@nasiko/ui/primitives/badge'
import { Button } from '@nasiko/ui/primitives/button'
import { PageLoader } from '@nasiko/ui/composites/page-loader'
import { EmptyState, StateCard } from '@nasiko/ui/composites/state-card'
import { Section } from '@nasiko/features/agents/components/bits'
import { useMyConnections } from '../api'
import { copy, reason } from '../copy'
import type { CallerConnection } from '../types'

const STATUS_LABEL: Record<CallerConnection, string> = {
  ready: copy.callerReady,
  connected: copy.callerConnected,
  needs_connection: copy.callerNeedsConnection,
  not_shared: copy.callerNotShared,
}

export function SharedMcpTab({ agentId }: { agentId: string }) {
  const list = useMyConnections(agentId, true)
  return (
    <Section title={copy.agentMcpTitle}>
      <p className="text-sm text-muted-foreground">{copy.sharedMcpSub}</p>
      {list.isPending ? (
        <PageLoader label={copy.loadingCatalog} inline className="min-h-32" />
      ) : list.isError ? (
        <StateCard
          tone="error"
          title={copy.agentFailed}
          fix={reason(list.error)}
          action={
            <Button size="sm" variant="outline" onClick={() => void list.refetch()}>
              <RotateCw className="size-3.5" aria-hidden /> {copy.retry}
            </Button>
          }
        />
      ) : !list.data.length ? (
        <EmptyState title={copy.noSharedServers} />
      ) : (
        <ul className="space-y-2">
          {list.data.map((c) => (
            <li
              key={c.connector_id}
              className="flex flex-wrap items-center justify-between gap-2 rounded-md border p-3"
            >
              <div className="min-w-0">
                <div className="truncate text-sm font-medium">{c.display_name ?? c.name}</div>
                {c.status === 'not_shared' ? (
                  <div className="text-xs text-muted-foreground">{copy.callerNotSharedNote}</div>
                ) : null}
              </div>
              {c.status === 'needs_connection' ? (
                <Link
                  to="/mcp/$connectorId"
                  params={{ connectorId: c.connector_id }}
                  search={{}}
                  className="text-sm text-primary-text underline-offset-4 hover:underline"
                >
                  {STATUS_LABEL[c.status]}
                </Link>
              ) : (
                <Badge variant={c.status === 'not_shared' ? 'muted' : 'success'}>
                  {STATUS_LABEL[c.status]}
                </Badge>
              )}
            </li>
          ))}
        </ul>
      )}
    </Section>
  )
}
