/**
 * The agent detail's MCP tab (plans/feat-mcp.md §5.2, legacy agent page "Configure → MCP"): the MCP servers this agent
 * may use, one `ConnectorRules` each. The server lists only servers the caller has connected, plus no-auth ones; an
 * upload that is still building or failed is one of those, so it shows greyed with its status (`UnavailableRow`).
 * Its build state comes from the catalog list (polled while a build runs), so a finished build turns into a row.
 */
import { useQuery } from '@tanstack/react-query'
import { Link } from '@tanstack/react-router'
import { RotateCw } from 'lucide-react'
import { Button } from '@/components/ui/button'
import { PageLoader } from '@/components/shared/page-loader'
import { EmptyState, StateCard } from '@/components/shared/state-card'
import { Section } from '@/features/agents/components/bits'
import { meQuery } from '@/lib/api/auth'
import { useAgentConnectors, useConnectors } from '../api'
import { copy, reason } from '../copy'
import { labelOf, unusable } from '../logic'
import { ConnectorRules, UnavailableRow } from './ConnectorRules'

export function AgentMcpTab({ agentId }: { agentId: string }) {
  const list = useAgentConnectors(agentId, true)
  // A failed catalog read just means no row is greyed: the agent list itself still works.
  const servers = useConnectors()
  const me = useQuery(meQuery).data
  const notReady = unusable(servers.data)
  const owned = new Set(servers.data?.created_by_you.map((c) => c.connector_id))
  return (
    <Section title={copy.agentMcpTitle}>
      <p className="text-sm text-muted-foreground">{copy.agentMcpSub}</p>
      {list.isPending || (servers.isPending && !servers.isError) ? (
        <PageLoader label={copy.loadingCatalog} inline className="min-h-64" />
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
        <EmptyState
          title={copy.noAgentServers}
          action={
            <Button asChild size="sm" variant="outline">
              <Link to="/mcp" search={{}}>
                {copy.openCatalog}
              </Link>
            </Button>
          }
        >
          {copy.noAgentServersFix}
        </EmptyState>
      ) : (
        <ul className="space-y-2">
          {list.data.map((c) => {
            const target = {
              connectorId: c.connector_id,
              label: labelOf(c),
              logoUrl: c.logo_url,
              enabled: c.enabled,
            }
            const status = notReady.get(c.connector_id)
            return (
              <li key={c.connector_id}>
                {status ? (
                  <UnavailableRow
                    target={target}
                    status={status}
                    logs={owned.has(c.connector_id) || !!me?.is_superuser}
                  />
                ) : (
                  <ConnectorRules agentId={agentId} target={target} />
                )}
              </li>
            )
          })}
        </ul>
      )}
    </Section>
  )
}
