/**
 * The LLM router page (plan §4). Order: title + "How routing works" → tabs: Your agents · Your configs · Providers
 * (`?tab=`, one at a time). The routing summary is the Agents table's own filter tabs, the only thing it filters. It shows configured routing and who pays, not observed calls (R-L5).
 *
 *   useOwnedAgents ─► ids ─► useRowReads (limiter, 4 at a time) ─► rows ─┬─► summary strip / counts / Affects N
 *   useConfigs · useSecrets · useCatalog · useCustomProviders ───────────┴─► sections and sheets
 */
import { useQuery } from '@tanstack/react-query'
import { useCallback, useEffect, useMemo, useRef, useState } from 'react'
import { PageLoader } from '@nasiko/ui/composites/page-loader'
import { Tabs, TabsContent, TabsList, TabsTrigger } from '@nasiko/ui/primitives/tabs'
import { useOwnedAgents } from '@nasiko/features/agents/api'
import { ErrorState } from '@nasiko/features/observability/StateCard'
import { meQuery } from '@nasiko/data-access/auth'
import { siblingsAround } from '@nasiko/app-core/siblings'
import { useSlots } from '@nasiko/app-core/edition/edition-context'
import { useAnnounce } from './announce'
import {
  useCatalog,
  useConfigs,
  useCustomProviders,
  useRegistry,
  useRowReads,
  useRowsFollowConfigs,
  useSecrets,
  useSetDefault,
  useUsageByAgent,
} from './api'
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
import type { AgentUsage, LlmConfig } from './types'

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
  // Budgets are Enterprise only (docs/requirements/budgets.md): a layer may add the tab, and the OSS build has none.
  const budgetSlot = useSlots().budgets
  const hasBudgets = budgetSlot.useRouterTab()
  const BudgetsTab = budgetSlot.RouterTab
  const announce = useAnnounce()

  const [editor, setEditor] = useState<EditorMode | null>(null)
  const [routingFor, setRoutingFor] = useState<string | null>(null)
  // The agents table's order when the sheet opened from it, for the sheet's Previous/Next; [] from elsewhere.
  const [routingOrder, setRoutingOrder] = useState<readonly string[]>([])
  const [customMode, setCustomMode] = useState<CustomMode | null>(null)
  const [deleting, setDeleting] = useState<LlmConfig | null>(null)
  const [updated, setUpdated] = useState<ReadonlyMap<string, number>>(new Map())

  const agentById = useMemo(() => new Map(agents.map((a) => [a.id, a])), [agents])
  const agentLabel = useCallback(
    (id: string) => {
      const a = agentById.get(id)
      return a ? a.display_name || a.name : undefined
    },
    [agentById],
  )
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
  const routingStep = routingFor ? siblingsAround(routingOrder, routingFor) : null
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
  // `#router-budgets` (Needs you, the session ledger) opens the Budgets tab.
  const wanted: RouterTab =
    search.tab ??
    (typeof window !== 'undefined' && window.location.hash === '#router-budgets'
      ? 'budgets'
      : 'agents')
  const tab: RouterTab = wanted === 'budgets' && !hasBudgets ? 'agents' : wanted
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

  // The tab's own `#router-budgets` scroll waits on the page's other reads (it owns the rest).
  const pageReady = !owned.isPending && reads.every((r) => r.state !== 'pending')

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
          <Tabs
            value={tab}
            onValueChange={(t) =>
              setSearch({
                tab: t === 'configs' || t === 'providers' || t === 'budgets' ? t : undefined,
              })
            }
          >
            <TabsList
              data-tour="router-tabs"
              variant="line"
              aria-label={copy.anchorsLabel}
              className="w-full max-w-full justify-start overflow-x-auto shadow-[inset_0_-1px_0_var(--color-border)]"
            >
              {ROUTER_TABS.filter((t) => t !== 'budgets' || hasBudgets).map((t) => (
                <TabsTrigger key={t} value={t} className="flex-none px-3">
                  {copy.anchors[t]}
                </TabsTrigger>
              ))}
            </TabsList>
            <TabsContent value="agents" keepMounted hidden={tab !== 'agents'} className="pt-4">
              <AgentsSection
                filters={
                  <SourceTabs
                    value={source}
                    onChange={(v) => showAgents(v)}
                    tabs={[
                      {
                        value: 'all',
                        label: copy.filterAll,
                        count: String(summary.total),
                        name: copy.summaryAgents(String(summary.total)),
                      },
                      {
                        value: 'default',
                        label: copy.filterNames.default,
                        count: kpiCount(summary.onDefault),
                        name: copy.summaryDefault(countText(summary.onDefault), defaultDetail),
                        title: defaultDetail ?? copy.kpiNoDefault,
                      },
                      {
                        value: 'attached',
                        label: copy.filterNames.attached,
                        count: kpiCount(summary.attached),
                        name: copy.summaryAttached(countText(summary.attached)),
                        title: copy.kpiAttachedNote,
                      },
                      {
                        value: 'none',
                        label: copy.filterNames.none,
                        count: kpiCount(summary.none),
                        name: copy.summaryNone(countText(summary.none)),
                        title: copy.kpiNoneNote,
                      },
                    ]}
                  />
                }
                rows={filtered}
                total={rows.length}
                spend={spend}
                updated={updated}
                onChange={(a, order) => {
                  setRoutingFor(a.id)
                  setRoutingOrder(order)
                }}
                onRetry={retry}
                onShowConfig={(id) => goTo('configs', () => focusEl(`config-${id}`))}
                filterLabel={filterLabel}
              />
            </TabsContent>
            <TabsContent value="configs" keepMounted hidden={tab !== 'configs'} className="pt-4">
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
            {hasBudgets && BudgetsTab ? (
              <TabsContent value="budgets" keepMounted hidden={tab !== 'budgets'} className="pt-4">
                <BudgetsTab
                  agents={agents.map((a) => ({ id: a.id, name: a.display_name || a.name }))}
                  agentName={agentLabel}
                  announce={announce}
                  pageReady={pageReady}
                />
              </TabsContent>
            ) : null}
            <TabsContent
              value="providers"
              keepMounted
              hidden={tab !== 'providers'}
              className="pt-4"
            >
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
        step={
          routingStep
            ? {
                index: routingStep.index,
                count: routingStep.count,
                prev: routingStep.prev,
                next: routingStep.next,
                go: setRoutingFor,
              }
            : undefined
        }
        viewer={{ sub: me?.sub, superuser }}
        configs={configs.data}
        catalog={catalog.data}
        onClose={() => setRoutingFor(null)}
        onSaved={(id, at) => setUpdated((m) => new Map(m).set(id, at))}
      />
      <CustomProviderSheet mode={customMode} onClose={() => setCustomMode(null)} />
      <DeleteConfigDialog
        config={deleting}
        reads={reads}
        attached={attached}
        onClose={() => setDeleting(null)}
        onChangeRouting={(id) => {
          setRoutingFor(id)
          setRoutingOrder([])
        }}
      />
    </div>
  )
}

/**
 * The routing summary as the Agents table's filter: one tab per source with its count, All clears it. Each tab's name
 * is the summary's sentence ("5 attached", "at least 13 on your default"), so a screen reader hears the "at least".
 */
function SourceTabs({
  value,
  onChange,
  tabs,
}: {
  value: SourceFilter | undefined
  onChange: (v: SourceFilter | undefined) => void
  tabs: {
    value: SourceFilter | 'all'
    label: string
    count: string
    name: string
    title?: string
  }[]
}) {
  return (
    <Tabs
      value={value ?? 'all'}
      onValueChange={(v) => onChange(v === 'all' ? undefined : (v as SourceFilter))}
      className="max-w-full min-w-0"
    >
      {/* Phones: the four tabs scroll sideways inside the card rather than overflow it. */}
      <TabsList aria-label={copy.summaryLabel} className="max-w-full justify-start overflow-x-auto">
        {tabs.map((t) => (
          <TabsTrigger
            key={t.value}
            value={t.value}
            aria-label={t.name}
            title={t.title}
            // No tab panels: every tab narrows the one agents table (the tab would otherwise point at no panel).
            aria-controls="router-agents"
          >
            {t.label} <span className="text-xs text-muted-foreground tabular-nums">{t.count}</span>
          </TabsTrigger>
        ))}
      </TabsList>
    </Tabs>
  )
}

/** The tab's count: "13+" while some rows are unread (the tab's name keeps "at least 13"). */
const kpiCount = (c: { n: number; atLeast: boolean }) => `${c.n}${c.atLeast ? '+' : ''}`
