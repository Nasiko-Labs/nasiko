/**
 * Agents (plans/feat-mcp.md §5, the legacy catalog modal's "Agent access"): pick an agent (`?agent=`), then that
 * agent's access to this one server through the same row the agent's MCP tab uses. Owners also see the agents that
 * already have it configured (`/consumers`). The agent list only offers servers the caller has connected (or no-auth
 * ones), so an unconnected server says to connect first rather than showing a switch that can't take effect.
 */
import { useNavigate } from '@tanstack/react-router'
import { Check, ChevronsUpDown, RotateCw } from 'lucide-react'
import { useState } from 'react'
import { Badge } from '@/components/ui/badge'
import { Button } from '@/components/ui/button'
import {
  Command,
  CommandEmpty,
  CommandGroup,
  CommandInput,
  CommandItem,
  CommandList,
} from '@/components/ui/command'
import { Popover, PopoverContent, PopoverTrigger } from '@/components/ui/popover'
import { Skeleton } from '@/components/ui/skeleton'
import { StateCard } from '@/components/shared/state-card'
import { useAgentsDirectory } from '@/features/agents/api'
import { AgentLinkTo, AgentMark, Section } from '@/features/agents/components/bits'
import { isHarness } from '@/features/agents/status'
import type { Agent } from '@/features/agents/types'
import { cn } from '@/lib/utils'
import { useAgentConnectors, useConsumers } from '../api'
import { ConnectControl } from '../components/ConnectControl'
import { ConnectorRules, UnavailableRow } from '../components/ConnectorRules'
import { copy, reason } from '../copy'
import { authFlowOf, labelOf, serverStatus } from '../logic'
import type { ConnectorDetail } from '../types'

export function AgentsTab({
  connector: c,
  manage,
  agentId,
  openPopup,
}: {
  connector: ConnectorDetail
  manage: boolean
  agentId?: string
  openPopup: (url: string | undefined) => void
}) {
  const navigate = useNavigate()
  const dir = useAgentsDirectory()
  const agents = (dir.data ?? []).filter((a) => !isHarness(a))
  const picked = agentId ? agents.find((a) => a.id === agentId) : undefined
  const pick = (id: string | undefined) =>
    void navigate({ to: '.', search: (s) => ({ ...s, tab: 'agents', agent: id }), replace: true })

  return (
    <div className="space-y-4">
      <Section title={copy.agentAccess}>
        <p className="text-sm text-muted-foreground">{copy.agentAccessSub}</p>
        {dir.isPending ? (
          <Skeleton className="h-9 w-72" />
        ) : (
          <AgentPicker agents={agents} value={picked} onPick={(a) => pick(a.id)} />
        )}
        {picked ? (
          <PickedAccess connector={c} agent={picked} manage={manage} openPopup={openPopup} />
        ) : null}
      </Section>
      {manage ? <ConsumerList id={c.connector_id} onManage={pick} /> : null}
    </div>
  )
}

function PickedAccess({
  connector: c,
  agent,
  manage,
  openPopup,
}: {
  connector: ConnectorDetail
  agent: Agent
  manage: boolean
  openPopup: (url: string | undefined) => void
}) {
  const status = serverStatus(c)
  const ready = status !== 'building' && status !== 'failed'
  const list = useAgentConnectors(agent.id, ready)
  // An upload that is building or failed has nothing to give the agent yet (§5.2).
  if (!ready)
    return (
      <UnavailableRow
        target={{
          connectorId: c.connector_id,
          label: labelOf(c),
          logoUrl: c.logo_url,
          enabled: false,
        }}
        status={status}
        logs={manage}
      />
    )
  if (list.isPending) return <Skeleton className="h-16" />
  if (list.isError)
    return (
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
    )
  const row = list.data.find((x) => x.connector_id === c.connector_id)
  if (!row)
    return (
      <StateCard
        tone="warning"
        title={copy.notConnectedNote}
        action={
          <ConnectControl
            target={{
              id: c.connector_id,
              label: labelOf(c),
              authFlow: authFlowOf(c.auth_type),
              authType: c.auth_type ?? null,
              connected: c.is_connected,
            }}
            openPopup={openPopup}
          />
        }
      />
    )
  return (
    <ConnectorRules
      key={agent.id}
      agentId={agent.id}
      target={{
        connectorId: c.connector_id,
        label: agent.display_name || agent.name,
        logoUrl: agent.icon_url,
        enabled: row.enabled,
      }}
      defaultOpen
    />
  )
}

function AgentPicker({
  agents,
  value,
  onPick,
}: {
  agents: readonly Agent[]
  value?: Agent
  onPick: (a: Agent) => void
}) {
  const [open, setOpen] = useState(false)
  return (
    <Popover open={open} onOpenChange={setOpen}>
      <PopoverTrigger asChild>
        <Button
          variant="outline"
          aria-label={copy.agentLabel}
          className="w-72 justify-between font-normal"
        >
          <span className="truncate">
            {value ? value.display_name || value.name : copy.chooseAgent}
          </span>
          <ChevronsUpDown className="size-4 opacity-50" aria-hidden />
        </Button>
      </PopoverTrigger>
      <PopoverContent align="start" className="w-72 p-0">
        <Command label={copy.agentLabel}>
          <CommandInput aria-label={copy.searchAgents} placeholder={copy.searchAgents} />
          <CommandList>
            <CommandEmpty>{copy.noAgents}</CommandEmpty>
            <CommandGroup>
              {agents.map((a) => (
                <CommandItem
                  key={a.id}
                  value={`${a.display_name ?? ''} ${a.name} ${a.id}`}
                  onSelect={() => {
                    setOpen(false)
                    onPick(a)
                  }}
                >
                  <AgentMark name={a.display_name || a.name} iconUrl={a.icon_url} size={20} />
                  <span className="min-w-0 flex-1 truncate">{a.display_name || a.name}</span>
                  <Check
                    className={cn('size-4', value?.id === a.id ? 'opacity-100' : 'opacity-0')}
                    aria-hidden
                  />
                </CommandItem>
              ))}
            </CommandGroup>
          </CommandList>
        </Command>
      </PopoverContent>
    </Popover>
  )
}

function ConsumerList({ id, onManage }: { id: string; onManage: (agentId: string) => void }) {
  const q = useConsumers(id, true)
  return (
    <Section title={copy.consumersTitle}>
      {q.isPending ? (
        <Skeleton className="h-12" />
      ) : q.isError ? (
        <p role="alert" className="text-sm text-destructive">
          {reason(q.error)}{' '}
          <Button size="sm" variant="outline" className="ml-2 h-7" onClick={() => void q.refetch()}>
            <RotateCw className="size-3.5" aria-hidden /> {copy.retry}
          </Button>
        </p>
      ) : !q.data.agents.length ? (
        <p className="text-sm text-muted-foreground">{copy.noConsumers}</p>
      ) : (
        <ul className="divide-y divide-border text-sm">
          {q.data.agents.map((a) => (
            <li key={a.agent_id} className="flex flex-wrap items-center gap-2 py-2">
              <AgentLinkTo id={a.agent_id} className="min-w-0 flex-1 truncate font-medium">
                {a.agent_display_name || a.agent_name}
              </AgentLinkTo>
              <Badge variant={a.enabled ? 'success' : 'muted'}>
                {a.enabled ? copy.enabled : copy.summaryDisabled}
              </Badge>
              <span className="text-xs text-muted-foreground tabular-nums">
                {copy.toolsOf(a.tools_used, a.total_tools)}
              </span>
              <Button size="sm" variant="ghost" onClick={() => onManage(a.agent_id)}>
                {copy.manage}
              </Button>
            </li>
          ))}
        </ul>
      )}
    </Section>
  )
}
