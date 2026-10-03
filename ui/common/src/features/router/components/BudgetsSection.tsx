/**
 * Budgets and Alerts (plans/feat-llm-router.md §5.1, approved variant A): one hairline table of the caller's budgets,
 * then a separate Alerts list. Every number is router-metered (token_usage) and covers routed calls only. The
 * contract is the proposed R-L10 (mocked; no server has it yet).
 */
import { AlertTriangle, CheckCircle2, Info, OctagonX } from 'lucide-react'
import { EmptyState } from '@/components/shared/state-card'
import { Badge } from '@/components/ui/badge'
import { Button } from '@/components/ui/button'
import { Skeleton } from '@/components/ui/skeleton'
import {
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeader,
  TableRow,
} from '@/components/ui/table'
import { fmtInt, fmtMoney, fmtShortDay } from '@/lib/format'
import { isBudgetsAbsent } from '@/lib/api/detect'
import { cn } from '@/lib/utils'
import { budgetForecast, budgetLabel, sortBudgets, stateView, usedPercent } from '../budgets'
import { copy } from '../copy'
import type { Budget, BudgetAlert, BudgetStatus, BudgetStatusResponse } from '../types'
import { LinkButton } from './bits'
import { SectionError } from './SectionError'

type Q<T> = {
  data?: T
  isPending: boolean
  isError: boolean
  error: unknown
  refetch: () => unknown
}

// A table from lg; stacked (with inline labels) below it, where the sidebar leaves too little room for the tracks.
const ROW = 'text-sm hover:bg-transparent max-lg:grid max-lg:gap-1.5 max-lg:px-4 max-lg:py-2.5'
const CELL = 'p-0 whitespace-normal max-lg:block lg:px-1.5 lg:py-2.5 lg:first:pl-4 lg:last:pr-4'
const HEAD = 'h-auto px-1.5 py-2 text-xs text-muted-foreground first:pl-4 last:pr-4'
/** A cell's name in the stacked layout, where the header row is hidden. */
const Label = ({ children }: { children: string }) => (
  <span className="text-muted-foreground lg:hidden">{children}: </span>
)

export function BudgetsSection({
  budgets,
  status,
  statusAt,
  alerts,
  agentName,
  resetsAt,
  pending,
  onNew,
  onEdit,
  onSwitchToAlert,
}: {
  budgets: Q<Budget[]>
  status: Q<BudgetStatusResponse>
  /** When the status was read: the forecast's clock, so elapsed days and the daily series agree. */
  statusAt: Date
  alerts: Q<BudgetAlert[]>
  agentName: (id: string) => string | undefined
  resetsAt: string
  /** Budgets with a switch to Alert only in flight. */
  pending: ReadonlySet<string>
  onNew: () => void
  onEdit: (b: Budget) => void
  onSwitchToAlert: (b: Budget) => void
}) {
  const list = sortBudgets(budgets.data ?? [], agentName)
  const byId = new Map((status.data?.data ?? []).map((s) => [s.budget_id, s]))
  const missing = isBudgetsAbsent(budgets.error) && !budgets.data
  return (
    <>
      <section
        id="router-budgets"
        aria-labelledby="router-budgets-h"
        className="scroll-mt-4 space-y-2"
      >
        <div className="flex flex-wrap items-start justify-between gap-2">
          <div>
            <h2 id="router-budgets-h" tabIndex={-1} className="text-sm font-semibold">
              {copy.budgetsTitle}
            </h2>
            <p className="text-xs text-muted-foreground">
              {copy.budgetsSubtitle(fmtShortDay(resetsAt))}
            </p>
            {list.length ? (
              <p className="text-xs text-muted-foreground">{copy.forecastNote}</p>
            ) : null}
          </div>
          {list.length ? (
            <Button size="sm" variant="outline" onClick={onNew} className="pointer-coarse:min-h-11">
              {copy.newBudget}
            </Button>
          ) : null}
        </div>
        {budgets.isPending ? (
          <div className="space-y-2" aria-busy="true">
            {[0, 1, 2].map((i) => (
              <Skeleton key={i} className="h-11" />
            ))}
          </div>
        ) : missing ? (
          // A live server without the proposed endpoints (as TokenOps' traces drawer does for /finops/top-traces).
          <div className="flex gap-2 rounded-lg border border-info/30 bg-info/5 p-3 text-sm">
            <Info className="mt-0.5 size-4 shrink-0 text-info" aria-hidden />
            <div>
              <p className="font-medium">{copy.budgetsMissing}</p>
              <p className="mt-1 text-xs text-muted-foreground">{copy.budgetsMissingBody}</p>
            </div>
          </div>
        ) : budgets.isError && !budgets.data ? (
          <SectionError
            error={budgets.error}
            onRetry={() => void budgets.refetch()}
            title={copy.budgetsFailed}
          />
        ) : list.length === 0 ? (
          <EmptyState
            title={copy.noBudgets}
            action={
              <Button size="sm" className="pointer-coarse:min-h-11" onClick={onNew}>
                {copy.createFirstBudget}
              </Button>
            }
          >
            {copy.noBudgetsText}
          </EmptyState>
        ) : (
          <div className="rounded-lg border border-border bg-card">
            <Table aria-label={copy.budgetsTitle} className="max-lg:block">
              <TableHeader className="max-lg:sr-only">
                <TableRow className="hover:bg-transparent">
                  <TableHead className={HEAD}>{copy.colScope}</TableHead>
                  <TableHead className={HEAD}>{copy.colUsedOfLimit}</TableHead>
                  <TableHead className={HEAD}>{copy.colForecast}</TableHead>
                  <TableHead className={HEAD}>{copy.colState}</TableHead>
                  <TableHead className={HEAD}>{copy.colAt100}</TableHead>
                  <TableHead className={HEAD}>
                    <span className="sr-only">{copy.colActions}</span>
                  </TableHead>
                </TableRow>
              </TableHeader>
              <TableBody className="max-lg:block">
                {list.map((b) => (
                  <Row
                    key={b.id}
                    budget={b}
                    status={byId.get(b.id)}
                    statusFailed={status.isError}
                    label={budgetLabel(b, agentName)}
                    statusAt={statusAt}
                    switching={pending.has(b.id)}
                    onEdit={() => onEdit(b)}
                    onSwitchToAlert={() => onSwitchToAlert(b)}
                  />
                ))}
              </TableBody>
            </Table>
          </div>
        )}
        {status.isError && list.length ? (
          <p className="text-xs text-destructive">
            {copy.budgetStatusFailed}{' '}
            <LinkButton
              className="text-xs font-normal text-inherit underline"
              onClick={() => void status.refetch()}
            >
              {copy.retry}
            </LinkButton>
          </p>
        ) : null}
      </section>
      {list.length ? (
        <AlertsSection alerts={alerts} budgets={list} agentName={agentName} onView={onEdit} />
      ) : null}
    </>
  )
}

function Row({
  budget,
  status,
  statusFailed,
  label,
  statusAt,
  switching,
  onEdit,
  onSwitchToAlert,
}: {
  budget: Budget
  status?: BudgetStatus
  statusFailed: boolean
  label: string
  statusAt: Date
  switching: boolean
  onEdit: () => void
  onSwitchToAlert: () => void
}) {
  const view = stateView(status)
  const fc = status ? budgetForecast(status, budget.limit_usd, statusAt) : null
  const pct = status ? usedPercent(status.used_usd, budget.limit_usd) : 0
  return (
    <TableRow data-budget={budget.id} className={ROW}>
      <TableCell className={cn(CELL, 'min-w-0 font-medium')}>
        <span className="block truncate">{label}</span>
        {budget.scope === 'owner' ? (
          <span className="block text-xs font-normal text-muted-foreground">
            {copy.budgetOwnerHint}
          </span>
        ) : null}
      </TableCell>
      <TableCell className={cn(CELL, 'min-w-0')}>
        <div className="flex items-center gap-2">
          {status ? (
            <>
              <span
                className="relative h-1.5 w-16 shrink-0 overflow-hidden rounded-full bg-muted"
                aria-hidden
              >
                <span
                  className={cn(
                    'absolute inset-y-0 left-0 rounded-full',
                    view?.kind === 'exceeded'
                      ? 'bg-destructive'
                      : view?.kind === 'warning'
                        ? 'bg-warning'
                        : 'bg-primary',
                  )}
                  style={{ width: `${pct}%` }}
                />
              </span>
              <span className="whitespace-nowrap">
                {copy.usedOf(fmtMoney(status.used_usd), fmtMoney(budget.limit_usd))}
              </span>
            </>
          ) : (
            <span className="text-muted-foreground">
              {statusFailed ? copy.usedOf('—', fmtMoney(budget.limit_usd)) : '…'}
            </span>
          )}
        </div>
        {status?.unpriced_calls ? (
          <span className="block text-xs text-warning">
            {copy.unpriced(fmtInt(status.unpriced_calls))}
          </span>
        ) : null}
      </TableCell>
      <TableCell className={cn(CELL, 'text-muted-foreground')}>
        <Label>{copy.colForecast}</Label>
        {!fc || !status ? (
          '—'
        ) : status.stopped ? (
          copy.forecastStopped
        ) : !fc.show ? (
          fc.hidden === 'early' ? (
            copy.forecastTooEarly
          ) : (
            copy.forecastNoSpend
          )
        ) : (
          <span className={cn('whitespace-nowrap', fc.overLimit && 'text-warning')}>
            {copy.forecastBand(fmtMoney(fc.low), fmtMoney(fc.high))}
            {fc.overLimit ? <span className="block text-xs">{copy.forecastOver}</span> : null}
          </span>
        )}
      </TableCell>
      <TableCell className={CELL}>
        {view ? <StateBadge kind={view.kind} label={view.label} /> : null}
      </TableCell>
      <TableCell className={CELL}>
        <Label>{copy.colAt100}</Label>
        {budget.action === 'stop' ? copy.actionStop : copy.actionAlert}
        {status?.stopped ? (
          <span className="block text-xs text-destructive">
            {copy.stoppedUntil(fmtShortDay(status.resets_at))}
          </span>
        ) : null}
      </TableCell>
      <TableCell className={CELL}>
        <div className="flex flex-wrap gap-x-3 gap-y-1 lg:justify-end">
          <LinkButton
            data-raise={budget.id}
            onClick={onEdit}
            aria-label={`${copy.raiseLimit}: ${label}`}
          >
            {copy.raiseLimit}
          </LinkButton>
          {budget.action === 'stop' ? (
            <LinkButton
              disabled={switching}
              onClick={onSwitchToAlert}
              aria-label={`${copy.switchToAlert}: ${label}`}
            >
              {copy.switchToAlert}
            </LinkButton>
          ) : null}
        </div>
      </TableCell>
    </TableRow>
  )
}

function StateBadge({ kind, label }: { kind: 'ok' | 'warning' | 'exceeded'; label: string }) {
  const Icon = kind === 'exceeded' ? OctagonX : kind === 'warning' ? AlertTriangle : CheckCircle2
  return (
    <Badge
      data-state={kind}
      variant={kind === 'exceeded' ? 'outline' : kind === 'warning' ? 'warning' : 'success'}
      className={cn(
        'font-normal [&>svg]:size-3.5',
        kind === 'exceeded' && 'border-destructive/40 bg-destructive/10 text-destructive',
      )}
    >
      <Icon aria-hidden /> {label}
    </Badge>
  )
}

function AlertsSection({
  alerts,
  budgets,
  agentName,
  onView,
}: {
  alerts: Q<BudgetAlert[]>
  budgets: readonly Budget[]
  agentName: (id: string) => string | undefined
  onView: (b: Budget) => void
}) {
  const byId = new Map(budgets.map((b) => [b.id, b]))
  const rows = (alerts.data ?? []).filter((a) => byId.has(a.budget_id))
  return (
    <section id="router-alerts" aria-labelledby="router-alerts-h" className="scroll-mt-4 space-y-2">
      <div>
        <h2 id="router-alerts-h" className="text-sm font-semibold">
          {copy.alertsTitle}
        </h2>
        <p className="text-xs text-muted-foreground">{copy.alertsSubtitle}</p>
      </div>
      {alerts.isPending ? (
        <Skeleton className="h-11" />
      ) : alerts.isError && !alerts.data ? (
        <SectionError error={alerts.error} onRetry={() => void alerts.refetch()} />
      ) : rows.length === 0 ? (
        <EmptyState title={copy.noAlerts} />
      ) : (
        <ul
          aria-label={copy.alertsTitle}
          className="divide-y divide-border rounded-lg border border-border bg-card"
        >
          {rows.map((a) => {
            const b = byId.get(a.budget_id)
            if (!b) return null // rows are filtered to known budgets above
            const label = budgetLabel(b, agentName)
            return (
              <li
                key={a.id}
                className="flex flex-wrap items-center gap-x-4 gap-y-1 px-4 py-2.5 text-sm"
              >
                <span className="min-w-0 flex-1">
                  {copy.alertCrossed(
                    b.scope === 'owner' ? copy.alertOwner : label,
                    a.threshold,
                    fmtMoney(a.amount_usd),
                    a.stopped,
                  )}
                </span>
                <time dateTime={a.at} className="text-xs text-muted-foreground">
                  {fmtShortDay(a.at)}
                </time>
                <LinkButton
                  className="text-xs"
                  onClick={() => onView(b)}
                  aria-label={`${copy.viewBudget}: ${label}`}
                >
                  {copy.viewBudget}
                </LinkButton>
              </li>
            )
          })}
        </ul>
      )}
    </section>
  )
}
