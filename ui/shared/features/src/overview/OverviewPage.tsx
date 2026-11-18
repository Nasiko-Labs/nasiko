/**
 * The Overview homepage (plans/feat-overview.md): what needs you, where the money goes, and how healthy the fleet is,
 * each card linking to the page that owns it. The header's range (`?range=`, 7/30/90 days) drives the KPI row, Spend and
 * Harnesses; the other cards keep the windows their rules are defined on.
 *
 * "Now" is frozen per visit; coming back to the page 1 min+ later moves it (useReturnTick, the TokenOps rule), so the
 * windows and the cards refresh together (design review 8A). Pending requests aren't refetched here: they already
 * refetch on focus (eng review correction).
 * The grids follow the page area's width, not the viewport, because the sidebar changes it (design review 1A). The summary
 * is a bento: the headline in a lead card beside the KPI tiles (2×2) from 1100 px of content, stacked below (tiles 2
 * across from 560 px). Below it, from 1100 px, twelve columns: Needs you and Spend each span the row, then Fleet health
 * beside Recent sessions and Recent chats; 2 columns from 700 px, 1 below. DOM order stays the priority order (Needs you first); wide layouts place cards
 * explicitly, so nothing leaves a hole. A card below Needs you with nothing to show steps out (user 2026-11-06).
 *
 * Restored at `/` on 2026-10-17 in place of the agent-centric Home (its table, scope switch and chart band are gone),
 * with Home's Budget burn panel (the `budgets.useHomePanel` slot, EE only) spanning the row under Needs you. The panel
 * reads the persona's default scope (`homeScopeFor`: Fleet for leads and finance, Yours otherwise), since the Overview
 * has no scope switch; the first paint waits for it, so it never pushes the cards down mid-look.
 *
 * The header's view switch swaps the summary bento for the estate's canvas (plans/feat-village-overview.md, ported from
 * nasiko-ui-lab feat/product-shell); the cards under it stay either way. `?view=village` draws the village and
 * `?view=map` the Map's board (`fleet/`), with the same HUD and insight drawer. A bare `/` opens the viewer's last pick
 * (`homeViewKey`), else the summary; a `?sel=` without a view opens the village. The estate is read only while the
 * canvas shows. Admins pick Org or Mine (`?scope=`) beside the range on the canvas; the filters bar shows only while a
 * filter is set. The village is OSS only (the desktop app embeds the OSS build); other editions switch between the
 * summary and the Map, and anything that would open the village opens the Map there (NAS-1318).
 */
import { useEdition, useSlots } from '@nasiko/app-core/edition/edition-context'
import { useQuery, useQueryClient } from '@tanstack/react-query'
import { Link } from '@tanstack/react-router'
import { ToggleGroup, ToggleGroupItem } from '@nasiko/ui/primitives/toggle-group'
import { RotateCw } from 'lucide-react'
import { useCallback, useEffect, useMemo, useState, type ReactNode } from 'react'
import { toast } from 'sonner'
import { TourButton } from '@nasiko/app-core/shell/TopBar'
import { PageHeader } from '@nasiko/ui/composites/page-header'
import { PageLoader } from '@nasiko/ui/composites/page-loader'
import { Reveal, RevealItem } from '@nasiko/ui/composites/reveal'
import { StateCard } from '@nasiko/ui/composites/state-card'
import { Button, asButton } from '@nasiko/ui/primitives/button'
import { useChatSessions } from '@nasiko/features/chat/api'
import { overviewNarrative } from '@nasiko/features/narrative/overview'
import { usePersona } from '@nasiko/features/onboarding/api'
import { homeScopeFor } from '@nasiko/features/onboarding/logic'
import { meQuery, type Me } from '@nasiko/data-access/auth'
import { DRAFT_PREFIX } from '@nasiko/app-core/draftKeys'
import { isEndpointAbsent } from '@nasiko/data-access/detect'
import { useReturnTick } from '@nasiko/app-core/useReturnTick'
import {
  useFleetHealth,
  useHarnessSummary,
  useNeedsYou,
  useOverviewSavings,
  useSavedByAgent,
  useSetupSteps,
  useSpend,
} from './api'
import { FirstRunHero, FirstRunPreview } from './components/FirstRun'
import { FleetHealth } from './components/FleetHealth'
import { HarnessActivity, PreviewActivity } from './components/HarnessActivity'
import { PreviewHeader, PreviewProvider } from './components/HarnessPreview'
import { NeedsYou } from './components/NeedsYou'
import { usePausedFlows } from '@nasiko/features/flows/paused'
import { RecentChats } from './components/RecentChats'
import { RecentSessions } from './components/RecentSessions'
import { SavedByAgent } from './components/SavedByAgent'
import { SetupGuide } from './components/SetupGuide'
import { SetUpNow } from './components/SetUpNow'
import { SummaryCard } from './components/SummaryCard'
import { Spend } from './components/Spend'
import { TryAgentRun } from './components/TryAgentRun'
import { readDone, showTryRun } from './tryRun'
import { FilterBar } from './components/FilterBar'
import { copy } from './copy'
import { useEstate, useProblemAction } from './estate/api'
import { estateCopy } from './estate/copy'
import type { ProblemPatch } from './estate/types'
import { watchSummary } from './needs'
import { useEstateView } from './estate/useEstateView'
import { formatSel, parseSel, type Selection } from './map/selection'
import type { ProblemActions } from './panel/ProblemCard'
import { DEFAULT_RANGE, RANGE_DAYS, RANGES, type OverviewSearch } from './search'
import { RETURN_REFRESH_MS } from './tuning'
import type { VillageView } from './village/Hud'
import { VillageCanvas, VillageLoading } from './village'
import { loadVillage } from './village/load'

type SetSearch = (patch: Partial<OverviewSearch>) => void
type HomeView = 'overview' | VillageView
const HOME_VIEWS: readonly HomeView[] = ['overview', 'village', 'map']
/** Every edition but OSS: the Map without the village. */
const MAP_VIEWS: readonly HomeView[] = ['overview', 'map']

/** The view a bare `/` opens, per viewer, under the drafts prefix (sign-out clears it, as chat's rail view). */
const homeViewKey = (sub: string) => `${DRAFT_PREFIX}${sub}:meta:homeView`
function readHomeView(sub: string): HomeView {
  try {
    const v = globalThis.localStorage?.getItem(homeViewKey(sub))
    return HOME_VIEWS.find((x) => x === v) ?? 'overview'
  } catch {
    return 'overview'
  }
}
/** Written when the viewer picks a view; arriving with `?view=` alone doesn't. */
function rememberHomeView(sub: string, view: HomeView) {
  try {
    globalThis.localStorage?.setItem(homeViewKey(sub), view)
  } catch {
    // Blocked storage: a bare `/` opens the cards.
  }
}

function Frame({
  description,
  actions,
  children,
}: {
  description?: ReactNode
  actions?: ReactNode
  children: ReactNode
}) {
  // The coding harness preview plays on the page itself (HarnessPreview.tsx), its bar in the header's place.
  return (
    <PreviewProvider>
      <div className="@container/overview mx-auto flex w-full max-w-page flex-col gap-4">
        <PreviewHeader>
          <PageHeader title={copy.title} description={description} actions={actions} />
        </PreviewHeader>
        {children}
      </div>
    </PreviewProvider>
  )
}

function HeaderActions({
  search,
  setSearch,
  setup,
  admin,
  view,
  views,
  onView,
}: {
  search: OverviewSearch
  setSearch: SetSearch
  /** False on first run: the Setup guide (or deploy) card is on the page already. */
  setup: boolean
  /** Admins choose Org or Mine for the canvas; members only ever see their own. */
  admin: boolean
  view: HomeView
  /** The edition's views (the village is OSS only). */
  views: readonly HomeView[]
  onView(v: HomeView): void
}) {
  return (
    <>
      <ToggleGroup
        data-tour="overview-views"
        variant="outline"
        size="sm"
        value={[view]}
        onValueChange={(vs) => {
          const v = vs[0] as string
          return v && onView(v as HomeView)
        }}
        aria-label={copy.views.label}
      >
        {views.map((v) => (
          <ToggleGroupItem
            key={v}
            value={v}
            className="px-3 text-xs pointer-coarse:min-h-11"
            // Intent to open the canvas starts its download, so the switch feels instant.
            onPointerEnter={v === 'overview' ? undefined : () => void loadVillage()}
            onFocus={v === 'overview' ? undefined : () => void loadVillage()}
          >
            {copy.views[v]}
          </ToggleGroupItem>
        ))}
      </ToggleGroup>
      {admin && view !== 'overview' ? (
        <ToggleGroup
          variant="outline"
          size="sm"
          value={[search.scope ?? 'org']}
          onValueChange={(vs) => {
            const s = vs[0] as string
            return s && setSearch({ scope: s === 'org' ? undefined : 'mine', sel: undefined })
          }}
          aria-label={estateCopy.scope.label}
        >
          <ToggleGroupItem value="mine" className="px-3 text-xs pointer-coarse:min-h-11">
            {estateCopy.scope.mine}
          </ToggleGroupItem>
          <ToggleGroupItem value="org" className="px-3 text-xs pointer-coarse:min-h-11">
            {estateCopy.scope.org}
          </ToggleGroupItem>
        </ToggleGroup>
      ) : null}
      <ToggleGroup
        data-tour="overview-range"
        variant="outline"
        size="sm"
        value={[search.range ?? DEFAULT_RANGE]}
        onValueChange={(vs) => {
          const v = vs[0] as string
          return (
            v &&
            setSearch({ range: v === DEFAULT_RANGE ? undefined : (v as OverviewSearch['range']) })
          )
        }}
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
      {/* No top bar here on desktop (phones keep it, with its own button). */}
      <TourButton className="max-md:hidden" />
    </>
  )
}

/** Where each card sits: twelve columns from 1100 px, 2 from 700. Literal class names, so Tailwind sees them. */
// Redesign 2026-10-13: Needs you, Budget burn and Spend span the row; then Fleet health beside Recent sessions and
// Recent chats beside Quick actions, alternating widths. Heights match within a row, so no card stretches far past
// its content.
// Redesign 2026-10-19: Needs you (8) beside Fleet health (4), both "what's wrong"; Needs you at that width draws one
// row per item (its tiles start at 900 px of card). Budget burn and Spend span the row; then Recent sessions (7) beside
// Saved tokens by agent (5), and Recent chats (7) beside Quick actions (5): pairs of about the same height, so neither
// card in a row is left half empty.
// 2026-11-06: Coding harness activity spans the row right under Needs your attention and Fleet health.
// 2026-11-06 (user): a card with nothing to show steps out and Quick actions is gone (the sidebar, Agents and "Did you
// know?" cover it). Recent sessions and Saved tokens share a flex row, so either alone takes the whole row and the row
// itself collapses (`empty:hidden`) when both step out; Recent chats spans the row.
const FULL = '@[700px]/overview:col-span-2 @[1100px]/overview:col-span-12'
const PLACE = {
  tryRun: FULL,
  needs: '@[700px]/overview:col-span-2 @[1100px]/overview:col-span-8',
  health: '@[700px]/overview:col-span-2 @[1100px]/overview:col-span-4',
  burn: FULL,
  spend: FULL,
  pair: `${FULL} flex flex-col gap-3 empty:hidden @[1100px]/overview:flex-row`,
  sessions: '@[1100px]/overview:flex-7',
  saved: '@[1100px]/overview:flex-5',
  chats: FULL,
  harness: FULL,
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
                <Link to="/status" {...asButton({ variant: 'outline', size: 'sm' })}>
                  {copy.checkStatus}
                </Link>
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
  // A try-a-run finished: Home moves on and refreshes as on a return, so the run's spend and tokens show at once.
  const [ranTick, setRanTick] = useState(0)
  const refreshTick = returnTick + ranTick
  // eslint-disable-next-line react-hooks/exhaustive-deps
  const now = useMemo(() => new Date(), [refreshTick])
  useEffect(() => {
    if (!refreshTick) return
    // Window-keyed queries (finops, sessions) move with `now`; these keys don't, so refresh them.
    void qc.invalidateQueries({ queryKey: ['agents'], exact: true })
    void qc.invalidateQueries({ queryKey: ['agents', 'deployment'] })
    void qc.invalidateQueries({ queryKey: ['router', 'budgets'] })
    // The Budget burn panel's burn and At risk, and Needs you's budget rows (Home's rule, Codex 2).
    void qc.invalidateQueries({ queryKey: ['budgets', 'v1'] })
    // The month calendar is keyed by month, not window: refresh it too, as TokenOps does (/ship review).
    void qc.invalidateQueries({ queryKey: ['tokenops', 'calendar'] })
    void qc.invalidateQueries({ queryKey: ['overview', 'estate'] })
  }, [refreshTick, qc])
  // A try-a-run just finished: its spend, tokens and run are already metered, but the range-keyed cards (`7d`, `30d`)
  // keep their query keys when `now` moves, so refetch everything Home shows rather than wait for them to go stale.
  useEffect(() => {
    if (ranTick) void qc.invalidateQueries({ type: 'active' })
  }, [ranTick, qc])

  const fleet = useFleetHealth(now)
  // A failed agent list must not freeze the other cards: they load, and Needs you says agent health failed (/ship review).
  const ready = !!fleet.error || (!!fleet.summary && fleet.agentCount > 0)
  // First run: the directory answered with no agents but harnesses (design review 7A).
  const firstRun = !!fleet.summary && fleet.agentCount === 0
  const spend = useSpend(now, range, ready)
  const savings = useOverviewSavings(now, range, ready)
  const savedByAgent = useSavedByAgent(now, range, fleet, ready && !firstRun)
  const needsYou = useNeedsYou(now, me, fleet, ready)
  const setup = useSetupSteps(ready && !firstRun)
  const harnesses = useHarnessSummary(now, me, range)
  // The Recent chats card's own query (same key): first paint waits for it too.
  const chats = useChatSessions()
  // Budget burn (EE): the persona's default scope; null until the persona settles (it settles on a failure too).
  const persona = usePersona()
  const slots = useSlots()
  const burn = slots.budgets.useHomePanel({
    scope: persona.settled ? homeScopeFor(persona.persona, me.is_superuser) : null,
    meSub: me.sub,
    isSuperuser: me.is_superuser,
    unitLabel: null,
  })
  const { needs } = needsYou
  // Watch agents for an otherwise empty inbox (user 2026-11-05: Fleet health keeps them otherwise).
  const watch = fleet.summary ? watchSummary(fleet.summary.watch) : null
  // Read only when there are requests to link (plans/feat-flows.md O2, amended 2026-10-06).
  const pausedFlows = usePausedFlows(needs.rows.some((r) => r.kind === 'request'))
  const narrative = useMemo(
    () =>
      overviewNarrative({
        month: spend.summary,
        unpriced: spend.unpriced,
        actionCount: needs.actionCount,
        waitingCount: needs.waitingCount,
        otherCount: needs.rows.filter((r) => r.kind === 'budget' || r.kind === 'sessions').length,
        empty: needs.empty,
        setupCount: setup.left,
        watchCount: watch?.count,
      }),
    [spend.summary, spend.unpriced, needs, setup.left, watch?.count],
  )

  // `list` was the village's List view; its list is the Map's System card now, so an old link opens the Map. Without
  // the village (any edition but OSS), so does a village link or pick.
  const hasVillage = useEdition() === 'oss'
  const [remembered, setRemembered] = useState(() => readHomeView(me.sub))
  const asked =
    search.view === 'list' ? 'map' : (search.view ?? (search.sel ? 'village' : remembered))
  const view: HomeView = asked === 'village' && !hasVillage ? 'map' : asked
  const canvas = view !== 'overview'
  // The canvas's chunk downloads alongside the estate, not after it.
  useEffect(() => {
    if (canvas) void loadVillage()
  }, [canvas])
  const pickView = (v: HomeView) => {
    rememberHomeView(me.sub, v)
    setRemembered(v)
    // The summary is the bare `/`; leaving the canvas drops its selection with it.
    setSearch(v === 'overview' ? { view: undefined, sel: undefined } : { view: v })
  }

  // The canvas's estate: the server's (OV-1), or one built from the fleet when the endpoint is absent. Read only while
  // the canvas shows, so the summary never waits for it.
  const admin = me.is_superuser
  const estateQ = useEstate(range, !firstRun && canvas)
  const absent = estateQ.data === null || (estateQ.isError && isEndpointAbsent(estateQ.error))
  const v = useEstateView({
    estate: estateQ.data,
    absent,
    fleet,
    me,
    search,
    admin,
    waiting: needsYou.waiting,
    now: now.getTime(),
  })
  // Selections made in the panel, so Back walks them.
  const [trail, setTrail] = useState<string[]>([])
  const select = useCallback(
    (s: Selection) => {
      const next = formatSel(s)
      if (search.sel && search.sel !== next) setTrail((t) => [...t.slice(-20), search.sel ?? ''])
      setSearch({ sel: next })
    },
    [search.sel, setSearch],
  )
  const clear = useCallback(() => {
    setTrail([])
    setSearch({ sel: undefined })
  }, [setSearch])
  const back = trail.length
    ? () => {
        const prev = trail[trail.length - 1]
        setTrail((t) => t.slice(0, -1))
        setSearch({ sel: prev || undefined })
      }
    : null
  // OV-2: optimistic, with Undo in the toast.
  const act = useProblemAction()
  const problemActions: ProblemActions = {
    enabled: !!estateQ.data && !v?.fallback,
    me: me.sub,
    patch: (id, patch: ProblemPatch, undo, message) =>
      act.mutate(
        { id, patch },
        {
          onSuccess: () => {
            if (!message) return
            toast(
              message,
              undo
                ? {
                    action: {
                      label: estateCopy.card.toast.undo,
                      onClick: () => act.mutate({ id, patch: undo }),
                    },
                  }
                : undefined,
            )
          },
          onError: () => toast.error(estateCopy.card.toast.failed),
        },
      ),
    showChain: (id) => select({ type: 'chain', id }),
  }

  const description = copy.greeting(now.getHours(), me.username, days)
  const actions = (
    <HeaderActions
      search={search}
      setSearch={setSearch}
      setup={!firstRun}
      admin={admin}
      view={view}
      views={hasVillage ? HOME_VIEWS : MAP_VIEWS}
      onView={pickView}
    />
  )
  // One page loader until the first paint's reads settle (data or error: a failed read renders its card's error), in
  // place of every card's skeleton. Latched: a later range change or return tick keeps the cards' own loading states.
  // Needs you's rating and session checks aren't waited for: its card shows them loading (a fan-out can be slow).
  const coldPending =
    (!fleet.summary && !fleet.error) ||
    harnesses.isPending ||
    (!firstRun &&
      (spend.isPending ||
        (!spend.totals && !spend.totalsError) ||
        needsYou.sessions.recent.isPending ||
        setup.isPending ||
        // The lead card's foot is the savings band or a tip: waiting for it means the two never swap.
        savings.isPending ||
        !persona.settled ||
        burn.pending ||
        chats.isPending))
  const [painted, setPainted] = useState(false)
  // The try-a-run card, decided once at the first paint: it asks for a refresh after each run, so it must not vanish
  // when its own run lands mid-visit.
  const [tryRun, setTryRun] = useState(false)
  if (!painted && !coldPending) {
    setPainted(true)
    setTryRun(!firstRun && showTryRun(readDone(me.sub), spend.totals?.runs ?? null))
  }
  if (!painted && coldPending) {
    return (
      <Frame description={description} actions={actions}>
        <PageLoader label={copy.loadingPage} />
      </Frame>
    )
  }
  if (firstRun) {
    // A spending limit can be set before anything runs: the budgets layer's strip (EE; OSS has no budgets).
    const { FirstRunStrip } = slots.budgets
    // Nothing has run, so no range to pick and no window in the greeting; the tour button stays.
    return (
      <Frame
        description={copy.firstRun.greeting(now.getHours(), me.username)}
        actions={<TourButton className="max-md:hidden" />}
      >
        <Reveal className="flex flex-col gap-3">
          <RevealItem>
            <FirstRunHero />
          </RevealItem>
          {FirstRunStrip ? (
            <RevealItem>
              <FirstRunStrip />
            </RevealItem>
          ) : null}
          <RevealItem className="mt-3">
            <SetUpNow harnesses={harnesses} days={days} />
          </RevealItem>
          <RevealItem className="mt-5">
            <FirstRunPreview />
          </RevealItem>
        </Reveal>
      </Frame>
    )
  }
  const filtered = !!(search.team || search.owner || search.kind || search.tier)
  const clearFilters = () =>
    setSearch({ team: undefined, owner: undefined, kind: undefined, tier: undefined })
  /** The village's own selections (`place:…`) are raw `sel` values the map's parser doesn't know. */
  const selectRaw = (raw: string) => {
    const s = parseSel(raw)
    if (s) select(s)
    else setSearch({ sel: raw })
  }
  return (
    <Frame description={description} actions={actions}>
      {/* The summary, or in its place the canvas: the cards under it stay either way. */}
      {!canvas ? (
        <SummaryCard
          days={days}
          narrative={narrative}
          nothing={needs.empty}
          spend={spend}
          savings={savings}
          harnesses={harnesses}
        />
      ) : v ? (
        <div data-tour="overview-canvas" className="flex flex-col gap-3">
          {filtered ? (
            <FilterBar v={v} search={search} me={me.sub} onChange={(patch) => setSearch(patch)} />
          ) : null}
          <VillageCanvas
            v={v}
            sel={search.sel}
            onSel={(s) => (s ? selectRaw(s) : clear())}
            view={view === 'map' ? 'map' : 'village'}
            onView={pickView}
            panel={{
              select,
              back,
              close: clear,
              needs: needsYou,
              actions: problemActions,
              filtered,
              clearFilters,
              now: now.getTime(),
            }}
            saved={savings.view?.costSaved ?? null}
            optimisation={savings.view}
            spend={spend}
            now={now.getTime()}
          />
        </div>
      ) : estateQ.isError && !absent ? (
        <StateCard
          tone="error"
          title={estateCopy.error.title}
          action={
            <Button size="sm" variant="outline" onClick={() => void estateQ.refetch()}>
              <RotateCw className="size-3.5" aria-hidden /> {estateCopy.error.retry}
            </Button>
          }
        />
      ) : (
        <VillageLoading view={view === 'map' ? 'map' : 'village'} />
      )}
      <div className="grid grid-cols-1 items-stretch gap-3 @[700px]/overview:grid-cols-2 @[1100px]/overview:grid-cols-12">
        {/* The coding harness preview's sample card (HarnessPreview.tsx), right under the summary it plays in (user
            2026-11-06; the real card sits below the inbox), above the try-a-run card too (user 2026-11-13). Renders
            nothing outside a preview. */}
        {harnesses.ownOnly || harnesses.notVisible ? null : (
          <PreviewActivity days={days} className={PLACE.harness} />
        )}
        {/* Right after setup, before anything has run: the call to run the agent they picked, first in line. */}
        {tryRun ? (
          <TryAgentRun
            userId={me.sub}
            className={PLACE.tryRun}
            onRan={() => setRanTick((n) => n + 1)}
          />
        ) : null}
        <NeedsYou
          data={needsYou}
          now={now.getTime()}
          userId={me.sub}
          pausedFlows={pausedFlows}
          setup={setup.items}
          watch={watch}
          className={PLACE.needs}
        />
        <FleetHealth fleet={fleet} className={PLACE.health} />
        {/* The coding harnesses' behaviour, beside the money the Harnesses tile shows (2026-11-06), right under the
            inbox and fleet health (user 2026-11-06: too important to sit under Spend). A server without
            the harness usage endpoint (own usage only) or nothing visible at the landing scope has none to show. */}
        {harnesses.ownOnly || harnesses.notVisible ? null : (
          <HarnessActivity data={harnesses} days={days} className={PLACE.harness} />
        )}
        {burn.node ? <div className={PLACE.burn}>{burn.node}</div> : null}
        {/* Each card below renders nothing when it has nothing to show (user 2026-11-06); elsewhere (Weave) they keep
            their quiet states. */}
        <Spend spend={spend} now={now.getTime()} className={PLACE.spend} hideEmpty />
        <div className={PLACE.pair}>
          <RecentSessions
            sessions={needsYou.sessions}
            displayName={fleet.displayName}
            now={now.getTime()}
            className={PLACE.sessions}
            hideEmpty
          />
          <SavedByAgent data={savedByAgent} className={PLACE.saved} />
        </div>
        <RecentChats
          byId={fleet.byId}
          username={me.username}
          now={now.getTime()}
          className={PLACE.chats}
          hideEmpty
        />
      </div>
    </Frame>
  )
}
