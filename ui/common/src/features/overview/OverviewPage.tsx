/**
 * The Overview homepage (plans/feat-overview.md): what needs you, where the money goes, and how healthy the fleet is,
 * each card linking to the page that owns it. The header's range (`?range=`, 7/30/90 days) drives the KPI row, Spend and
 * Harnesses; the other cards keep the windows their rules are defined on.
 *
 * "Now" is frozen per visit; coming back to the page 1 min+ later moves it (useReturnTick, the TokenOps rule), so the
 * windows and the cards refresh together (design review 8A). Pending requests aren't refetched here: they already
 * refetch on focus (eng review correction).
 * The grids follow the page area's width, not the viewport, because the sidebar changes it (design review 1A). The summary
 * card holds the headline and the KPI tiles: side by side from 1100 px of content (tiles 2×2), stacked below (tiles 2
 * across from 560 px). Below it, from 1100 px, four columns: Spend then Recent
 * sessions on the left three, Needs you, Budgets and Quick actions down the right, then Recent chats beside Fleet health;
 * 2 columns from 700 px, 1 below. DOM order stays the priority order (Needs you first); wide layouts place cards
 * explicitly, so nothing leaves a hole with or without budgets.
 */
import { useQuery, useQueryClient } from '@tanstack/react-query'
import { Link } from '@tanstack/react-router'
import { ToggleGroup, ToggleGroupItem } from '@/components/ui/toggle-group'
import { RotateCw } from 'lucide-react'
import { useEffect, useMemo, useState, type ReactNode } from 'react'
import { PageHeader } from '@/components/shared/page-header'
import { PageLoader } from '@/components/shared/page-loader'
import { StateCard } from '@/components/shared/state-card'
import { Badge } from '@/components/ui/badge'
import { Button } from '@/components/ui/button'
import { Card } from '@/components/ui/card'
import { useChatSessions } from '@/features/chat/api'
import { overviewNarrative } from '@/features/narrative/overview'
import { meQuery, type Me } from '@/lib/api/auth'
import { useReturnTick } from '@/lib/useReturnTick'
// import { fmtMonthYear } from '@/lib/format' // Budgets hidden: used only by the Budgets card.
// import { useBudgetCard, useFleetHealth, useHarnessSummary, useNeedsYou, useSpend } from './api'
import { useFleetHealth, useHarnessSummary, useNeedsYou, useOverviewSavings, useSpend } from './api'
// Budgets hidden: no server support for /api/budgets yet (R-L10). Restore when it lands.
// import { Budget } from './components/Budget'
import { FirstRun, FirstRunLead, FirstRunPreview } from './components/FirstRun'
import { FleetHealth } from './components/FleetHealth'
import { Harnesses, HarnessesLine } from './components/Harnesses'
import { Headline } from './components/Headline'
import { AgentsTile, RunsTile, SpendTile } from './components/Kpis'
import { Savings } from './components/Savings'
import { MonthBar } from './components/MonthBar'
import { NeedsYou } from './components/NeedsYou'
import { QuickActions } from './components/QuickActions'
import { RecentChats } from './components/RecentChats'
import { RecentSessions } from './components/RecentSessions'
import { SetupGuide } from './components/SetupGuide'
import { Spend } from './components/Spend'
import { copy } from './copy'
import { DEFAULT_RANGE, RANGE_DAYS, RANGES, type OverviewSearch } from './search'
import { RETURN_REFRESH_MS } from './tuning'

type SetSearch = (patch: Partial<OverviewSearch>) => void

function Frame({
  description,
  actions,
  children,
}: {
  description?: ReactNode
  actions?: ReactNode
  children: ReactNode
}) {
  return (
    <div className="@container/overview mx-auto flex w-full max-w-page flex-col gap-4">
      <PageHeader title={copy.title} description={description} actions={actions} />
      {children}
    </div>
  )
}

function HeaderActions({
  search,
  setSearch,
  setup,
}: {
  search: OverviewSearch
  setSearch: SetSearch
  /** False on first run: the Setup guide (or deploy) card is on the page already. */
  setup: boolean
}) {
  return (
    <>
      <ToggleGroup
        type="single"
        variant="outline"
        size="sm"
        value={search.range ?? DEFAULT_RANGE}
        onValueChange={(v) =>
          v &&
          setSearch({ range: v === DEFAULT_RANGE ? undefined : (v as OverviewSearch['range']) })
        }
        aria-label={copy.range.label}
      >
        {RANGES.map((r) => (
          <ToggleGroupItem
            key={r}
            value={r}
            aria-label={copy.range.item(RANGE_DAYS[r])}
            className="px-3 text-xs pointer-coarse:min-h-11"
          >
            {r}
          </ToggleGroupItem>
        ))}
      </ToggleGroup>
      {setup ? <SetupGuide /> : null}
    </>
  )
}

/** A first-run row that spans the grid at every width. */
const FULL_ROW = '@[700px]/overview:col-span-2 @[1100px]/overview:col-span-3'

/**
 * Where each card sits from 1100 px (4 columns) and from 700 px (2), with and without budgets. Literal class names, so
 * Tailwind sees them.
 */
const PLACE = {
  budgets: {
    needs:
      '@[700px]/overview:col-span-2 @[1100px]/overview:col-span-1 @[1100px]/overview:col-start-4 @[1100px]/overview:row-start-1',
    spend:
      '@[700px]/overview:col-span-2 @[1100px]/overview:col-span-3 @[1100px]/overview:col-start-1 @[1100px]/overview:row-start-1 @[1100px]/overview:row-span-2',
    budget: '@[1100px]/overview:col-start-4 @[1100px]/overview:row-start-2',
    health:
      '@[1100px]/overview:col-span-2 @[1100px]/overview:col-start-3 @[1100px]/overview:row-start-4',
    sessions:
      '@[700px]/overview:col-span-2 @[1100px]/overview:col-span-3 @[1100px]/overview:col-start-1 @[1100px]/overview:row-start-3',
    chats:
      '@[1100px]/overview:col-span-2 @[1100px]/overview:col-start-1 @[1100px]/overview:row-start-4',
    actions: '@[1100px]/overview:col-start-4 @[1100px]/overview:row-start-3',
  },
  // Quick actions takes the budget's place beside Spend, and Recent sessions spans the row.
  none: {
    needs:
      '@[700px]/overview:col-span-2 @[1100px]/overview:col-span-1 @[1100px]/overview:col-start-4 @[1100px]/overview:row-start-1',
    spend:
      '@[700px]/overview:col-span-2 @[1100px]/overview:col-span-3 @[1100px]/overview:col-start-1 @[1100px]/overview:row-start-1 @[1100px]/overview:row-span-2',
    budget: '',
    health:
      '@[700px]/overview:col-span-2 @[1100px]/overview:col-start-3 @[1100px]/overview:row-start-4',
    sessions:
      '@[700px]/overview:col-span-2 @[1100px]/overview:col-span-4 @[1100px]/overview:col-start-1 @[1100px]/overview:row-start-3',
    chats:
      '@[700px]/overview:col-span-2 @[1100px]/overview:col-start-1 @[1100px]/overview:row-start-4',
    // Full width at 2 columns, so it doesn't sit alone at half width on the last row.
    actions:
      '@[700px]/overview:col-span-2 @[1100px]/overview:col-span-1 @[1100px]/overview:col-start-4 @[1100px]/overview:row-start-2',
  },
} as const

export function OverviewPage({
  search,
  setSearch,
}: {
  search: OverviewSearch
  setSearch: SetSearch
}) {
  const meQ = useQuery(meQuery)
  const me = meQ.data
  // A non-401 failure (server down, 5xx) must not spin forever; /status explains how to start the server (/ship review).
  if (!me && meQ.isError) {
    return (
      <Frame>
        <div data-testid="overview-me-error">
          <StateCard
            tone="error"
            title={copy.meFailed}
            action={
              <div className="flex flex-wrap gap-2">
                <Button asChild size="sm" variant="outline">
                  <Link to="/status">{copy.checkStatus}</Link>
                </Button>
                <Button size="sm" variant="outline" onClick={() => void meQ.refetch()}>
                  <RotateCw className="size-3.5" aria-hidden /> {copy.retry}
                </Button>
              </div>
            }
          />
        </div>
      </Frame>
    )
  }
  if (!me)
    return (
      <Frame>
        <PageLoader label={copy.loadingPage} />
      </Frame>
    )
  return <Overview me={me} search={search} setSearch={setSearch} />
}

function Overview({
  me,
  search,
  setSearch,
}: {
  me: Me
  search: OverviewSearch
  setSearch: SetSearch
}) {
  const range = search.range ?? DEFAULT_RANGE
  const days = RANGE_DAYS[range]
  const qc = useQueryClient()
  const returnTick = useReturnTick(RETURN_REFRESH_MS)
  // eslint-disable-next-line react-hooks/exhaustive-deps
  const now = useMemo(() => new Date(), [returnTick])
  useEffect(() => {
    if (!returnTick) return
    // Window-keyed queries (finops, sessions) move with `now`; these keys don't, so refresh them.
    void qc.invalidateQueries({ queryKey: ['agents'], exact: true })
    void qc.invalidateQueries({ queryKey: ['agents', 'deployment'] })
    void qc.invalidateQueries({ queryKey: ['router', 'budgets'] })
    // The month calendar is keyed by month, not window: refresh it too, as TokenOps does (/ship review).
    void qc.invalidateQueries({ queryKey: ['tokenops', 'calendar'] })
  }, [returnTick, qc])

  const fleet = useFleetHealth(now)
  // A failed agent list must not freeze the other cards: they load, and Needs you says agent health failed (/ship review).
  const ready = !!fleet.error || (!!fleet.summary && fleet.agentCount > 0)
  // First run: the directory answered with no agents but harnesses (design review 7A).
  const firstRun = !!fleet.summary && fleet.agentCount === 0
  const spend = useSpend(now, range, ready)
  const savings = useOverviewSavings(now, range, ready)
  const needsYou = useNeedsYou(now, me, fleet, ready)
  const harnesses = useHarnessSummary(now, me, range)
  // The Recent chats card's own query (same key): first paint waits for it too.
  const chats = useChatSessions()
  // Budgets hidden: no server support for /api/budgets yet (R-L10). Restore when it lands.
  // const budget = useBudgetCard(fleet, ready)
  // const budgets = !budget.absent
  const budgets = false
  const { needs } = needsYou
  const narrative = useMemo(
    () =>
      overviewNarrative({
        month: spend.summary,
        unpriced: spend.unpriced,
        actionCount: needs.actionCount,
        waitingCount: needs.waitingCount,
        otherCount: needs.rows.filter((r) => r.kind === 'budget' || r.kind === 'sessions').length,
        empty: needs.empty,
      }),
    [spend.summary, spend.unpriced, needs],
  )

  const grid =
    'grid grid-cols-1 items-stretch gap-3 @[700px]/overview:grid-cols-2 @[1100px]/overview:grid-cols-3'
  const description = copy.greeting(now.getHours(), me.username, days)
  const actions = <HeaderActions search={search} setSearch={setSearch} setup={!firstRun} />
  // One page loader until the first paint's reads settle (data or error: a failed read renders its card's error), in
  // place of every card's skeleton. Latched: a later range change or return tick keeps the cards' own loading states.
  // Needs you's rating and session checks aren't waited for: its card shows them loading (a fan-out can be slow).
  const coldPending =
    (!fleet.summary && !fleet.error) ||
    harnesses.isPending ||
    (!firstRun &&
      (spend.isPending ||
        (!spend.totals && !spend.totalsError) ||
        needsYou.sessions.list.isPending ||
        chats.isPending))
  const [painted, setPainted] = useState(false)
  if (!painted && !coldPending) setPainted(true)
  if (!painted && coldPending) {
    return (
      <Frame description={description} actions={actions}>
        <PageLoader label={copy.loadingPage} />
      </Frame>
    )
  }
  if (firstRun) {
    return (
      <Frame description={description} actions={actions}>
        <FirstRunLead />
        <div className={grid}>
          {/* From 1100 px Quick actions and Harnesses' one line stack beside the first-run card, so neither stretches
              to its height; below that each spans the grid. The preview of later cards sits apart, after a wider gap. */}
          <FirstRun />
          <div className="contents @[1100px]/overview:flex @[1100px]/overview:flex-col @[1100px]/overview:gap-3">
            <QuickActions firstRun className={`${FULL_ROW} @[1100px]/overview:flex-1`} />
            <HarnessesLine data={harnesses} days={days} className={FULL_ROW} />
          </div>
          <FirstRunPreview className={`${FULL_ROW} mt-3`} />
        </div>
      </Frame>
    )
  }
  const place = budgets ? PLACE.budgets : PLACE.none
  return (
    <Frame description={description} actions={actions}>
      {/* The summary as one card like TokenOps' SummaryHero: the headline on the left, the KPI tiles 2×2 on the right. */}
      <Card
        asChild
        className="gap-0 overflow-hidden p-0 @[1100px]/overview:grid @[1100px]/overview:grid-cols-[minmax(0,1fr)_minmax(0,1.25fr)]"
      >
        <section aria-labelledby="overview-summary-title">
          <h2 id="overview-summary-title" className="sr-only">
            {copy.summary}
          </h2>
          <div className="flex flex-col justify-center gap-4 p-5">
            <Badge variant="outline" className="border-primary/30 bg-primary/10 text-primary-text">
              {copy.range.item(days)}
            </Badge>
            <Headline narrative={narrative} nothing={needs.empty} />
            <MonthBar spend={spend} />
            <Savings view={savings.view} isPending={savings.isPending} />
          </div>
          {/* The tiles drop their own card chrome here: the hairlines between them are the grid's gap. */}
          <div className="grid grid-cols-1 gap-px border-t border-border bg-border *:rounded-none *:border-0 @[560px]/overview:grid-cols-2 @[1100px]/overview:border-t-0 @[1100px]/overview:border-l">
            <SpendTile spend={spend} />
            <RunsTile spend={spend} />
            <AgentsTile fleet={fleet} />
            <Harnesses data={harnesses} days={days} />
          </div>
        </section>
      </Card>
      <div className="grid grid-cols-1 items-stretch gap-3 @[700px]/overview:grid-cols-2 @[1100px]/overview:grid-cols-4">
        <NeedsYou data={needsYou} now={now.getTime()} userId={me.sub} className={place.needs} />
        <Spend spend={spend} now={now.getTime()} className={place.spend} />
        {/* Budgets hidden: no server support for /api/budgets yet (R-L10). Restore when it lands.
        {budgets ? (
          <Budget data={budget} month={fmtMonthYear(now)} className={place.budget} />
        ) : null}
        */}
        <FleetHealth fleet={fleet} className={place.health} />
        <RecentSessions
          sessions={needsYou.sessions}
          displayName={fleet.displayName}
          now={now.getTime()}
          className={place.sessions}
        />
        <RecentChats
          byId={fleet.byId}
          username={me.username}
          now={now.getTime()}
          className={place.chats}
        />
        <QuickActions budgets={budgets} className={place.actions} />
      </div>
    </Frame>
  )
}
