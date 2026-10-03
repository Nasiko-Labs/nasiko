/**
 * Budgets (the router's R2 budgets, plans/feat-llm-router.md §5.1): the viewer's own monthly budget in full (used of
 * limit, a bar notched at each alert mark, days left, forecast, what happens at 100%), then the agent budgets nearest
 * their limits. The page doesn't render it on a server without budgets. Bars use status tokens by state, and every
 * state is also a word (never colour alone).
 */
import { Link } from '@tanstack/react-router'
import { CircleAlert, CircleCheck, TriangleAlert } from 'lucide-react'
import { useRef } from 'react'
import { Button } from '@/components/ui/button'
import { Progress } from '@/components/ui/progress'
import { Separator } from '@/components/ui/separator'
import { fmtMoney, fmtPct } from '@/lib/format'
import { cn } from '@/lib/utils'
import type { BudgetCard, BudgetLine } from '../api'
import { copy } from '../copy'
import { BUDGET_AGENT_ROWS } from '../tuning'
import { Card, CardError, CardSkeleton, TOUCH } from './Card'

const HASH = 'router-budgets'

const STATE = {
  ok: {
    Icon: CircleCheck,
    text: 'text-success',
    bar: '[&>[data-slot=progress-indicator]]:bg-primary',
  },
  warning: {
    Icon: TriangleAlert,
    text: 'text-warning',
    bar: '[&>[data-slot=progress-indicator]]:bg-warning',
  },
  exceeded: {
    Icon: CircleAlert,
    text: 'text-destructive',
    bar: '[&>[data-slot=progress-indicator]]:bg-destructive',
  },
} as const

export function Budget({
  data,
  month,
  className,
}: {
  data: BudgetCard
  month: string
  className?: string
}) {
  const titleRef = useRef<HTMLHeadingElement>(null)
  const { own, agents } = data
  return (
    <Card
      id="overview-budget"
      title={copy.budget.title}
      meta={month}
      to="/router"
      linkHash={HASH}
      linkLabel={copy.budget.link}
      titleRef={titleRef}
      className={className}
    >
      {data.isPending ? (
        <CardSkeleton rows={4} />
      ) : data.error ? (
        <CardError what={copy.budget.what} onRetry={data.retry} titleRef={titleRef} />
      ) : !own && !agents.length ? (
        <p className="text-sm">
          {copy.budget.none}{' '}
          <Button asChild variant="link" size="sm" className={`h-auto px-0 ${TOUCH}`}>
            <Link to="/router" hash={HASH}>
              {copy.budget.set}
            </Link>
          </Button>
        </p>
      ) : (
        <div className="flex flex-col gap-3">
          {own ? <OwnBudget line={own} /> : null}
          {own && agents.length ? <Separator /> : null}
          {agents.length ? (
            <div>
              <h3 className="text-xs font-medium text-muted-foreground">{copy.budget.agents}</h3>
              <ul className="mt-1 divide-y divide-border">
                {agents.slice(0, BUDGET_AGENT_ROWS).map((l) => (
                  <AgentBudget key={l.id} line={l} />
                ))}
              </ul>
            </div>
          ) : null}
        </div>
      )}
    </Card>
  )
}

function Meter({ line, className }: { line: BudgetLine; className?: string }) {
  return (
    <div aria-hidden className={cn('relative', className)}>
      <Progress value={line.pct} className={cn('h-1.5 bg-muted', STATE[line.state].bar)} />
      {/* Each alert mark below 100% is a notch in the surface colour. */}
      {line.thresholds
        .filter((t) => t > 0 && t < 100)
        .map((t) => (
          <span
            key={t}
            className="absolute inset-y-0 w-0.5 -translate-x-1/2 bg-card"
            style={{ left: `${t}%` }}
          />
        ))}
    </div>
  )
}

function StateWord({ line }: { line: BudgetLine }) {
  const s = line.stopped ? STATE.exceeded : STATE[line.state]
  return (
    <span className={cn('inline-flex items-center gap-1', s.text)}>
      <s.Icon className="size-3.5 shrink-0" aria-hidden />
      {copy.budget.state(line.state, line.stopped)}
    </span>
  )
}

function OwnBudget({ line }: { line: BudgetLine }) {
  const f = line.forecast
  return (
    <div className="flex flex-col gap-2">
      <div className="flex flex-wrap items-baseline justify-between gap-x-3">
        <p className="text-sm">
          <span className="text-lg font-semibold tabular-nums">{fmtMoney(line.used)}</span>{' '}
          <span className="text-muted-foreground tabular-nums">
            {copy.budget.of(fmtMoney(line.limit))}
          </span>
        </p>
        <span className="text-xs">
          <StateWord line={line} />
        </span>
      </div>
      <Meter line={line} />
      <p className="flex justify-between text-xs text-muted-foreground tabular-nums">
        <span>{copy.budget.used(fmtPct(line.pct))}</span>
        <span>{copy.budget.daysLeft(line.daysLeft)}</span>
      </p>
      <dl className="mt-1 grid grid-cols-[1fr_auto] gap-x-3 gap-y-1.5 text-xs">
        <dt className="text-muted-foreground">{copy.budget.forecast}</dt>
        <dd className="text-right tabular-nums">
          {f.show && f.low !== null && f.high !== null ? (
            <span className={cn(f.overLimit && 'inline-flex items-center gap-1 text-warning')}>
              {f.overLimit ? <TriangleAlert className="size-3.5" aria-hidden /> : null}
              {f.low === f.high ? fmtMoney(f.high) : `${fmtMoney(f.low)}–${fmtMoney(f.high)}`}
              {f.overLimit ? `, ${copy.budget.overLimit}` : ''}
            </span>
          ) : f.hidden === 'no-spend' ? (
            copy.budget.noSpend
          ) : (
            copy.budget.tooEarly
          )}
        </dd>
        <dt className="text-muted-foreground">{copy.budget.alerts}</dt>
        <dd className="text-right tabular-nums">
          {line.thresholds.map((t) => `${t}%`).join(', ')}
        </dd>
        <dt className="text-muted-foreground">{copy.budget.atLimit}</dt>
        <dd className="text-right">
          {line.action === 'stop' ? copy.budget.stopCalls : copy.budget.alertOnly}
        </dd>
      </dl>
      {f.show && !f.overLimit && f.high !== null && f.high < line.limit ? (
        <p className="mt-1 flex items-center gap-1.5 rounded-md bg-success/10 px-2.5 py-2 text-xs text-success">
          <CircleCheck className="size-3.5 shrink-0" aria-hidden />
          {copy.budget.underBudget(fmtMoney(line.limit - f.high))}
        </p>
      ) : null}
    </div>
  )
}

function AgentBudget({ line }: { line: BudgetLine }) {
  return (
    <li className="flex flex-col gap-1.5 py-2 text-xs">
      <div className="flex items-center justify-between gap-2">
        <span className="truncate font-medium">{line.name}</span>
        <span className="flex shrink-0 items-center gap-3 tabular-nums">
          <span className="text-muted-foreground">
            {fmtMoney(line.used)} {copy.budget.of(fmtMoney(line.limit))}
          </span>
          <StateWord line={line} />
        </span>
      </div>
      <Meter line={line} />
    </li>
  )
}
