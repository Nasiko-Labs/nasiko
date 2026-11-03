/**
 * The desktop app's LLM routing card (routing.ts): one row per routable harness on this computer, with the LLM config
 * to route it with and Connect / Use this config / Take over / Disconnect. In a browser, or before the app is signed
 * in, it renders nothing. Outcomes are spoken through the card's one polite live region.
 */
import { useState } from 'react'
import { Link } from '@tanstack/react-router'
import { AlertTriangle } from 'lucide-react'
import { Announcer } from '@nasiko/ui/composites/announcer'
import { useAnnounce } from '@nasiko/ui/composites/announce'
import { Button, asButton } from '@nasiko/ui/primitives/button'
import { Card } from '@nasiko/ui/primitives/card'
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from '@nasiko/ui/primitives/select'
import { Skeleton } from '@nasiko/ui/primitives/skeleton'
import { Spinner } from '@nasiko/ui/primitives/spinner'
import { useAgentRouting, useConfigs } from '@nasiko/features/router/api'
import type { LlmConfig } from '@nasiko/features/router/types'
import { ApiError } from '@nasiko/data-access/client'
import { copy as pageCopy } from '../copy'
import { useDesktopState } from '../desktop'
import {
  configModel,
  connectAction,
  initialChoice,
  useDesktopRouting,
  useRoutingChange,
  type RoutedHarness,
} from '../routing'
import { HarnessLabel } from './bits'

const copy = pageCopy.routing

export function RoutingCard() {
  const desktop = useDesktopState()
  const signedIn = !!desktop.data?.signed_in
  const harnesses = useDesktopRouting(signedIn)
  const configs = useConfigs(signedIn)
  if (!signedIn) return null
  return (
    <Announcer>
      <Card
        className="gap-4 p-5"
        render={<section aria-labelledby="harness-routing-title" data-testid="harness-routing" />}
      >
        <div className="flex flex-col gap-1">
          <h2 id="harness-routing-title" className="text-base font-semibold">
            {copy.title}
          </h2>
          <p className="max-w-[70ch] text-sm text-muted-foreground">{copy.body}</p>
        </div>
        {harnesses.isError ? (
          <Problem text={copy.loadFailed} onRetry={() => void harnesses.refetch()} />
        ) : configs.isError ? (
          <Problem text={copy.configsFailed} onRetry={() => void configs.refetch()} />
        ) : !harnesses.data || !configs.data ? (
          <Skeleton className="h-24 w-full" />
        ) : !configs.data.length ? (
          <div className="flex flex-wrap items-center gap-2 text-sm">
            <span className="text-muted-foreground">{copy.noConfigs}</span>
            <Link to="/router" {...asButton({ variant: 'outline', size: 'sm' })}>
              {copy.openRouter}
            </Link>
          </div>
        ) : (
          <ul aria-label={copy.listLabel} className="flex flex-col divide-y divide-border">
            {harnesses.data.map((h) => (
              <HarnessRow key={h.id} harness={h} configs={configs.data} />
            ))}
          </ul>
        )}
      </Card>
    </Announcer>
  )
}

function Problem({ text, onRetry }: { text: string; onRetry: () => void }) {
  return (
    <div role="alert" className="flex flex-wrap items-center gap-2 text-sm text-destructive">
      <AlertTriangle aria-hidden className="size-4" />
      <span>{text}</span>
      <Button size="sm" variant="outline" onClick={onRetry}>
        {copy.retry}
      </Button>
    </div>
  )
}

function HarnessRow({ harness, configs }: { harness: RoutedHarness; configs: LlmConfig[] }) {
  const announce = useAnnounce()
  const agentId = harness.agent_id ?? ''
  const routing = useAgentRouting(agentId, !!harness.agent_id)
  const change = useRoutingChange()
  const [picked, setPicked] = useState<string | null>(null)
  const [notes, setNotes] = useState<string[]>([])
  const choice = picked ?? initialChoice(routing.data, configs)
  const action = connectAction(harness, choice, routing.data)
  const busy = change.isPending
  const connected = harness.connected_by !== null

  const run = (kind: 'connect' | 'disconnect') => {
    setNotes([])
    announce(kind === 'connect' ? copy.working(harness.name) : copy.disconnecting(harness.name))
    change.mutate(
      kind === 'connect'
        ? { action: 'connect', agent: harness.id, config: choice }
        : { action: 'disconnect', agent: harness.id },
      {
        onSuccess: (r) => {
          setNotes(r.notes)
          announce(
            kind === 'connect'
              ? copy.connected(harness.name, r.provider, r.model)
              : copy.disconnected(harness.name),
          )
        },
        onError: (e) => announce(copy.failed(harness.name, errorDetail(e))),
      },
    )
  }

  return (
    <li className="flex flex-col gap-2 py-3 first:pt-0 last:pb-0" data-harness={harness.id}>
      <div className="flex flex-wrap items-center justify-between gap-x-4 gap-y-2">
        <div className="flex min-w-0 flex-col gap-0.5">
          <HarnessLabel id={harness.id} className="font-medium" />
          <span className="text-sm text-muted-foreground">{statusLine(harness, routing.data)}</span>
        </div>
        <div className="flex flex-wrap items-center gap-2">
          <Select
            value={choice ?? undefined}
            onValueChange={setPicked}
            disabled={!harness.detected || busy}
          >
            <SelectTrigger
              size="sm"
              aria-label={copy.configLabel(harness.name)}
              className="w-64 max-w-full pointer-coarse:min-h-11"
            >
              <SelectValue />
            </SelectTrigger>
            <SelectContent>
              {configs.map((c) => (
                <SelectItem key={c.id} value={c.id}>
                  {copy.configOption(c.name, c.provider, configModel(c), c.is_default)}
                </SelectItem>
              ))}
            </SelectContent>
          </Select>
          {action ? (
            <Button size="sm" disabled={busy} onClick={() => run('connect')}>
              {busy && change.variables?.action === 'connect' ? <Spinner aria-hidden /> : null}
              {action === 'connect'
                ? copy.connect
                : action === 'take-over'
                  ? copy.takeOver
                  : copy.apply}
            </Button>
          ) : null}
          {connected ? (
            <Button size="sm" variant="outline" disabled={busy} onClick={() => run('disconnect')}>
              {busy && change.variables?.action === 'disconnect' ? <Spinner aria-hidden /> : null}
              {copy.disconnect}
            </Button>
          ) : null}
        </div>
      </div>
      {harness.connected_by === 'cli' ? (
        <p className="text-xs text-muted-foreground">{copy.byCli}</p>
      ) : null}
      {harness.changed ? <p className="text-xs text-warning">{copy.changed}</p> : null}
      {connected && !harness.mcp ? (
        <p className="text-xs text-muted-foreground">{copy.noMcp}</p>
      ) : null}
      {change.isError ? (
        <p role="alert" className="flex items-center gap-2 text-sm text-destructive">
          <AlertTriangle aria-hidden className="size-4 shrink-0" />
          {copy.failed(harness.name, errorDetail(change.error))}
        </p>
      ) : null}
      {notes.length ? (
        <ul className="flex flex-col gap-0.5 text-xs text-muted-foreground">
          {notes.map((n) => (
            <li key={n}>{n}</li>
          ))}
        </ul>
      ) : null}
    </li>
  )
}

function statusLine(
  harness: RoutedHarness,
  routing: ReturnType<typeof useAgentRouting>['data'],
): string {
  if (!harness.detected && !harness.connected_by) return copy.notOnComputer
  if (!harness.connected_by) return copy.notRouted
  const name = routing?.llm_config?.name
  if (!name) return copy.routed
  return routing.source === 'attached' ? copy.routedVia(name) : copy.routedViaDefault(name)
}

function errorDetail(e: unknown): string | null {
  return e instanceof ApiError ? e.serverMessage : e instanceof Error ? e.message : null
}
