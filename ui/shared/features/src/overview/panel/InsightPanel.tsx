/**
 * The insight panel beside the map (docs/requirements/overview.md, "The insight panel"). With nothing selected it is
 * the action list: what needs you (the waiting requests, Needs you's own rows), what is falling behind (the problems,
 * highest score first) and the opportunities. With a selection it explains that selection and offers its one action.
 * Back walks the selections made in this panel; Close (or Escape on the map) clears. One polite live region, so a
 * screen reader hears what a selection opened.
 */
import { Link } from '@tanstack/react-router'
import { ArrowLeft, Coins, Hand, Link2, TriangleAlert, X, type LucideIcon } from 'lucide-react'
import type { HitlKind } from '@nasiko/features/chat/types'
import type { ReactNode } from 'react'
import { Button, asButton } from '@nasiko/ui/primitives/button'
import { ScrollArea } from '@nasiko/ui/primitives/scroll-area'
import { fmtInt, fmtLatency, fmtMoney, fmtShortDay } from '@nasiko/ui/lib/format'
import { cn } from '@nasiko/ui/lib/utils'
import type { NeedsYou } from '../api'
import { NeedRowView } from '../components/NeedsYou'
import { copy as overviewCopy } from '../copy'
import { estateCopy as copy } from '../estate/copy'
import { flowBadges, isProblemState, KIND_ORDER, pace, problemsOf } from '../estate/model'
import type { Opportunity, PanelId, Problem, ProblemKind } from '../estate/types'
import type { EstateView } from '../estate/useEstateView'
import type { Selection } from '../map/selection'
import { STRONG_BG, SWATCH } from '../map/tones'
import { ProblemCard, type ProblemActions } from './ProblemCard'
import { homeOf } from './home'
import { TargetLink } from './targets'

export interface PanelProps {
  v: EstateView
  sel: Selection | null
  select(s: Selection): void
  back: (() => void) | null
  close(): void
  needs: NeedsYou
  actions: ProblemActions
  filtered: boolean
  clearFilters(): void
  now: number
}

const VISIBLE = 6

export function InsightPanel(props: PanelProps) {
  const { v, sel } = props
  let body: ReactNode
  if (!sel) body = <DefaultView {...props} />
  else
    switch (sel.type) {
      case 'node':
        body = <NodeView {...props} id={sel.id} />
        break
      case 'problem': {
        const p = v.estate.problems.find((x) => x.id === sel.id)
        body = p ? (
          <>
            <Head {...props} title={copy.insight.problem} sub={copy.kindLong[p.kind]} />
            <div className="px-4 pt-1">
              <ProblemCard problem={p} v={v} actions={props.actions} />
            </div>
            <Section title={copy.insight.involved}>
              <Chips ids={p.entities} {...props} />
            </Section>
          </>
        ) : (
          <DefaultView {...props} />
        )
        break
      }
      case 'flow':
        body = <FlowView {...props} id={sel.id} />
        break
      case 'chain':
        body = <ChainView {...props} id={sel.id} />
        break
      case 'opp':
        body = <OppView {...props} id={sel.id} />
        break
      case 'panel':
        body = <CardView {...props} id={sel.id} kind={sel.kind} />
        break
      case 'tile':
        body = <TileView {...props} id={sel.id} day={sel.day} />
        break
      case 'sess':
        body = <SessionView {...props} id={sel.id} index={sel.index} />
        break
    }
  return (
    <section
      aria-label={copy.insight.label}
      aria-live="polite"
      className="flex h-full min-h-0 flex-col"
      data-testid="insight-panel"
    >
      <ScrollArea className="h-full min-h-0 [&>[data-slot=scroll-area-viewport]>div]:block!">
        <div className="pb-4">{body}</div>
      </ScrollArea>
    </section>
  )
}

function Head({
  title,
  sub,
  icon: Icon,
  back,
  close,
  sel,
}: Pick<PanelProps, 'back' | 'close' | 'sel'> & {
  title: ReactNode
  sub?: ReactNode
  icon?: LucideIcon
}) {
  return (
    <div className="sticky top-0 z-10 bg-card/95 px-4 pt-3.5 pb-2 backdrop-blur-sm">
      <div className="flex items-center gap-1.5">
        {sel && back ? (
          <Button
            variant="ghost"
            size="icon-sm"
            className="-ml-1.5 text-muted-foreground"
            aria-label={copy.insight.back}
            onClick={back}
          >
            <ArrowLeft aria-hidden />
          </Button>
        ) : null}
        <h3 className="flex min-w-0 flex-1 items-center gap-1.5 text-sm font-semibold">
          {Icon ? <Icon aria-hidden className="size-4 shrink-0 text-muted-foreground" /> : null}
          <span className="truncate">{title}</span>
        </h3>
        {sel ? (
          <Button
            variant="ghost"
            size="icon-sm"
            className="-mr-1.5 text-muted-foreground"
            aria-label={copy.insight.close}
            onClick={close}
          >
            <X aria-hidden />
          </Button>
        ) : null}
      </div>
      {sub ? <p className="mt-0.5 text-xs text-muted-foreground">{sub}</p> : null}
    </div>
  )
}

function Section({
  title,
  aside,
  children,
  icon: Icon,
}: {
  title: ReactNode
  aside?: ReactNode
  children: ReactNode
  icon?: LucideIcon
}) {
  return (
    <section className="mx-4 mt-3 border-t border-border/60 pt-3">
      <div className="mb-1.5 flex items-center gap-1.5">
        {Icon ? <Icon aria-hidden className="size-4 shrink-0 text-muted-foreground" /> : null}
        <h4 className="flex-1 text-xs font-semibold">{title}</h4>
        {aside}
      </div>
      {children}
    </section>
  )
}

function KindBadges({ counts }: { counts: Partial<Record<ProblemKind, number>> }) {
  return (
    <span className="inline-flex gap-1">
      {KIND_ORDER.filter((k) => counts[k]).map((k) => (
        <span
          key={k}
          className={cn(
            'grid h-4 min-w-4 place-items-center rounded-full px-1 text-3xs font-semibold text-card',
            STRONG_BG[k],
          )}
          aria-label={`${counts[k]} ${copy.kind[k]}`}
        >
          {counts[k]}
        </span>
      ))}
    </span>
  )
}

/** A selectable list row: a small marker, a title and one quiet line. */
function Row({
  marker,
  title,
  detail,
  onClick,
  label,
}: {
  marker: ReactNode
  title: string
  detail?: ReactNode
  onClick(): void
  label?: string
}) {
  return (
    <Button
      variant="ghost"
      className="h-auto w-full items-start justify-start gap-2.5 px-2 py-1.5 text-left font-normal whitespace-normal"
      onClick={onClick}
      aria-label={label}
    >
      <span className="mt-0.5 shrink-0">{marker}</span>
      <span className="min-w-0 flex-1">
        <span className="block text-sm leading-5 text-foreground">{title}</span>
        {detail ? (
          <span className="block truncate text-xs text-muted-foreground">{detail}</span>
        ) : null}
      </span>
    </Button>
  )
}

function ProblemRow({
  p,
  v,
  select,
  me,
}: {
  p: Problem
  v: EstateView
  select(s: Selection): void
  me: string
}) {
  const owner = p.owner_id ? v.estate.people.find((x) => x.id === p.owner_id) : undefined
  return (
    <Row
      marker={
        <span
          className={cn(
            'grid size-4 place-items-center rounded-full text-3xs font-bold text-card',
            STRONG_BG[p.kind],
          )}
          aria-hidden
        >
          {p.tier === 'broken' ? '!' : '↗'}
        </span>
      }
      title={p.title}
      detail={`${copy.tier[p.tier]} · ${owner ? copy.insight.onIt(owner.name, owner.id === me) : copy.insight.nobody}`}
      onClick={() => select({ type: 'problem', id: p.id })}
    />
  )
}

function OppRow({ o, select }: { o: Opportunity; select(s: Selection): void }) {
  return (
    <Row
      marker={<Coins aria-hidden className="size-4 text-muted-foreground" />}
      title={o.title}
      detail={copy.insight.estMonth(fmtMoney(o.saving_usd))}
      onClick={() => select({ type: 'opp', id: o.id })}
    />
  )
}

function visibleOpps(v: EstateView) {
  return v.estate.opportunities.filter(
    (o) => v.view.scope === 'org' || o.entities.some((x) => v.mine.has(x)),
  )
}

function DefaultView(props: PanelProps) {
  const { v, needs, select, actions, filtered, clearFilters, now } = props
  const org = v.view.scope === 'org'
  const reqRows = needs.needs.rows.filter(
    (r) => r.kind === 'request' || r.kind === 'outside' || r.kind === 'budget',
  )
  // Failed recent sessions are Needs you's own check, not a problem the estate reports: they sit under the problems.
  const other = needs.needs.rows.filter((r) => r.kind === 'sessions')
  const counts: Partial<Record<ProblemKind, number>> = {}
  for (const p of v.problems) counts[p.kind] = (counts[p.kind] ?? 0) + 1
  const opps = visibleOpps(v)
  const day = fmtShortDay(v.estate.days[v.view.day] ?? '')
  return (
    <>
      <Head {...props} title={copy.insight.needsAction} icon={Hand} />
      <div className="px-2">
        {!v.live ? (
          <p className="px-2 py-1 text-xs text-muted-foreground">{copy.insight.requestsToday}</p>
        ) : reqRows.length ? (
          <ul aria-label={copy.insight.needsAction} className="divide-y divide-border/60 px-2">
            {reqRows.slice(0, 5).map((r) => (
              <li key={r.key}>
                <NeedRowView row={r} now={now} userId={props.actions.me} />
              </li>
            ))}
          </ul>
        ) : needs.needs.loading ? (
          <p className="px-2 py-1 text-xs text-muted-foreground">{overviewCopy.loading}…</p>
        ) : (
          <p className="px-2 py-1 text-xs text-muted-foreground">{copy.insight.noRequests}</p>
        )}
        {reqRows.length > 5 ? (
          <Link
            to="/chat"
            {...asButton({ variant: 'link', size: 'sm', className: 'h-auto px-2 pt-1 text-xs' })}
          >
            {copy.insight.viewAll(reqRows.length)}
          </Link>
        ) : null}
      </div>
      <Section
        icon={TriangleAlert}
        title={`${org ? copy.insight.fallingBehind : copy.insight.yourFallingBehind}${v.live ? '' : ` ${copy.insight.on(day)}`}`}
        aside={<KindBadges counts={counts} />}
      >
        {v.problems.length ? (
          <div className="-mx-2">
            {v.problems.slice(0, VISIBLE).map((p) => (
              <ProblemRow key={p.id} p={p} v={v} select={select} me={actions.me} />
            ))}
          </div>
        ) : filtered ? (
          <p className="py-1 text-xs text-muted-foreground">
            {copy.insight.nothingMatches}{' '}
            <Button variant="link" size="sm" className="h-auto px-0 text-xs" onClick={clearFilters}>
              {copy.insight.clearFilters}
            </Button>
          </p>
        ) : (
          <p className="py-1 text-xs text-muted-foreground">{copy.insight.nothingBehind}</p>
        )}
        {v.problems.length > VISIBLE ? (
          <p className="pt-1 text-xs text-muted-foreground">
            {copy.insight.moreBehind(v.problems.length - VISIBLE)}
          </p>
        ) : null}
        {other.map((r) => (
          <div key={r.key} className="-mx-1">
            <NeedRowView row={r} now={now} userId={props.actions.me} />
          </div>
        ))}
      </Section>
      {opps.length ? (
        <Section
          icon={Coins}
          title={org ? copy.insight.opportunities : copy.insight.yourOpportunities}
          aside={
            <span className="grid h-4 min-w-4 place-items-center rounded-full bg-muted-foreground px-1 text-3xs font-semibold text-card">
              {opps.length}
            </span>
          }
        >
          <div className="-mx-2">
            {opps.map((o) => (
              <OppRow key={o.id} o={o} select={select} />
            ))}
          </div>
        </Section>
      ) : null}
      {v.fallback ? (
        <p className="mx-4 mt-4 text-xs text-muted-foreground">{copy.fallback.note}</p>
      ) : null}
    </>
  )
}

function Chips({ ids, v, select }: { ids: readonly string[] } & Pick<PanelProps, 'v' | 'select'>) {
  return (
    <div className="flex flex-wrap gap-1.5">
      {ids
        .filter((x) => v.nodes.has(x))
        .map((x) => {
          const s = v.state.get(x) ?? 'ok'
          return (
            <Button
              key={x}
              variant="outline"
              size="sm"
              className="h-6 gap-1.5 px-2 text-xs font-normal"
              onClick={() => select({ type: 'node', id: x })}
            >
              <span
                aria-hidden
                className={cn('size-2 rounded-xs', SWATCH[isProblemState(s) ? s : 'ok'])}
              />
              {v.nodes.get(x)?.name}
            </Button>
          )
        })}
    </div>
  )
}

function KV({ rows }: { rows: [string, ReactNode][] }) {
  return (
    <dl className="grid grid-cols-2 gap-px overflow-hidden rounded-lg border bg-border">
      {rows.map(([k, val]) => (
        <div key={k} className="bg-card px-2.5 py-1.5">
          <dt className="text-2xs text-muted-foreground">{k}</dt>
          <dd className="text-sm font-medium tabular-nums">{val}</dd>
        </div>
      ))}
    </dl>
  )
}

function Note({ children, tone = 'ok' }: { children: ReactNode; tone?: 'ok' | 'muted' }) {
  return (
    <p
      className={cn(
        'mx-4 mt-2 rounded-md px-3 py-2 text-xs',
        tone === 'ok' ? 'bg-success/10' : 'bg-muted',
      )}
    >
      {children}
    </p>
  )
}

function Cards({ problems, ...props }: PanelProps & { problems: Problem[] }) {
  return (
    <div className="flex flex-col gap-2 px-4 pt-1">
      {problems.map((p) => (
        <ProblemCard
          key={p.id}
          problem={p}
          v={props.v}
          actions={props.actions}
          compact={problems.length > 2}
        />
      ))}
    </div>
  )
}

function NodeView(props: PanelProps & { id: string }) {
  const { v, id, select } = props
  const e = v.estate
  const n = v.nodes.get(id)
  if (!n) return <DefaultView {...props} />
  const ps = problemsOf(v.nodes, v.problems, id)
  const reqs = v.inputs.get(id) ?? []
  const opps = visibleOpps(v).filter((o) => o.entities.includes(id))
  const owner = n.owner ? v.nodes.get(n.owner)?.name : undefined
  const kindName = copy.nodeKind[n.kind]
  const home = homeOf(e, n)
  const group = n.kind === 'group' ? e.groups.find((g) => g.id === id) : undefined
  const wf = n.kind === 'workflow' ? e.workflows.find((w) => w.id === id) : undefined
  const related = [...relatedIds(props, id)].filter((x) => x !== id).slice(0, 14)
  return (
    <>
      <Head
        {...props}
        title={n.name}
        sub={owner ? copy.insight.owned(kindName, owner) : kindName}
      />
      {reqs.length ? (
        <div className="px-2">
          {reqs.map((r) => (
            <Link
              key={r.id}
              to="/chat/$sessionId"
              params={{ sessionId: r.sessionId }}
              state={{ waitingRequest: r.id } as never}
              {...asButton({
                variant: 'ghost',
                className:
                  'h-auto w-full items-start justify-start gap-2.5 px-2 py-1.5 text-left font-normal whitespace-normal',
              })}
            >
              <Hand aria-hidden className="mt-0.5 size-4 shrink-0 text-info" />
              <span className="min-w-0 flex-1">
                <span className="block text-sm leading-5">
                  {overviewCopy.needs.request(r.kind as HitlKind, 1)}
                </span>
                {r.title ? (
                  <span className="block truncate text-xs text-muted-foreground">{r.title}</span>
                ) : null}
              </span>
            </Link>
          ))}
        </div>
      ) : null}
      {ps.length ? (
        <Cards {...props} problems={ps} />
      ) : (
        <Note>
          {v.live
            ? copy.insight.healthyHere
            : copy.insight.healthyOn(fmtShortDay(e.days[v.view.day] ?? ''))}
        </Note>
      )}
      {opps.length ? (
        <div className="mt-2 px-2">
          {opps.map((o) => (
            <OppRow key={o.id} o={o} select={select} />
          ))}
        </div>
      ) : null}
      {group?.budget ? (
        <Section title={copy.insight.budget}>
          <KV
            rows={[
              [
                copy.insight.monthBudget,
                `${fmtMoney(group.budget.limit_usd)}, ${group.budget.hard ? copy.insight.hard : copy.insight.soft}`,
              ],
              [copy.insight.spentSoFar, fmtMoney(group.budget.spent_usd)],
              [
                copy.insight.projected,
                <span
                  key="p"
                  className={cn((pace(group.budget) ?? 0) > 1 && 'text-cost')}
                >{`${fmtMoney(group.budget.projected_usd)}, ${Math.round((pace(group.budget) ?? 0) * 100)}%`}</span>,
              ],
              [copy.insight.harnessShare, fmtMoney(group.budget.harness_usd)],
            ]}
          />
        </Section>
      ) : null}
      {group ? (
        <Section title={copy.insight.members}>
          <Chips ids={e.people.filter((p) => p.group_id === id).map((p) => p.id)} {...props} />
        </Section>
      ) : null}
      {wf ? (
        <Section title={copy.insight.steps}>
          <ol className="flex flex-col gap-1.5">
            {wf.steps.map((st, k) => {
              const s = st.ref ? (v.state.get(st.ref) ?? 'ok') : 'ok'
              const waiting = st.kind === 'approval' && reqs.length > 0
              return (
                <li
                  key={`${st.kind}:${st.ref ?? st.label}`}
                  className="flex items-center gap-2 text-sm"
                >
                  <span
                    className={cn(
                      'grid size-5 place-items-center rounded-full text-3xs font-semibold text-card',
                      waiting
                        ? 'bg-info'
                        : isProblemState(s)
                          ? STRONG_BG[s]
                          : 'bg-muted-foreground/60',
                    )}
                  >
                    {k + 1}
                  </span>
                  <span className="min-w-0 flex-1 truncate">{st.label}</span>
                  <span className="text-xs text-muted-foreground">
                    {waiting
                      ? copy.insight.stepState.waiting
                      : isProblemState(s)
                        ? copy.kind[s]
                        : copy.insight.stepState.ok}
                  </span>
                </li>
              )
            })}
          </ol>
        </Section>
      ) : null}
      <Glance {...props} id={id} />
      {related.length ? (
        <Section title={copy.insight.connected}>
          <Chips ids={related} {...props} />
        </Section>
      ) : null}
      {home ? (
        <div className="px-4 pt-3">
          <TargetLink target={home.target} estate={e} variant="outline">
            {copy.insight.openIn(home.page)}
          </TargetLink>
        </div>
      ) : null}
    </>
  )
}

function relatedIds({ v }: PanelProps, id: string) {
  const s = new Set<string>()
  const e = v.estate
  const a = e.agents.find((x) => x.id === id)
  const h = e.harnesses.find((x) => x.id === id)
  for (const x of [
    a?.owner_id,
    a?.router_id,
    ...(a?.model_ids ?? []),
    ...(a?.tool_ids ?? []),
    h?.owner_id,
    ...(h?.model_ids ?? []),
    ...(h?.tool_ids ?? []),
  ])
    if (x) s.add(x)
  for (const ag of e.agents)
    if (
      ag.model_ids.includes(id) ||
      ag.tool_ids.includes(id) ||
      ag.router_id === id ||
      ag.owner_id === id
    )
      s.add(ag.id)
  for (const hh of e.harnesses)
    if (hh.model_ids.includes(id) || hh.tool_ids.includes(id) || hh.owner_id === id) s.add(hh.id)
  for (const w of e.workflows) if (w.steps.some((st) => st.ref === id)) s.add(w.id)
  return s
}

function Glance({ v, id }: PanelProps & { id: string }) {
  const e = v.estate
  const a = e.agents.find((x) => x.id === id)
  const h = e.harnesses.find((x) => x.id === id)
  const md = e.models.find((x) => x.id === id)
  const p = e.people.find((x) => x.id === id)
  const w = e.workflows.find((x) => x.id === id)
  const r = e.routers.find((x) => x.id === id)
  const rows: [string, ReactNode][] = a
    ? [
        [copy.glance.spend, fmtMoney(a.spend_usd)],
        [copy.glance.runs, fmtInt(a.runs)],
        [copy.glance.success, a.success_pct == null ? '—' : `${a.success_pct}%`],
        [copy.glance.p95, a.p95_ms == null ? '—' : fmtLatency(a.p95_ms)],
      ]
    : h
      ? [
          [copy.glance.spend, h.spend_usd == null ? 'Unpriced' : fmtMoney(h.spend_usd)],
          [copy.glance.sessions, fmtInt(h.sessions.length)],
          [copy.glance.failed, fmtInt(h.sessions.filter((x) => x === 'failed').length)],
          [
            copy.glance.longOrCostly,
            fmtInt(h.sessions.filter((x) => x === 'slow' || x === 'cost').length),
          ],
        ]
      : md
        ? [
            [copy.glance.spend, fmtMoney(md.spend_usd)],
            [copy.glance.calls, fmtInt(md.calls)],
            [copy.glance.p95, md.p95_ms == null ? '—' : fmtLatency(md.p95_ms)],
            [copy.glance.provider, e.providers.find((x) => x.id === md.provider_id)?.name ?? '—'],
          ]
        : p && p.sessions
          ? [
              [copy.glance.spend, fmtMoney(p.spend_usd)],
              [copy.glance.sessions, fmtInt(p.sessions)],
              [copy.glance.success, p.success_pct == null ? '—' : `${p.success_pct}%`],
              [copy.glance.segment, copy.segments[p.segment][0]],
            ]
          : w
            ? [
                [copy.glance.runs, fmtInt(w.runs)],
                [copy.glance.steps, fmtInt(w.steps.length)],
              ]
            : r
              ? [
                  [
                    copy.glance.agents,
                    fmtInt(
                      e.agents.filter((x) => x.router_id === r.id).length ||
                        (r.direct ? e.harnesses.length : 0),
                    ),
                  ],
                  [copy.glance.setup, r.note ?? '—'],
                ]
              : []
  if (!rows.length) return null
  return (
    <Section title={copy.insight.atAGlance}>
      <KV rows={rows} />
    </Section>
  )
}

function FlowView(props: PanelProps & { id: string }) {
  const { v, id, select } = props
  const f = v.estate.flows.find((x) => x.id === id)
  if (!f) return <DefaultView {...props} />
  const ps = v.problems.filter((p) => p.flow_id === id)
  const chains = [...new Set(ps.map((p) => p.chain_id).filter((c): c is string => !!c))]
  const badges = flowBadges(v.problems, id)
  const status = ps.some((p) => p.tier === 'broken') ? 'broken' : ps.length ? 'degraded' : 'healthy'
  return (
    <>
      <Head {...props} title={copy.flows[id] ?? id} sub={copy.insight.flowSub} />
      <div className="flex flex-wrap gap-1.5 px-4">
        <span
          className={cn(
            'rounded-md border px-2 py-0.5 text-xs',
            status === 'broken'
              ? 'border-destructive/40 text-destructive'
              : status === 'degraded'
                ? 'border-warning/40 text-warning'
                : 'border-success/40 text-success',
          )}
        >
          {copy.insight.flowStatus[status]}
        </span>
        {f.direct ? (
          <span className="rounded-md border px-2 py-0.5 text-xs text-muted-foreground">
            {copy.insight.direct}
          </span>
        ) : null}
      </div>
      {f.unit ? (
        <div className="px-4 pt-2.5">
          <KV
            rows={[
              [copy.insight.volume, `${fmtInt(f.volume)} ${f.unit}`],
              [copy.insight.perDay, fmtInt(Math.round(f.volume / 30))],
              [
                copy.insight.failed,
                <span key="f" className={cn(badges.errors && 'text-destructive')}>
                  {fmtInt(badges.errors ?? 0)}
                </span>,
              ],
              [copy.insight.spendOnFlow, f.spend_usd == null ? '—' : fmtMoney(f.spend_usd)],
            ]}
          />
        </div>
      ) : null}
      {chains.length ? (
        <Section title={copy.insight.chainsThrough}>
          <div className="-mx-2">
            {chains.map((c, j) => {
              const ch = v.estate.chains.find((x) => x.id === c)
              if (!ch) return null
              return (
                <Row
                  key={c}
                  marker={
                    <span className="grid size-4 place-items-center rounded-full bg-foreground text-3xs font-semibold text-background">
                      {j + 1}
                    </span>
                  }
                  title={ch.title}
                  detail={copy.insight.startsAt(v.nodes.get(ch.steps[0]?.entity ?? '')?.name ?? '')}
                  onClick={() => select({ type: 'chain', id: c })}
                />
              )
            })}
          </div>
        </Section>
      ) : null}
      {ps.length ? (
        <Section title={copy.insight.problemsOnFlow}>
          <div className="-mx-4">
            <Cards {...props} problems={ps} />
          </div>
        </Section>
      ) : (
        <Note>{copy.insight.flowHealthy}</Note>
      )}
    </>
  )
}

function ChainView(props: PanelProps & { id: string }) {
  const { v, id, select } = props
  const ch = v.estate.chains.find((x) => x.id === id)
  if (!ch) return <DefaultView {...props} />
  return (
    <>
      <Head
        {...props}
        title={ch.title}
        sub={`${copy.insight.chain}. ${copy.insight.chainConfidence[ch.confidence]}`}
        icon={Link2}
      />
      <ol className="relative mx-4 mt-1 flex flex-col gap-0.5">
        {ch.steps.map((st, i) => {
          const s = v.state.get(st.entity) ?? 'ok'
          return (
            <li key={`${st.entity}:${st.what}`} className="relative">
              {i < ch.steps.length - 1 ? (
                <span
                  aria-hidden
                  className="absolute top-8 bottom-[-6px] left-[11px] w-0.5 bg-border"
                />
              ) : null}
              <Button
                variant="ghost"
                className="h-auto w-full items-start justify-start gap-3 px-0 py-1.5 text-left font-normal whitespace-normal hover:bg-transparent"
                onClick={() => select({ type: 'node', id: st.entity })}
              >
                <span
                  className={cn(
                    'grid size-6 shrink-0 place-items-center rounded-full text-2xs font-semibold text-card',
                    i === 0
                      ? 'bg-foreground'
                      : isProblemState(s)
                        ? STRONG_BG[s]
                        : 'bg-muted-foreground',
                  )}
                >
                  {i + 1}
                </span>
                <span className="min-w-0">
                  <span className="block text-sm font-medium">{st.what}</span>
                  <span className="block text-xs text-muted-foreground">
                    {v.nodes.get(st.entity)?.name}, {st.detail}
                  </span>
                </span>
              </Button>
            </li>
          )
        })}
      </ol>
      <p className="mx-4 mt-2 rounded-md bg-muted px-3 py-2 text-xs">
        <b className="font-semibold">{copy.insight.fixRoot}:</b> {ch.fix}
      </p>
      <div className="flex flex-wrap gap-2 px-4 pt-3">
        <TargetLink target={ch.action.target} estate={v.estate}>
          {ch.action.label}
        </TargetLink>
        <Button variant="outline" size="sm" onClick={props.back ?? props.close}>
          {copy.insight.hideChain}
        </Button>
      </div>
    </>
  )
}

function OppView(props: PanelProps & { id: string }) {
  const { v, id } = props
  const o = v.estate.opportunities.find((x) => x.id === id)
  if (!o) return <DefaultView {...props} />
  return (
    <>
      <Head {...props} title={o.title} sub={copy.insight.opportunity} icon={Coins} />
      <div className="mx-4 rounded-md bg-success/10 px-3 py-2 text-sm">
        <b className="font-semibold">{copy.insight.estMonth(fmtMoney(o.saving_usd))}</b>
        <p className="mt-0.5 text-xs text-muted-foreground">
          {copy.insight.method}: {o.method}
        </p>
      </div>
      <p className="mx-4 mt-2 text-sm">{o.detail}</p>
      <div className="px-4 pt-2.5">
        <KV
          rows={[
            [copy.insight.effort, o.effort],
            [copy.insight.appliesTo, copy.insight.items(o.entities.length)],
          ]}
        />
      </div>
      <div className="px-4 pt-2.5">
        <Chips ids={o.entities} {...props} />
      </div>
      <div className="px-4 pt-3">
        <TargetLink target={o.action.target} estate={v.estate}>
          {o.action.label}
        </TargetLink>
      </div>
    </>
  )
}

function CardView(props: PanelProps & { id: PanelId; kind: ProblemKind | null }) {
  const { v, id, kind, select } = props
  const ids = [...v.nodes.values()].filter((n) => n.panel === id).map((n) => n.id)
  let ps = v.problems.filter((p) => p.entities.some((x) => ids.includes(x)))
  if (kind) ps = ps.filter((p) => p.kind === kind || p.also === kind)
  const counts: Partial<Record<ProblemKind, number>> = {}
  for (const n of ids) {
    const s = v.state.get(n)
    if (s && isProblemState(s)) counts[s] = (counts[s] ?? 0) + 1
  }
  const bad = ids.filter((n) => isProblemState(v.state.get(n) ?? 'ok')).length
  const name = id === 'people' && v.estate.groups.length ? copy.panel.peopleTeams : copy.panel[id]
  return (
    <>
      <Head
        {...props}
        title={name}
        sub={`${copy.insight.panelSub(ids.length, bad)}${kind ? `. ${copy.insight.onlyKind(copy.kind[kind])}` : ''}`}
      />
      <div className="flex flex-wrap gap-1.5 px-4">
        {KIND_ORDER.filter((k) => counts[k]).map((k) => (
          <Button
            key={k}
            variant={kind === k ? 'secondary' : 'outline'}
            size="sm"
            className="h-6 gap-1.5 px-2 text-xs font-normal"
            aria-pressed={kind === k}
            onClick={() => select({ type: 'panel', id, kind: kind === k ? null : k })}
          >
            <span aria-hidden className={cn('size-2 rounded-xs', STRONG_BG[k])} />
            {counts[k]} {copy.kind[k]}
          </Button>
        ))}
      </div>
      {ps.length ? (
        <div className="pt-2">
          <Cards {...props} problems={ps} />
        </div>
      ) : (
        <Note>{copy.insight.healthyHere}</Note>
      )}
      <Section title={copy.insight.everything}>
        <Chips ids={ids.slice(0, 40)} {...props} />
      </Section>
    </>
  )
}

function TileView(props: PanelProps & { id: string; day: number }) {
  const { v, id, day, select } = props
  const a = v.estate.agents.find((x) => x.id === id)
  const date = v.estate.days[day] ?? ''
  const ps = v.estate.problems
    .filter((p) => p.entities.includes(id) && (v.view.scope === 'org' || p.mine) && !p.handled_at)
    .filter(
      (p) => (v.estate.days.indexOf(p.since) >= 0 ? v.estate.days.indexOf(p.since) : 0) <= day,
    )
  // The estate has no per-day figures: the panel shows the agent's period figures, says so, and links that day's
  // sessions rather than guessing a day's runs or spend.
  return (
    <>
      <Head
        {...props}
        title={copy.map.tile(a?.name ?? '', fmtShortDay(date))}
        sub={copy.insight.oneDay}
      />
      {a ? (
        <div className="px-4">
          <KV
            rows={[
              [copy.glance.runsPerDay, fmtInt(Math.round(a.runs / 30))],
              [copy.glance.success, a.success_pct == null ? '—' : `${a.success_pct}%`],
              [copy.glance.spend, fmtMoney(a.spend_usd)],
              [copy.glance.p95, a.p95_ms == null ? '—' : fmtLatency(a.p95_ms)],
            ]}
          />
          <p className="mt-1.5 text-2xs text-muted-foreground">{copy.glance.periodNote}</p>
        </div>
      ) : null}
      {ps.length ? (
        <Section title={copy.insight.problemsThatDay(ps.length)}>
          <div className="-mx-4">
            <Cards {...props} problems={ps} />
          </div>
        </Section>
      ) : (
        <Note>{copy.insight.healthyOn(fmtShortDay(date))}</Note>
      )}
      <div className="flex flex-wrap gap-2 px-4 pt-3">
        <Button variant="outline" size="sm" onClick={() => select({ type: 'node', id })}>
          {copy.insight.seeAll(a?.name ?? '')}
        </Button>
        <TargetLink
          target={{ page: 'sessions', query: { day: date } }}
          estate={v.estate}
          variant="ghost"
        >
          {copy.insight.sessionsThatDay}
        </TargetLink>
      </div>
    </>
  )
}

function SessionView(props: PanelProps & { id: string; index: number }) {
  const { v, id, index } = props
  const h = v.estate.harnesses.find((x) => x.id === id)
  if (!h) return <DefaultView {...props} />
  const o = h.sessions[index] ?? 'ok'
  const ps = v.problems.filter(
    (p) =>
      p.entities.includes(id) &&
      (o === 'ok' ? false : p.kind === o || p.also === o || o === 'failed'),
  )
  return (
    <>
      <Head {...props} title={copy.insight.sessionOf(index + 1, h.sessions.length)} sub={h.name} />
      <div className="px-4">
        <KV
          rows={[
            [
              copy.glance.outcome,
              <span key="o" className={cn(o === 'failed' && 'text-destructive')}>
                {copy.insight.sessionOutcome[o]}
              </span>,
            ],
          ]}
        />
      </div>
      {ps.length ? (
        <div className="pt-2">
          <Cards {...props} problems={ps} />
        </div>
      ) : (
        <Note>{copy.insight.nothingUnusual}</Note>
      )}
      <div className="px-4 pt-3">
        <TargetLink target={{ page: 'harnesses' }} estate={v.estate} variant="outline">
          {copy.insight.openHarness}
        </TargetLink>
      </div>
    </>
  )
}
