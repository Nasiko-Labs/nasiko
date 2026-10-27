/**
 * The panels for the village's own places (`place:<id>`), beside the insight panel that explains everything else
 * (plans/feat-village-overview.md V20, V22, V23, V27):
 *
 * - Town hall: governance, in three tabs. Allocation (how the pool is carved, the basic team-wise version; its real
 *   shape is still open), Policies (the routers and where they are set) and Guardrails (to come: walls and
 *   checkpoints). A project starts here. The surface shows state and the page that owns it sets it (V6).
 * - Token battery: the org's budget for the month, each team's line, and where to set them.
 * - Orchestrator: how many agents it is coordinating, the parking by colour, and what waits on you.
 * - Observability (the walking eye): what the product's own agents did, by their traces.
 * - Piggy bank: what optimizations saved, with an honest empty state when there is no baseline.
 * - Opportunities: the upgrades the log suggests, and what was saved (V50).
 * - Workflows (the clock tower): every workflow and its runs (V53).
 * - Project plots: the project, its team and its workflows.
 */
import { Link } from '@tanstack/react-router'
import { X } from 'lucide-react'
import { useState, type ReactNode } from 'react'
import { Button, asButton } from '@nasiko/ui/primitives/button'
import { ScrollArea } from '@nasiko/ui/primitives/scroll-area'
import { Tabs, TabsList, TabsTrigger } from '@nasiko/ui/primitives/tabs'
import { fmtMoney } from '@nasiko/ui/lib/format'
import { cn } from '@nasiko/ui/lib/utils'
import type { EstateView } from '../estate/useEstateView'
import { villageCopy as copy } from './copy'
import { PoseMark } from './Hud'
import { beaconOf, type Beacon, type PlaceId, type Village } from './model'
import { BEACON } from './palette'

function Head({ title, sub, onClose }: { title: string; sub: string; onClose(): void }) {
  return (
    <div className="sticky top-0 z-10 flex items-start gap-2 bg-card/95 px-4 pt-3.5 pb-2 backdrop-blur-sm">
      <div className="min-w-0 flex-1">
        <h3 className="truncate text-sm font-semibold">{title}</h3>
        <p className="text-xs text-muted-foreground">{sub}</p>
      </div>
      <Button
        variant="ghost"
        size="icon-sm"
        className="-mr-1.5 text-muted-foreground"
        aria-label={copy.close}
        onClick={onClose}
      >
        <X aria-hidden />
      </Button>
    </div>
  )
}

function Section({ title, children }: { title: string; children: ReactNode }) {
  return (
    <section className="border-b px-4 py-3 last:border-b-0">
      <h4 className="mb-2 text-xs font-semibold text-muted-foreground">{title}</h4>
      {children}
    </section>
  )
}

/** Spent so far (gold), projected by month end (dashed), the line (black). */
function BudgetBar({
  spent,
  projected,
  limit,
}: {
  spent: number
  projected: number
  limit: number
}) {
  const mx = Math.max(1.08, (projected / limit) * 1.05)
  const s = (spent / limit / mx) * 100
  const p = (projected / limit / mx) * 100
  const over = projected > limit
  return (
    <div className="relative my-2.5 h-3 rounded-full border bg-muted">
      <div
        className="absolute inset-y-0 left-0 rounded-full"
        style={{ width: `${s.toFixed(1)}%`, background: 'linear-gradient(#f7d25c,#e9ad1f)' }}
      />
      {p > s ? (
        <div
          className={cn(
            'absolute -inset-y-px rounded-r-full border-[1.5px] border-l-0 border-dashed',
            over ? 'border-destructive bg-destructive/10' : 'border-muted-foreground/60',
          )}
          style={{ left: `${s.toFixed(1)}%`, width: `${(p - s).toFixed(1)}%` }}
        />
      ) : null}
      <div
        className="absolute -inset-y-1.5 w-0.5 rounded bg-foreground"
        style={{ left: `calc(${(100 / mx).toFixed(1)}% - 1px)` }}
      />
    </div>
  )
}

function Townhall({ v, village }: { v: EstateView; village: Village }) {
  const [tab, setTab] = useState<'alloc' | 'policy' | 'guardrails'>('alloc')
  const teams = village.teams
  const funded = teams.flatMap((c) => (c.budget ? [{ ...c, budget: c.budget }] : []))
  const limit = funded.reduce((n, c) => n + c.budget.limit, 0)
  return (
    <>
      <div className="px-4 pt-3 pb-1">
        <Tabs value={tab} onValueChange={(t) => setTab(t as typeof tab)}>
          <TabsList className="w-full">
            {(['alloc', 'policy', 'guardrails'] as const).map((t) => (
              <TabsTrigger key={t} value={t} className="flex-1 text-xs">
                {copy.panel.tabs[t]}
              </TabsTrigger>
            ))}
          </TabsList>
        </Tabs>
      </div>
      {tab === 'alloc' ? (
        <Section title={copy.panel.allocation}>
          <div className="my-2 flex h-4 overflow-hidden rounded">
            {funded.map((c) => (
              <i
                key={c.id}
                title={c.name}
                className="block h-full"
                style={{
                  width: `${((c.budget.limit / (limit || 1)) * 100).toFixed(1)}%`,
                  background: c.color,
                }}
              />
            ))}
          </div>
          <ul className="flex flex-col gap-1 text-sm">
            {teams.map((c) => (
              <li key={c.id} className="flex items-center gap-2">
                <i className="size-2.5 shrink-0 rounded-sm" style={{ background: c.color }} />
                <span className="flex-1">{c.name}</span>
                <span className="text-xs text-muted-foreground tabular-nums">
                  {c.budget
                    ? `${fmtMoney(c.budget.limit)} · ${Math.round((c.budget.limit / (limit || 1)) * 100)}%`
                    : copy.chip.noBudget}
                </span>
              </li>
            ))}
          </ul>
          <p className="mt-2.5 text-xs text-muted-foreground">{copy.panel.allocationNote}</p>
        </Section>
      ) : null}
      {tab === 'policy' ? (
        <>
          <Section title={copy.panel.routers}>
            <ul className="flex flex-col gap-1.5 text-sm">
              {v.estate.routers.map((r) => (
                <li key={r.id} className="flex items-baseline gap-2">
                  <span className="flex-1">{r.name}</span>
                  <span className="text-xs text-muted-foreground">{r.note}</span>
                </li>
              ))}
            </ul>
            {village.hud.routedShare !== null ? (
              <p className="mt-2.5 flex justify-between text-sm">
                <span className="text-muted-foreground">{copy.panel.routed}</span>
                <b className="font-semibold tabular-nums">
                  {Math.round(village.hud.routedShare * 100)}%
                </b>
              </p>
            ) : null}
            <p className="mt-2.5 text-xs text-muted-foreground">{copy.panel.policyNote}</p>
          </Section>
          <div className="flex flex-wrap gap-2 px-4 py-3">
            <Link to="/router" {...asButton({ size: 'sm' })}>
              {copy.panel.routerPolicy}
            </Link>
            <Link to="/settings" {...asButton({ variant: 'outline', size: 'sm' })}>
              {copy.panel.flowLimits}
            </Link>
          </div>
        </>
      ) : null}
      {tab === 'guardrails' ? (
        <>
          <Section title={copy.panel.guardrailsTitle}>
            <p className="text-xs text-muted-foreground">{copy.panel.guardrailsNote}</p>
          </Section>
        </>
      ) : null}
    </>
  )
}

function Tank({ v, village }: { v: EstateView; village: Village }) {
  const teams = village.teams
  const groups = v.estate.groups.flatMap((g) => (g.budget ? [g.budget] : []))
  const limit = groups.reduce((n, b) => n + b.limit_usd, 0)
  const spent = groups.reduce((n, b) => n + b.spent_usd, 0)
  const projected = groups.reduce((n, b) => n + b.projected_usd, 0)
  const own = v.view.scope === 'mine' ? v.estate.allowance : null
  return (
    <>
      {own ? (
        <Section title={copy.panel.allowance}>
          <p className="text-2xl font-semibold tabular-nums">
            {fmtMoney(own.spent_usd)}
            <span className="ml-1.5 text-sm font-normal text-muted-foreground">
              {copy.panel.ofLimit(own.limit_usd)}
            </span>
          </p>
          <BudgetBar spent={own.spent_usd} projected={own.projected_usd} limit={own.limit_usd} />
        </Section>
      ) : null}
      {limit > 0 ? (
        <Section title={copy.panel.orgBudget}>
          <p className="text-2xl font-semibold tabular-nums">
            {fmtMoney(spent)}
            <span className="ml-1.5 text-sm font-normal text-muted-foreground">
              {copy.panel.ofLimit(limit)}
            </span>
          </p>
          <BudgetBar spent={spent} projected={projected} limit={limit} />
          <p className="flex justify-between text-sm">
            <span className="text-muted-foreground">{copy.panel.onPace}</span>
            <b
              className={cn('font-semibold tabular-nums', projected > limit && 'text-destructive')}
            >
              {fmtMoney(projected)} · {Math.round((projected / limit) * 100)}%
            </b>
          </p>
        </Section>
      ) : null}
      <Section title={copy.panel.teams}>
        <ul className="flex flex-col gap-1.5 text-sm">
          {teams.map((c) => (
            <li key={c.id} className="flex items-center gap-2">
              <i className="size-2.5 shrink-0 rounded-sm" style={{ background: c.color }} />
              <span className="min-w-0 flex-1 truncate">{c.name}</span>
              <span className="text-xs text-muted-foreground tabular-nums">
                {c.budget
                  ? `${fmtMoney(c.budget.spent)} / ${fmtMoney(c.budget.limit)}`
                  : copy.chip.noBudget}
              </span>
              <span
                className={cn(
                  'w-14 text-right text-xs font-medium tabular-nums',
                  c.pace !== null && c.pace > 1 ? 'text-destructive' : 'text-muted-foreground',
                )}
              >
                {c.pace === null ? '—' : `${Math.round(c.pace * 100)}%`}
              </span>
            </li>
          ))}
        </ul>
        <p className="mt-2.5 text-xs text-muted-foreground">{copy.panel.tankNote}</p>
      </Section>
      <div className="flex flex-wrap gap-2 px-4 py-3">
        <Link to="/router" hash="router-budgets" {...asButton({ size: 'sm' })}>
          {copy.panel.setBudgets}
        </Link>
        <Link to="/settings" {...asButton({ variant: 'outline', size: 'sm' })}>
          {copy.panel.flowLimits}
        </Link>
      </div>
    </>
  )
}

function Light({ b }: { b: Beacon }) {
  return (
    <i
      aria-hidden
      className="inline-block size-2.5 shrink-0 rounded-full border border-foreground/40"
      style={{ background: BEACON[b] }}
    />
  )
}

function Orchestrator({ village }: { village: Village }) {
  const live = village.cars.filter((t) => t.pose === 'live').length
  const by = new Map<Beacon, number>()
  for (const c of village.cars) by.set(beaconOf(c.pose), (by.get(beaconOf(c.pose)) ?? 0) + 1)
  const d = village.districts.bay
  return (
    <>
      <Section title={copy.hud.fleet}>
        <p className="flex items-center gap-2 text-sm font-medium">
          <PoseMark pose={live ? 'live' : 'idle'} />
          {copy.panel.orchestrator(live)}
        </p>
        <p className="mt-1 text-xs text-muted-foreground">
          {copy.panel.waiting(village.log.approve.length)}
        </p>
      </Section>
      <Section title={copy.panel.depot}>
        <ul className="flex flex-col gap-1 text-sm">
          {(['running', 'slow', 'broken', 'asleep'] as const).map((b) => (
            <li key={b} className="flex items-center gap-2">
              <Light b={b} />
              <span className="flex-1">{copy.beacon[b]}</span>
              <span className="text-xs text-muted-foreground tabular-nums">{by.get(b) ?? 0}</span>
            </li>
          ))}
        </ul>
        <p className="mt-2.5 text-xs text-muted-foreground">
          {copy.panel.depotNote(d.drawn, d.total)}
        </p>
      </Section>
      <div className="flex flex-wrap gap-2 px-4 py-3">
        <Link to="/chat" {...asButton({ size: 'sm' })}>
          {copy.panel.openOrchestrator}
        </Link>
        <Link to="/agents" {...asButton({ variant: 'outline', size: 'sm' })}>
          {copy.panel.openAgents}
        </Link>
      </div>
    </>
  )
}

function Observatory({ village }: { village: Village }) {
  const w = village.watch
  return (
    <>
      <Section title={copy.panel.watchTitle}>
        <p className="flex justify-between text-sm">
          <span className="text-muted-foreground">{copy.panel.runs}</span>
          <b className="font-semibold tabular-nums">{w.runs.toLocaleString('en-US')}</b>
        </p>
        <p className="mt-1 flex justify-between text-sm">
          <span className="text-muted-foreground">{copy.panel.success}</span>
          <b className="font-semibold tabular-nums">
            {w.success === null ? '—' : `${w.success.toFixed(1)}%`}
          </b>
        </p>
        <p className="mt-2.5 text-xs text-muted-foreground">{copy.panel.watchNote}</p>
      </Section>
      <div className="flex flex-wrap gap-2 px-4 py-3">
        <Link to="/sessions" {...asButton({ size: 'sm' })}>
          {copy.panel.openSessions}
        </Link>
        <Link to="/flows" {...asButton({ variant: 'outline', size: 'sm' })}>
          {copy.panel.openFlows}
        </Link>
      </div>
    </>
  )
}

function RailList({ rails }: { rails: Village['workflows'] }) {
  return rails.length ? (
    <ul className="flex flex-col gap-2 text-sm">
      {rails.map((r) => (
        <li key={r.id} className="flex items-baseline gap-2">
          <Link
            to="/workflows/$workflowId"
            params={{ workflowId: r.id }}
            className="min-w-0 flex-1 truncate font-medium underline-offset-2 hover:underline"
          >
            {r.name}
          </Link>
          <span className="text-xs text-muted-foreground tabular-nums">
            {copy.panel.steps(r.stops.length)} · {copy.panel.runsIn(r.runs)}
          </span>
        </li>
      ))}
    </ul>
  ) : (
    <p className="text-sm text-muted-foreground">{copy.panel.noRails}</p>
  )
}

function WorkflowsPanel({ village }: { village: Village }) {
  return (
    <>
      <Section title={copy.panel.railsTitle}>
        <RailList rails={village.workflows} />
        <p className="mt-2.5 text-xs text-muted-foreground">{copy.panel.workflowsNote}</p>
      </Section>
      <div className="px-4 py-3">
        <Link to="/workflows" {...asButton({ size: 'sm' })}>
          {copy.panel.openWorkflows}
        </Link>
      </div>
    </>
  )
}

function Opportunities({ village, saved }: { village: Village; saved: number | null }) {
  const ideas = village.log.upgrade
  return (
    <>
      <Section title={ideas.length ? copy.panel.orbOn(ideas.length) : copy.panel.orbOff}>
        {ideas.length ? (
          <ul className="flex flex-col gap-2 text-sm">
            {ideas.map((o) => (
              <li key={o.key}>
                <p className="font-medium">{o.title}</p>
                <p className="text-xs text-muted-foreground">{o.detail}</p>
              </li>
            ))}
          </ul>
        ) : null}
        <p className="mt-2.5 text-xs text-muted-foreground">{copy.panel.orbNote}</p>
      </Section>
      {saved ? (
        <Section title={copy.panel.orbSaved}>
          <p className="text-2xl font-semibold tabular-nums">{fmtMoney(saved)}</p>
        </Section>
      ) : null}
      <div className="px-4 py-3">
        <Link to="/optimization" {...asButton({ size: 'sm' })}>
          {copy.panel.openOptimization}
        </Link>
      </div>
    </>
  )
}

function YardPanel({ village, id }: { village: Village; id: string }) {
  const y = village.yards.find((x) => x.id === id)
  if (!y) return <Section title={copy.panel.railsTitle}>{copy.panel.noRails}</Section>
  return (
    <>
      <Section title={copy.panel.railsTitle}>
        <RailList rails={y.rails} />
      </Section>
    </>
  )
}

export function PlacePanel({
  place,
  v,
  village,
  saved,
  onClose,
}: {
  place: PlaceId | `yard:${string}`
  v: EstateView
  village: Village
  saved: number | null
  onClose(): void
}) {
  let title: string
  let sub: string
  let body: ReactNode
  if (place.startsWith('yard:')) {
    const id = place.slice(5)
    const y = village.yards.find((x) => x.id === id)
    title = y?.unfiled ? copy.place.unfiled : (y?.name ?? copy.place.unfiled)
    sub = y?.unfiled ? copy.place.unfiledSub : y?.org ? copy.place.yardOrg : copy.place.yardSub
    body = <YardPanel village={village} id={id} />
  } else
    switch (place) {
      case 'townhall':
        title = copy.place.townhall
        sub = copy.place.townhallSub
        body = <Townhall v={v} village={village} />
        break
      case 'tank':
        title = copy.place.tank
        sub = copy.place.tankSub
        body = <Tank v={v} village={village} />
        break
      case 'orchestrator':
        title = copy.place.orchestrator
        sub = copy.place.orchestratorSub
        body = <Orchestrator village={village} />
        break
      case 'observatory':
        title = copy.place.observatory
        sub = copy.place.observatorySub
        body = <Observatory village={village} />
        break
      case 'piggy':
        title = copy.place.piggy
        sub = copy.place.piggySub
        body = (
          <>
            <Section title={copy.panel.savedThisRange}>
              {saved ? (
                <>
                  <p className="text-2xl font-semibold tabular-nums">{fmtMoney(saved)}</p>
                  <p className="mt-1 text-xs text-muted-foreground">{copy.panel.savedNote}</p>
                </>
              ) : (
                <p className="text-sm text-muted-foreground">{copy.panel.savedEmpty}</p>
              )}
            </Section>
            <div className="px-4 py-3">
              <Link to="/optimization" {...asButton({ size: 'sm' })}>
                {copy.panel.openOptimization}
              </Link>
            </div>
          </>
        )
        break
      case 'workflows':
        title = copy.place.workflows
        sub = copy.place.workflowsSub
        body = <WorkflowsPanel village={village} />
        break
      default:
        title = copy.place.orb
        sub = copy.place.orbSub
        body = <Opportunities village={village} saved={saved} />
    }
  return (
    <ScrollArea className="h-full" data-testid="village-place-panel">
      <Head title={title} sub={sub} onClose={onClose} />
      {body}
    </ScrollArea>
  )
}
