/**
 * The LLM router page (plan §4). Order: title + "How routing works" → summary tiles (filters) → tabs: Your agents ·
 * Your configs · Providers (`?tab=`, one at a time). It shows configured routing and who pays, not observed calls (R-L5).
 *
 *   useOwnedAgents ─► ids ─► useRowReads (limiter, 4 at a time) ─► rows ─┬─► summary strip / counts / Affects N
 *   useConfigs · useSecrets · useCatalog · useCustomProviders ───────────┴─► sections and sheets
 */
import { useQuery } from '@tanstack/react-query'
import { Check } from 'lucide-react'
import { useCallback, useEffect, useMemo, useRef, useState } from 'react'
import { PageLoader } from '@/components/shared/page-loader'
import { Tabs, TabsContent, TabsList, TabsTrigger } from '@/components/ui/tabs'
import { Toggle } from '@/components/ui/toggle'
import { useOwnedAgents } from '@/features/agents/api'
import { ErrorState } from '@/features/observability/StateCard'
import { meQuery } from '@/lib/api/auth'
import { cn } from '@/lib/utils'
import { useAnnounce } from './announce'
import {
  // useBudgetAlerts,
  // useBudgets,
  // useBudgetStatus,
  useCatalog,
  useConfigs,
  useCustomProviders,
  useRegistry,
  useRowReads,
  useRowsFollowConfigs,
  // useSaveBudget,
  useSecrets,
  useSetDefault,
  useUsageByAgent,
} from './api'
// import { budgetResetsAt, sentenceName, toAlertOnly } from './budgets'
// import { BudgetSheet, type BudgetMode } from './components/BudgetSheet'
// import { BudgetsSection } from './components/BudgetsSection'
import { AgentsSection, type AgentRow, type SpendState } from './components/AgentsSection'
import { Announcer } from './components/bits'
import { RouterTitle } from './components/HowRouting'
import { ConfigSheet, type EditorMode } from './components/ConfigSheet'
import { ConfigsSection } from './components/ConfigsSection'
import { CustomProviderSheet, type CustomMode } from './components/CustomProviderSheet'
import { DeleteConfigDialog } from './components/DeleteConfigDialog'
import { ProvidersSection } from './components/ProvidersSection'
import { RoutingSheet } from './components/RoutingSheet'
import { copy } from './copy'
import { routerError } from './errors'
import {
  attachedAgents,
  catalogIndex,
  configWarnings,
  countText,
  duplicateName,
  summarize,
  type ClearableField,
} from './routing'
import { ROUTER_TABS, type RouterSearch, type RouterTab, type SourceFilter } from './search'
import { SPEND_LIMIT } from './tuning'
import type { AgentUsage, /* Budget, */ LlmConfig } from './types'

const FILTER_SOURCE: Record<SourceFilter, 'attached' | 'owner-default' | 'none'> = {
  attached: 'attached',
  default: 'owner-default',
  none: 'none',
}

export function RouterPage({
  search,
  setSearch,
}: {
  search: RouterSearch
  setSearch: (patch: Partial<RouterSearch>) => void
}) {
  return (
    <Announcer>
      <Page search={search} setSearch={setSearch} />
    </Announcer>
  )
}

function Page({
  search,
  setSearch,
}: {
  search: RouterSearch
  setSearch: (patch: Partial<RouterSearch>) => void
}) {
  const me = useQuery(meQuery).data
  const superuser = !!me?.is_superuser
  const owned = useOwnedAgents(me?.sub)
  const agents = useMemo(
    () =>
      [...(owned.data ?? [])].sort((a, b) =>
        (a.display_name || a.name).localeCompare(b.display_name || b.name),
      ),
    [owned.data],
  )
  const ids = useMemo(() => agents.map((a) => a.id), [agents])
  const { reads, retry } = useRowReads(ids)
  const configs = useConfigs()
  useRowsFollowConfigs(configs.data)
  const secrets = useSecrets()
  const catalog = useCatalog()
  const custom = useCustomProviders()
  const registry = useRegistry()
  const usage = useUsageByAgent(ids.length > 0)
  const setDefault = useSetDefault()
  // Budgets hidden: no server support for /api/budgets yet (R-L10). Uncomment when it lands.
  // const budgets = useBudgets()
  // // In parallel with the list; skipped only once the list itself failed.
  // const budgetStatus = useBudgetStatus(!budgets.isError)
  // const budgetAlerts = useBudgetAlerts(!budgets.isError)
  // const switchBudget = useSaveBudget()
  // // The forecast's clock is when the status was read, so its elapsed days match the daily series it forecasts from.
  // // Before the first status read, only the reset date's fallback needs a clock: the mount time.
  // const [mountedAt] = useState(() => new Date())
  // const statusAt = useMemo(
  //   () => (budgetStatus.dataUpdatedAt ? new Date(budgetStatus.dataUpdatedAt) : mountedAt),
  //   [budgetStatus.dataUpdatedAt, mountedAt],
  // )
  // const resetsAt = budgetResetsAt(budgetStatus.data?.data, statusAt)
  // const [budgetMode, setBudgetMode] = useState<BudgetMode | null>(null)
  // const [switching, setSwitching] = useState<ReadonlySet<string>>(new Set())
  const announce = useAnnounce()

  const [editor, setEditor] = useState<EditorMode | null>(null)
  const [routingFor, setRoutingFor] = useState<string | null>(null)
  const [customMode, setCustomMode] = useState<CustomMode | null>(null)
  const [deleting, setDeleting] = useState<LlmConfig | null>(null)
  const [updated, setUpdated] = useState<ReadonlyMap<string, number>>(new Map())

  const agentById = useMemo(() => new Map(agents.map((a) => [a.id, a])), [agents])
  // Budgets hidden: no server support for /api/budgets yet (R-L10). Uncomment when it lands.
  // const agentLabel = useCallback(
  //   (id: string) => {
  //     const a = agentById.get(id)
  //     return a ? a.display_name || a.name : undefined
  //   },
  //   [agentById],
  // )
  const rows: AgentRow[] = useMemo(
    () => agents.map((agent, i) => ({ agent, read: reads[i] ?? { state: 'pending' } })),
    [agents, reads],
  )
  const summary = useMemo(() => summarize(reads), [reads])
  const customLabels = useMemo(() => (custom.data ?? []).map((c) => c.label), [custom.data])
  const idx = useMemo(() => catalogIndex(catalog.data), [catalog.data])
  const secretNames = useMemo(() => secrets.data?.map((s) => s.name), [secrets.data])
  const warnings = useCallback(
    (c: LlmConfig) =>
      configWarnings(c, { secrets: secretNames, customLabels, idx, catalogLoaded: !!catalog.data }),
    [secretNames, customLabels, idx, catalog.data],
  )

  const spend: SpendState = useMemo(() => {
    const owned = new Set(ids)
    const byAgent = new Map<string, AgentUsage>()
    let dropped = 0
    for (const u of usage.data ?? []) {
      if (u.agent_id && owned.has(u.agent_id)) byAgent.set(u.agent_id, u)
      else dropped += u.request_count
    }
    return {
      byAgent,
      dropped,
      failed: usage.isError,
      pending: usage.isPending && ids.length > 0,
      partial: (usage.data?.length ?? 0) >= SPEND_LIMIT,
      anyUnpriced: [...byAgent.values()].some((u) => u.request_count > 0 && u.total_cost_usd === 0),
    }
  }, [usage.data, usage.isError, usage.isPending, ids])

  const source = search.source
  const filtered = source
    ? rows.filter((r) => r.read.state === 'ok' && r.read.routing.source === FILTER_SOURCE[source])
    : rows
  const filterLabel = search.source ? copy.filterNames[search.source] : null
  const routingAgent = routingFor ? agentById.get(routingFor) : undefined
  const attached = useMemo(
    () =>
      deleting
        ? attachedAgents(
            deleting.id,
            rows.map((r) => ({ id: r.agent.id, read: r.read })),
          ).map((id) => [id, agentById.get(id)?.display_name || id] as const)
        : [],
    [deleting, rows, agentById],
  )

  const onDefault = (c: LlmConfig, on: boolean) =>
    setDefault.mutate(
      { config: c, on },
      {
        onSuccess: () => announce(copy.saved(c.name)),
        onError: (e) => announce(routerError(e).problem),
      },
    )
  // Budgets hidden: no server support for /api/budgets yet (R-L10). Uncomment when it lands.
  // const onSwitchToAlert = (b: Budget) => {
  //   setSwitching((s) => new Set(s).add(b.id))
  //   const name = sentenceName(b, agentLabel)
  //   switchBudget.mutate(
  //     { mode: 'update', id: b.id, body: toAlertOnly(b) },
  //     {
  //       onSuccess: () => {
  //         announce(copy.switchedToAlert(name))
  //         // The switch button goes with the stop: keep keyboard focus in the row, on its Raise limit.
  //         ;(document.querySelector(`[data-raise="${b.id}"]`) as HTMLElement | null)?.focus()
  //       },
  //       onError: (e) => announce(routerError(e).problem),
  //       onSettled: () =>
  //         setSwitching((s) => {
  //           const n = new Set(s)
  //           n.delete(b.id)
  //           return n
  //         }),
  //     },
  //   )
  // }
  const onDuplicate = (source: LlmConfig, without?: ClearableField) =>
    setEditor({
      kind: 'duplicate',
      source,
      without,
      name: duplicateName(
        source.name,
        (configs.data ?? []).map((c) => c.name),
      ),
    })
  const focusSection = (id: string) => {
    const el = document.getElementById(id)
    el?.scrollIntoView({ block: 'start' })
    ;(el?.querySelector('h2') as HTMLElement | null)?.focus()
  }
  const focusEl = (id: string) => {
    const el = document.getElementById(id)
    el?.scrollIntoView({ block: 'center' })
    el?.focus()
  }
  // Every tab stays mounted (inactive ones `hidden`, so each keeps its state); a jump into another tab focuses once
  // that tab is shown.
  const tab: RouterTab = search.tab ?? 'agents'
  const pendingFocus = useRef<(() => void) | null>(null)
  useEffect(() => {
    const focus = pendingFocus.current
    pendingFocus.current = null
    focus?.()
  }, [tab])
  const goTo = (t: RouterTab, focus: () => void) => {
    if (t === tab) return focus()
    pendingFocus.current = focus
    setSearch({ tab: t === 'agents' ? undefined : t })
  }
  /** A summary tile (or "Show them") filters Your agents, so it opens their tab. */
  const showAgents = (next: SourceFilter | undefined, focus = false) => {
    setSearch({ source: next, tab: undefined })
    if (!focus) return
    if (tab === 'agents') focusSection('router-agents')
    else pendingFocus.current = () => focusSection('router-agents')
  }

  // Budgets hidden: no server support for /api/budgets yet (R-L10). Uncomment when it lands.
  // // TokenOps links to #router-budgets: scroll once the section and the agents table above it have their final height.
  // const budgetsReady =
  //   !budgets.isPending && !owned.isPending && reads.every((r) => r.state !== 'pending')
  // useEffect(() => {
  //   if (budgetsReady && window.location.hash === '#router-budgets')
  //     document.getElementById('router-budgets')?.scrollIntoView({ block: 'start' })
  // }, [budgetsReady])

  if (owned.isError && !owned.data) {
    return (
      <div className="space-y-4">
        <RouterTitle configsEmpty={false} />
        <ErrorState error={owned.error} onRetry={() => void owned.refetch()} />
      </div>
    )
  }

  const defaultDetail = summary.defaultConfig
    ? copy.defaultDetail(
        summary.defaultConfig.provider,
        !!summary.defaultConfig.api_key_secret_name,
      )
    : null

  return (
    <div className="space-y-5">
      <RouterTitle
        configsEmpty={!!configs.data && configs.data.length === 0}
        onDefault={
          configs.data?.some((c) => c.is_default) ? countText(summary.onDefault) : undefined
        }
        total={String(summary.total)}
        defaultName={configs.data?.find((c) => c.is_default)?.name}
        onShowDefaults={() => showAgents('default', true)}
      />
      {/* One loader until the first paint's reads settle (data or error); a section's error still shows its card. */}
      {owned.isPending || configs.isPending || catalog.isPending ? (
        <PageLoader label={copy.loading} />
      ) : (
        <>
          <nav aria-label={copy.summaryLabel} className="grid grid-cols-2 gap-3 lg:grid-cols-4">
            <FilterTile
              active={!source}
              onClick={() => showAgents(undefined)}
              name={copy.summaryAgents(String(summary.total))}
              label={copy.kpiAgents}
              value={String(summary.total)}
              note={copy.kpiAgentsNote}
            />
            <FilterTile
              active={source === 'default'}
              onClick={() => showAgents(source === 'default' ? undefined : 'default')}
              name={copy.summaryDefault(countText(summary.onDefault), defaultDetail)}
              label={copy.filterNames.default}
              value={kpiCount(summary.onDefault)}
              note={defaultDetail ?? copy.kpiNoDefault}
            />
            <FilterTile
              active={source === 'attached'}
              onClick={() => showAgents(source === 'attached' ? undefined : 'attached')}
              name={copy.summaryAttached(countText(summary.attached))}
              label={copy.filterNames.attached}
              value={kpiCount(summary.attached)}
              note={copy.kpiAttachedNote}
            />
            <FilterTile
              active={source === 'none'}
              onClick={() => showAgents(source === 'none' ? undefined : 'none')}
              name={copy.summaryNone(countText(summary.none))}
              label={copy.filterNames.none}
              value={kpiCount(summary.none)}
              note={copy.kpiNoneNote}
            />
          </nav>

          <Tabs
            value={tab}
            onValueChange={(t) =>
              setSearch({ tab: t === 'configs' || t === 'providers' ? t : undefined })
            }
          >
            <TabsList aria-label={copy.anchorsLabel}>
              {ROUTER_TABS.map((t) => (
                <TabsTrigger key={t} value={t} className="px-3 pointer-coarse:min-h-11">
                  {copy.anchors[t]}
                </TabsTrigger>
              ))}
            </TabsList>
            <TabsContent value="agents" forceMount hidden={tab !== 'agents'} className="pt-3">
              <AgentsSection
                rows={filtered}
                total={rows.length}
                spend={spend}
                updated={updated}
                onChange={(a) => setRoutingFor(a.id)}
                onRetry={retry}
                onShowConfig={(id) => goTo('configs', () => focusEl(`config-${id}`))}
                filterLabel={filterLabel}
                onClearFilter={() => setSearch({ source: undefined })}
              />
            </TabsContent>
            <TabsContent value="configs" forceMount hidden={tab !== 'configs'} className="pt-3">
              <ConfigsSection
                configs={configs}
                reads={reads}
                warnings={warnings}
                onNew={() => setEditor({ kind: 'create' })}
                onEdit={(c) => setEditor({ kind: 'edit', config: c })}
                onDuplicate={(c) => onDuplicate(c)}
                onDelete={setDeleting}
                onDefault={onDefault}
              />
            </TabsContent>
            {/* Budgets hidden: no server support for /api/budgets yet (R-L10). Uncomment when it lands (as a tab).
        <BudgetsSection
          budgets={budgets}
          status={budgetStatus}
          statusAt={statusAt}
          alerts={budgetAlerts}
          agentName={agentLabel}
          resetsAt={resetsAt}
          pending={switching}
          onNew={() => setBudgetMode({ kind: 'create' })}
          onEdit={(b) => setBudgetMode({ kind: 'edit', budget: b })}
          onSwitchToAlert={onSwitchToAlert}
        />
        */}
            <TabsContent value="providers" forceMount hidden={tab !== 'providers'} className="pt-3">
              <ProvidersSection
                catalog={catalog}
                custom={custom}
                registry={registry}
                superuser={superuser}
                onAdd={() => setCustomMode({ kind: 'create' })}
                onEdit={(p) => setCustomMode({ kind: 'edit', provider: p })}
              />
            </TabsContent>
          </Tabs>
        </>
      )}

      <ConfigSheet
        mode={editor}
        onClose={() => setEditor(null)}
        configs={configs.data}
        catalog={catalog}
        customLabels={customLabels}
        reads={reads}
        onDuplicate={(src, without) => onDuplicate(src, without)}
        onSetDefault={(c) => onDefault(c, true)}
        onFocusAgents={() => goTo('agents', () => focusSection('router-agents'))}
      />
      <RoutingSheet
        target={
          routingAgent
            ? {
                id: routingAgent.id,
                name: routingAgent.name,
                displayName: routingAgent.display_name || routingAgent.name,
                ownerId: routingAgent.owner_id,
              }
            : null
        }
        viewer={{ sub: me?.sub, superuser }}
        configs={configs.data}
        catalog={catalog.data}
        onClose={() => setRoutingFor(null)}
        onSaved={(id, at) => setUpdated((m) => new Map(m).set(id, at))}
      />
      <CustomProviderSheet mode={customMode} onClose={() => setCustomMode(null)} />
      {/* Budgets hidden: no server support for /api/budgets yet (R-L10). Uncomment when it lands.
      <BudgetSheet
        mode={budgetMode}
        budgets={budgets.data ?? []}
        agents={agents.map((a) => ({ id: a.id, name: a.display_name || a.name }))}
        agentName={agentLabel}
        status={
          budgetMode?.kind === 'edit'
            ? budgetStatus.data?.data.find((st) => st.budget_id === budgetMode.budget.id)
            : undefined
        }
        resetsAt={resetsAt}
        onClose={() => setBudgetMode(null)}
      />
      */}
      <DeleteConfigDialog
        config={deleting}
        reads={reads}
        attached={attached}
        onClose={() => setDeleting(null)}
        onChangeRouting={(id) => setRoutingFor(id)}
      />
    </div>
  )
}

/** A summary figure that filters Your agents (`aria-pressed`); its name is the strip's sentence ("5 attached"). */
function FilterTile({
  active,
  onClick,
  name,
  label,
  value,
  note,
}: {
  active: boolean
  onClick: () => void
  name: string
  label: string
  value: string
  note: string
}) {
  return (
    <Toggle
      pressed={active}
      onPressedChange={onClick}
      aria-label={name}
      className={cn(
        'h-auto min-w-0 flex-col items-stretch justify-start gap-1 rounded-lg border bg-card p-3 text-left font-normal whitespace-normal text-foreground shadow-xs transition-[border-color,box-shadow] hover:border-ring/60 hover:bg-card hover:text-foreground motion-reduce:transition-none',
        'data-[state=on]:border-primary data-[state=on]:bg-card data-[state=on]:text-foreground data-[state=on]:ring-1 data-[state=on]:ring-primary',
      )}
    >
      <span className="flex items-center justify-between gap-2 text-xs text-muted-foreground">
        {label}
        {active ? <Check className="size-3.5 text-primary-text" aria-hidden /> : null}
      </span>
      <span className="text-2xl font-semibold tabular-nums">{value}</span>
      <span className="truncate text-xs text-muted-foreground">{note}</span>
    </Toggle>
  )
}

/** The tile's figure: "13+" while some rows are unread (the tile's name keeps "at least 13"). */
const kpiCount = (c: { n: number; atLeast: boolean }) => `${c.n}${c.atLeast ? '+' : ''}`
