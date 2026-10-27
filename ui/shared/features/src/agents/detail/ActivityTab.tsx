/**
 * Activity (plan §7.3): 24 h stats, resources, and logs (newest first) with a live tail. Recent sessions moved to the
 * Overview's hub, which reads a bounded 7-day sample (plans/feat-agent-centric.md PR1c, eng "Hub sessions").
 */
import { useId, useState, type ReactNode } from 'react'
import { Button } from '@nasiko/ui/primitives/button'
import { Field, FieldLabel } from '@nasiko/ui/primitives/field'
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from '@nasiko/ui/primitives/select'
import { Progress } from '@nasiko/ui/primitives/progress'
import { Skeleton } from '@nasiko/ui/primitives/skeleton'
import { SearchInput } from '@nasiko/ui/composites/search-input'
import { LogDrawer } from '@nasiko/features/sessions/LogDrawer'
import { fmtInt, fmtLatency, fmtMoney } from '@nasiko/ui/lib/format'
import { cn } from '@nasiko/ui/lib/utils'
import { ApiError } from '@nasiko/data-access/client'
import { useAgentLogs, useAgentResources, useAgentStats } from '../api'
import { ErrorNote, LearnMore, Section } from '../components/bits'
import { copy } from '../copy'
import type { AgentView } from '../normalize'
import { MEMORY_WARN_PCT } from '../tuning'

const LEVELS = ['', 'ERROR', 'WARN', 'INFO'] as const
/** An empty value would show the placeholder: "all levels" travels as this key. */
const ALL = 'all'

export function ActivityTab({
  agent,
  tailOpen,
  onTailChange,
}: {
  agent: AgentView
  tailOpen: boolean
  onTailChange: (open: boolean) => void
}) {
  const stats = useAgentStats(agent.id, true)
  const resources = useAgentResources(agent.id, true)
  const [level, setLevel] = useState<(typeof LEVELS)[number]>('')
  const [term, setTerm] = useState('')
  const levelId = useId()
  const logs = useAgentLogs(agent.id, level, true)
  const obsOff = (e: unknown) => e instanceof ApiError && e.status === 503
  const lines = (logs.data ?? []).filter(
    (l) => !term || l.message.toLowerCase().includes(term.toLowerCase()),
  )

  return (
    <div className="space-y-4">
      {/* One strip, two readings: what the agent did (24 h) and what it holds now. */}
      <section
        aria-label={copy.summary}
        className="grid rounded-xl border bg-card text-card-foreground lg:grid-cols-[minmax(0,4fr)_minmax(0,3fr)]"
      >
        <StatGroup title={copy.stats}>
          {stats.isPending ? (
            <Skeleton className="h-12" />
          ) : stats.isError ? (
            obsOff(stats.error) ? (
              <p className="text-sm text-muted-foreground">{copy.obsOff}</p>
            ) : (
              <ErrorNote error={stats.error} onRetry={() => void stats.refetch()} />
            )
          ) : (
            <dl className="grid grid-cols-2 gap-x-6 gap-y-3 sm:grid-cols-4">
              <Stat
                label={copy.turns}
                value={fmtInt(stats.data.trace_count ?? 0)}
                title={copy.turnsTip}
              />
              <Stat label={copy.cost} value={fmtMoney(stats.data.cost_summary?.total?.cost ?? 0)} />
              <Stat label={copy.latencyP50} value={fmtLatency(stats.data.latency_ms_p50)} />
              <Stat label={copy.latencyP99} value={fmtLatency(stats.data.latency_ms_p99)} />
            </dl>
          )}
        </StatGroup>
        <StatGroup title={copy.resources} className="border-t lg:border-t-0 lg:border-l">
          {resources.isPending ? (
            <Skeleton className="h-12" />
          ) : resources.isError ? (
            obsOff(resources.error) ? (
              <p className="text-sm text-muted-foreground">{copy.obsOff}</p>
            ) : (
              <ErrorNote error={resources.error} onRetry={() => void resources.refetch()} />
            )
          ) : !resources.data.usage ? (
            <p className="text-sm text-muted-foreground">{copy.noResources}</p>
          ) : (
            <dl className="grid grid-cols-2 gap-x-6 gap-y-3">
              <Stat
                label={copy.cpu}
                value={
                  resources.data.usage.cpu_percent != null
                    ? `${resources.data.usage.cpu_percent.toFixed(1)}%`
                    : '—'
                }
              />
              <MemoryStat
                used={resources.data.usage.memory_usage_bytes}
                limit={resources.data.usage.memory_limit_bytes}
              />
            </dl>
          )}
        </StatGroup>
      </section>

      <Section
        title={copy.logs}
        action={
          <div className="flex items-center gap-2">
            <LearnMore href="operate" />
            <Button size="sm" variant="outline" onClick={() => onTailChange(true)}>
              {copy.liveTail}
            </Button>
          </div>
        }
      >
        <div className="flex flex-wrap items-center gap-2">
          <Field orientation="horizontal" className="w-auto gap-1">
            <FieldLabel htmlFor={levelId} className="font-normal text-muted-foreground">
              {copy.level}
            </FieldLabel>
            <Select
              value={level || ALL}
              onValueChange={(v) => setLevel(v === ALL ? '' : (v as (typeof LEVELS)[number]))}
            >
              <SelectTrigger id={levelId} size="sm">
                <SelectValue />
              </SelectTrigger>
              <SelectContent>
                {LEVELS.map((l) => (
                  <SelectItem key={l || ALL} value={l || ALL}>
                    {l || copy.allLevels}
                  </SelectItem>
                ))}
              </SelectContent>
            </Select>
          </Field>
          <SearchInput
            aria-label={copy.searchLogs}
            value={term}
            onChange={(e) => setTerm(e.target.value)}
            placeholder={copy.searchLogs}
            className="h-8"
          />
        </div>
        {logs.isPending ? (
          <Skeleton className="h-24" />
        ) : logs.isError ? (
          logs.error instanceof ApiError && logs.error.status === 404 ? (
            <p className="text-sm text-muted-foreground">{copy.noLogs}</p>
          ) : (
            <ErrorNote error={logs.error} onRetry={() => void logs.refetch()} />
          )
        ) : lines.length ? (
          // Newest first, as the server sends them.
          <ol
            className="max-h-[28rem] overflow-auto rounded-lg border bg-muted/40 py-1 font-mono text-xs leading-5"
            aria-label={copy.logLines}
          >
            {lines.map((l) => (
              <li
                key={`${l.timestamp}|${l.message}`}
                className="grid grid-cols-[auto_minmax(0,1fr)] gap-x-3 px-3 py-0.5 hover:bg-muted sm:grid-cols-[auto_3rem_minmax(0,1fr)] sm:py-0"
              >
                <time
                  dateTime={l.timestamp}
                  title={l.timestamp}
                  className="text-muted-foreground tabular-nums"
                >
                  {l.timestamp.slice(0, 19).replace('T', ' ')}
                </time>
                <span className={LEVEL_TONE[l.level ?? ''] ?? 'text-muted-foreground'}>
                  {l.level ?? ''}
                </span>
                <span
                  className={cn(
                    'col-span-2 break-words whitespace-pre-wrap sm:col-span-1',
                    l.level === 'ERROR' && 'text-destructive',
                  )}
                >
                  {l.message}
                </span>
              </li>
            ))}
          </ol>
        ) : (
          <p className="text-sm text-muted-foreground">{copy.noLogs}</p>
        )}
      </Section>
      {tailOpen ? (
        <LogDrawer agent={agent.id} label={agent.displayName} open onOpenChange={onTailChange} />
      ) : null}
    </div>
  )
}

function StatGroup({
  title,
  className,
  children,
}: {
  title: string
  className?: string
  children: ReactNode
}) {
  return (
    <div className={cn('flex min-w-0 flex-col gap-3 p-4', className)}>
      <h2 className="text-sm font-semibold">{title}</h2>
      {children}
    </div>
  )
}

function Stat({ label, value, title }: { label: string; value: string; title?: string }) {
  return (
    <div title={title} className="min-w-0">
      <dt className="text-xs text-muted-foreground">{label}</dt>
      <dd className="mt-0.5 text-xl font-medium tabular-nums">{value}</dd>
    </div>
  )
}

/** Memory against its limit: the headroom is what an operator reads here. */
function MemoryStat({ used, limit }: { used?: number | null; limit?: number | null }) {
  const pct = used != null && limit ? Math.min(100, (used / limit) * 100) : null
  return (
    <div className="min-w-0">
      <dt className="text-xs text-muted-foreground">{copy.memory}</dt>
      <dd className="mt-0.5 text-xl font-medium tabular-nums">
        {mb(used)}
        {limit ? (
          <span className="text-sm font-normal text-muted-foreground"> / {mb(limit)}</span>
        ) : null}
      </dd>
      {pct != null ? (
        <Progress
          value={pct}
          aria-label={copy.memoryOfLimit(Math.round(pct))}
          className={cn(
            'mt-2 h-1 bg-muted *:bg-muted-foreground',
            pct >= MEMORY_WARN_PCT && '*:bg-warning',
          )}
        />
      ) : null}
    </div>
  )
}

const LEVEL_TONE: Record<string, string> = {
  ERROR: 'font-medium text-destructive',
  WARN: 'text-warning',
}

const mb = (v: number | null | undefined) => (v == null ? '—' : `${Math.round(v / 1_048_576)} MB`)
