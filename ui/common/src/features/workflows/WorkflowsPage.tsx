/**
 * Deployed or draft workflows (plans/feat-workflows.md §2): sorted by the server, searched here. On `main` the rows
 * carry no metrics (W-3), so the metric chips, health, last-run line and sort are hidden, and Drafts is absent (W-1).
 */
import { useQuery, useQueryClient } from '@tanstack/react-query'
import { Link, useNavigate } from '@tanstack/react-router'
import { EllipsisVertical, Plus, RotateCw, SearchX, Workflow } from 'lucide-react'
import { memo, useDeferredValue, useState } from 'react'
import { toast } from 'sonner'
import { PageHeader } from '@/components/shared/page-header'
import { PageLoader } from '@/components/shared/page-loader'
import { SearchInput } from '@/components/shared/search-input'
import { EmptyState, StateCard } from '@/components/shared/state-card'
import { Button } from '@/components/ui/button'
import { Card } from '@/components/ui/card'
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuTrigger,
} from '@/components/ui/dropdown-menu'
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from '@/components/ui/select'
import { relTime } from '@/features/agents/format'
import { fmtLongDay, fmtPct, fmtTokens } from '@/lib/format'
import { cn } from '@/lib/utils'
import { dropFromLists, workflowListQuery } from './api'
import { HEALTH_LABEL, SORT_LABEL, copy, reason, type ListMode } from './copy'
import { useStartRun } from './run'
import {
  agentsOf,
  descriptionOf,
  hasMetrics,
  isDeployed,
  isDraftsAbsent,
  lastRun,
  stepsOf,
} from './logic'
import {
  DRAFT_SORTS,
  WORKFLOW_SORTS,
  type DraftSort,
  type DraftsSearch,
  type WorkflowSort,
} from './search'
import type { WorkflowRow } from './types'
import { Chip, SectionNav, ToneBadge } from './components/bits'
import { DeleteWorkflowDialog } from './components/DeleteWorkflowDialog'

/** The last-run line's dot; the words beside it carry the meaning. */
const DOT = {
  success: 'bg-success',
  error: 'bg-destructive',
  warning: 'bg-warning',
  info: 'bg-info',
  brand: 'bg-primary',
  neutral: 'bg-muted-foreground',
} as const

const HEALTH_TONE = { healthy: 'success', degraded: 'warning', unhealthy: 'error' } as const

type ListSearch = DraftsSearch | { q?: string; sort?: WorkflowSort }

export function WorkflowsPage({
  mode,
  search,
  setSearch,
}: {
  mode: ListMode
  search: ListSearch
  setSearch: (patch: Partial<{ q: string; sort: string }>) => void
}) {
  const text = copy.list[mode]
  const queryClient = useQueryClient()
  const sort = search.sort ?? (mode === 'deployed' ? 'recent' : 'all')
  const list = useQuery(
    mode === 'deployed'
      ? workflowListQuery({ mode, sort: sort as WorkflowSort })
      : workflowListQuery({ mode, sort: sort as DraftSort }),
  )
  const term = search.q ?? ''
  const q = useDeferredValue(term).trim().toLowerCase()
  const [deleting, setDeleting] = useState<WorkflowRow | null>(null)

  const startRun = useStartRun()

  // The drafts endpoint also returns promoted drafts: each workflow shows in one list only.
  const rows = (list.data ?? []).filter((wf) => isDeployed(wf) === (mode === 'deployed'))
  const metrics = hasMetrics(rows)
  const shown = q
    ? rows.filter((wf) => `${wf.name} ${descriptionOf(wf)}`.toLowerCase().includes(q))
    : rows
  const empty = list.isSuccess && rows.length === 0
  const absent = mode === 'drafts' && list.isError && isDraftsAbsent(list.error)
  const sorts: readonly string[] = mode === 'deployed' ? WORKFLOW_SORTS : DRAFT_SORTS
  const sortLabel = SORT_LABEL[mode] as Record<string, string>

  const create = (
    <Button asChild size="sm" className="pointer-coarse:min-h-11">
      <Link to="/workflows/new">
        <Plus aria-hidden /> {copy.create}
      </Link>
    </Button>
  )

  let body
  if (absent)
    body = (
      <StateCard tone="info" title={copy.draftsAbsent}>
        {copy.draftsAbsentText}
      </StateCard>
    )
  else if (list.isError && !list.data)
    body = (
      <StateCard
        tone="error"
        title={copy.loadFailed}
        fix={reason(list.error)}
        action={
          <Button size="sm" variant="outline" onClick={() => void list.refetch()}>
            <RotateCw className="size-3.5" aria-hidden /> {copy.retry}
          </Button>
        }
      />
    )
  else if (list.isPending) body = <PageLoader label={text.loading} />
  else if (empty)
    body = (
      <EmptyState
        icon={Workflow}
        title={text.empty}
        action={
          <>
            <Button asChild variant="ghost">
              <Link to={mode === 'deployed' ? '/workflows/drafts' : '/workflows'}>
                {text.other}
              </Link>
            </Button>
            <Button asChild variant="outline">
              <Link to="/workflows/new">{copy.create}</Link>
            </Button>
          </>
        }
      >
        {text.emptyText}
      </EmptyState>
    )
  else if (!shown.length)
    body = (
      <EmptyState
        icon={SearchX}
        title={copy.noMatch}
        action={
          <Button size="sm" variant="outline" onClick={() => setSearch({ q: undefined })}>
            {copy.clearSearch}
          </Button>
        }
      >
        {copy.noMatchText}
      </EmptyState>
    )
  else
    body = (
      <ul aria-label={text.title} className="grid gap-3 sm:grid-cols-2 lg:grid-cols-3">
        {shown.map((wf) => (
          <li key={wf.id} className="flex min-w-0 [&>*]:min-w-0 [&>*]:flex-1">
            <WorkflowCard
              wf={wf}
              mode={mode}
              metrics={metrics}
              onRun={() => startRun(wf.id)}
              onDelete={() => setDeleting(wf)}
            />
          </li>
        ))}
      </ul>
    )

  return (
    <div className="space-y-4">
      <PageHeader title={text.title} actions={absent ? null : create} />
      <SectionNav current={mode === 'deployed' ? '/workflows' : '/workflows/drafts'} />
      {absent ? null : (
        <div className="flex flex-wrap items-center gap-2">
          <SearchInput
            aria-label={text.search}
            placeholder={copy.searchPlaceholder}
            value={term}
            onChange={(e) => setSearch({ q: e.target.value || undefined })}
            disabled={empty}
            className="w-full max-w-none sm:w-80"
          />
          {/* `main` ignores `sort` (W-3): a menu that changes nothing is hidden. */}
          {metrics ? (
            <Select
              value={sort}
              onValueChange={(v) => setSearch({ sort: v === sorts[0] ? undefined : v })}
              disabled={empty}
            >
              <SelectTrigger size="sm" aria-label={copy.sortLabel}>
                <SelectValue />
              </SelectTrigger>
              <SelectContent>
                {sorts.map((v) => (
                  <SelectItem key={v} value={v}>
                    {sortLabel[v]}
                  </SelectItem>
                ))}
              </SelectContent>
            </Select>
          ) : null}
        </div>
      )}
      {body}
      <DeleteWorkflowDialog
        workflow={deleting}
        onClose={() => setDeleting(null)}
        onDeleted={(wf) => {
          dropFromLists(queryClient, wf.id)
          toast.success(copy.deleted)
        }}
      />
    </div>
  )
}

/** Memoized: every search keystroke re-renders the page. */
const WorkflowCard = memo(function WorkflowCard({
  wf,
  mode,
  metrics,
  onRun,
  onDelete,
}: {
  wf: WorkflowRow
  mode: ListMode
  metrics: boolean
  onRun: () => void
  onDelete: () => void
}) {
  const navigate = useNavigate()
  const steps = stepsOf(wf)
  const agents = agentsOf(wf)
  const status = lastRun(wf)
  const health = wf.health && wf.health !== 'unknown' ? wf.health : null
  const description = descriptionOf(wf)
  const open = () =>
    void navigate({ to: '/workflows/$workflowId', params: { workflowId: wf.id }, search: {} })
  const chips = [
    copy.steps(steps),
    copy.runs(wf.execution_count ?? 0),
    metrics && wf.success_rate != null ? copy.success(fmtPct(wf.success_rate)) : '',
    metrics && wf.total_tokens ? copy.tokens(fmtTokens(wf.total_tokens)) : '',
    mode === 'drafts' && wf.updated_at
      ? copy.updated(relTime(wf.updated_at))
      : wf.created_at
        ? copy.created(fmtLongDay(wf.created_at))
        : '',
  ].filter(Boolean)
  return (
    // The title link stretches over the card; the menu sits above it.
    <Card
      data-id={wf.id}
      className="relative h-full gap-3 p-4 transition-colors hover:border-primary/40 has-[a:focus-visible]:outline-2 has-[a:focus-visible]:outline-ring"
    >
      <div className="flex items-start gap-2">
        <h2 className="min-w-0 flex-1 truncate font-medium">
          <Link
            to="/workflows/$workflowId"
            params={{ workflowId: wf.id }}
            search={{}}
            className="underline-offset-4 after:absolute after:inset-0 after:rounded-xl hover:underline focus-visible:outline-none"
          >
            {wf.name}
          </Link>
        </h2>
        <DropdownMenu>
          <DropdownMenuTrigger asChild>
            <Button
              variant="ghost"
              size="icon-sm"
              aria-label={`${copy.actions}: ${wf.name}`}
              className="relative z-10 -mt-1 -mr-2"
            >
              <EllipsisVertical aria-hidden />
            </Button>
          </DropdownMenuTrigger>
          <DropdownMenuContent align="end">
            <DropdownMenuItem onSelect={open}>{copy.open}</DropdownMenuItem>
            {steps > 0 ? <DropdownMenuItem onSelect={onRun}>{copy.runNow}</DropdownMenuItem> : null}
            <DropdownMenuItem variant="destructive" onSelect={onDelete}>
              {copy.delete}
            </DropdownMenuItem>
          </DropdownMenuContent>
        </DropdownMenu>
      </div>
      {description ? (
        <p className="line-clamp-2 text-sm text-muted-foreground" title={description}>
          {description}
        </p>
      ) : null}
      <ul aria-label={copy.details} className="flex flex-wrap gap-1.5">
        {chips.map((t) => (
          <li key={t}>
            <Chip>{t}</Chip>
          </li>
        ))}
        {health ? (
          <li>
            <ToneBadge tone={HEALTH_TONE[health]}>{HEALTH_LABEL[health]}</ToneBadge>
          </li>
        ) : null}
      </ul>
      <div className="mt-auto flex items-center gap-3 text-xs">
        {agents.length ? (
          <span className="min-w-0 flex-1 truncate text-muted-foreground">
            {agents.join(' · ')}
          </span>
        ) : null}
        {status ? (
          <span className="ml-auto flex shrink-0 items-center gap-1.5">
            <span aria-hidden className={cn('size-1.5 rounded-full', DOT[status.tone])} />
            {status.text}
            {status.at ? ` ${relTime(status.at)}` : ''}
          </span>
        ) : null}
      </div>
    </Card>
  )
})
