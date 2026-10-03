/**
 * One MCP server on one agent (plans/feat-mcp.md §5.1, legacy agent page MCP card): mark, name, a summary ("3 of 5 tools
 * allowed · 1 ask first"), an Enabled switch, and when expanded each tool with Allow / Ask / Block. A stance click saves
 * at once (`useSetStance`: optimistic, rolled back with a toast). The agent's MCP tab lists one per server; the
 * server's Agents tab shows the one for the picked agent.
 */
import { Link } from '@tanstack/react-router'
import { ChevronDown, RotateCw } from 'lucide-react'
import { useState } from 'react'
import { toast } from 'sonner'
import { Button } from '@/components/ui/button'
import { Collapsible, CollapsibleContent, CollapsibleTrigger } from '@/components/ui/collapsible'
import { Skeleton } from '@/components/ui/skeleton'
import { Switch } from '@/components/ui/switch'
import { ToggleGroup, ToggleGroupItem } from '@/components/ui/toggle-group'
import { AgentMark } from '@/features/agents/components/bits'
import { cn } from '@/lib/utils'
import { useAgentTools, useSetAccess, useSetStance } from '../api'
import { copy, reason, STANCE } from '../copy'
import { stanceCounts } from '../logic'
import { ServerStatusBadge } from './bits'
import { STANCES, type AgentTool, type Stance } from '../types'

export interface RuleTarget {
  connectorId: string
  label: string
  logoUrl?: string | null
  enabled: boolean
}

export function ConnectorRules({
  agentId,
  target,
  defaultOpen = false,
}: {
  agentId: string
  target: RuleTarget
  defaultOpen?: boolean
}) {
  const [open, setOpen] = useState(defaultOpen)
  const tools = useAgentTools(agentId, target.connectorId, true)
  const access = useSetAccess(agentId)
  const enabled = target.enabled
  const n = stanceCounts(tools.data ?? [])
  const summary = !enabled
    ? copy.summaryDisabled
    : tools.isPending
      ? null
      : tools.isError
        ? copy.toolsFailed
        : n.total
          ? copy.summary(n.allowed, n.total, n.asks)
          : copy.summaryNoTools

  return (
    <Collapsible
      open={open}
      onOpenChange={setOpen}
      className={cn('rounded-lg border border-border', !enabled && 'bg-muted/40')}
    >
      <div className="flex items-center gap-3 p-3">
        <CollapsibleTrigger asChild>
          <Button
            variant="ghost"
            size="icon-sm"
            aria-label={open ? copy.collapse(target.label) : copy.expand(target.label)}
          >
            <ChevronDown
              className={cn('size-4 transition-transform', open && 'rotate-180')}
              aria-hidden
            />
          </Button>
        </CollapsibleTrigger>
        <AgentMark name={target.label} iconUrl={target.logoUrl} size={28} />
        <div className="min-w-0 flex-1">
          <div className="truncate text-sm font-medium">{target.label}</div>
          {summary ? (
            <div className="truncate text-xs text-muted-foreground">{summary}</div>
          ) : (
            <Skeleton className="mt-1 h-3 w-32" />
          )}
        </div>
        <Switch
          checked={enabled}
          disabled={access.isPending}
          aria-label={enabled ? copy.disable(target.label) : copy.enable(target.label)}
          onCheckedChange={(v) =>
            access.mutate(
              { connectorId: target.connectorId, enabled: v },
              { onError: (e) => toast.error(copy.accessFailed(reason(e))) },
            )
          }
        />
      </div>
      <CollapsibleContent className="border-t border-border p-3">
        {!enabled ? (
          <p className="mb-2 text-xs text-muted-foreground">{copy.disabledNote}</p>
        ) : null}
        {tools.isPending ? (
          <Skeleton className="h-16" />
        ) : tools.isError ? (
          <p role="alert" className="text-sm text-destructive">
            {copy.toolsFailed}{' '}
            <Button
              size="sm"
              variant="outline"
              className="ml-2 h-7"
              onClick={() => void tools.refetch()}
            >
              <RotateCw className="size-3.5" aria-hidden /> {copy.retry}
            </Button>
          </p>
        ) : !tools.data.length ? (
          <p className="text-sm text-muted-foreground">{copy.noToolsYet}</p>
        ) : (
          <ul className={cn('divide-y divide-border', !enabled && 'opacity-60')}>
            {tools.data.map((t) => (
              <ToolRow
                key={t.name}
                agentId={agentId}
                connectorId={target.connectorId}
                tool={t}
                tools={tools.data}
                disabled={!enabled}
              />
            ))}
          </ul>
        )}
      </CollapsibleContent>
    </Collapsible>
  )
}

function ToolRow({
  agentId,
  connectorId,
  tool,
  tools,
  disabled,
}: {
  agentId: string
  connectorId: string
  tool: AgentTool
  tools: readonly AgentTool[]
  disabled: boolean
}) {
  const set = useSetStance(agentId, connectorId)
  return (
    <li className="flex flex-wrap items-center gap-x-3 gap-y-1 py-2">
      <div className="min-w-0 flex-1">
        <div className="font-mono text-sm">{tool.name}</div>
        {tool.description ? (
          <div className="text-xs text-muted-foreground">{tool.description}</div>
        ) : null}
      </div>
      <ToggleGroup
        type="single"
        variant="outline"
        size="sm"
        value={tool.stance}
        disabled={disabled}
        aria-label={copy.stanceFor(tool.name)}
        onValueChange={(v) => {
          // Radix lets a single group be cleared by clicking the current item: a tool always has a stance.
          if (!v || v === tool.stance) return
          set.mutate(
            { tool: tool.name, stance: v as Stance, tools },
            { onError: (e) => toast.error(copy.ruleFailed(reason(e))) },
          )
        }}
      >
        {STANCES.map((s) => (
          <ToggleGroupItem key={s} value={s} title={STANCE[s].title} className="px-3 text-xs">
            {STANCE[s].label}
          </ToggleGroupItem>
        ))}
      </ToggleGroup>
    </li>
  )
}

/**
 * A server the agent can't use yet (plans/feat-mcp.md §5.2): an upload still building or whose build failed. Shown,
 * greyed, with its status and a disabled switch, so nothing looks usable when it isn't; the link opens the server
 * (its logs for a failed build, when the caller may read them).
 */
export function UnavailableRow({
  target,
  status,
  logs,
}: {
  target: RuleTarget
  status: 'building' | 'failed'
  logs: boolean
}) {
  return (
    <div className="flex flex-wrap items-center gap-3 rounded-lg border border-border bg-muted/40 p-3">
      <span className="size-8" aria-hidden />
      <AgentMark name={target.label} iconUrl={target.logoUrl} size={28} />
      <div className="min-w-0 flex-1">
        <div className="flex flex-wrap items-center gap-2 text-sm font-medium">
          <span className="truncate">{target.label}</span>
          <ServerStatusBadge status={status} />
        </div>
        <div className="text-xs text-muted-foreground">
          {status === 'building' ? copy.notReadyBuilding : copy.notReadyFailed}{' '}
          <Link
            to="/mcp/$connectorId"
            params={{ connectorId: target.connectorId }}
            search={status === 'failed' && logs ? { tab: 'logs' } : {}}
            className="text-primary-text underline-offset-4 hover:underline"
          >
            {status === 'failed' && logs ? copy.viewLogs : copy.viewServer}
          </Link>
        </div>
      </div>
      <Switch checked={false} disabled aria-label={copy.enable(target.label)} />
    </div>
  )
}
