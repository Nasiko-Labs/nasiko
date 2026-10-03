/**
 * LLM router queries and mutations (plan §4, §7), keys under `['router']`.
 * - Per-agent routing reads fan out through the router's own limiter (eng #4): no server batch read exists (R-L13).
 *   They stay fresh for the router's cache window and don't refetch on focus; list queries do refetch on focus
 *   (the app default is off), so a config changed in another tab shows up here.
 * - Each multi-step routing write runs in ONE mutationFn (eng #2), so closing the sheet or remounting the page
 *   can't abandon step 2. Results are always re-read, never written into the cache optimistically.
 * - Key-bearing mutations keep nothing: gcTime 0 and reset after settle (eng #5).
 */
import {
  useMutation,
  useQueries,
  useQuery,
  useQueryClient,
  type Query,
  type QueryClient,
  type UseQueryResult,
} from '@tanstack/react-query'
import { useCallback, useEffect, useRef } from 'react'
import { createLimiter } from '@/features/observability/limiter'
import { providersQuery } from '@/features/tokenops/api'
import { apiData, apiFetch, ApiError, withQuery } from '@/lib/api/client'
import { requestPlan, type RoutingState, type RowRead, type Step } from './routing'
import {
  LIST_STALE_MS,
  ROUTING_READ_CONCURRENCY,
  ROUTING_STALE_MS,
  SPEND_DAYS,
  SPEND_LIMIT,
  SPEND_STALE_MS,
} from './tuning'
import type {
  AgentRouting,
  AgentUsage,
  Budget,
  BudgetAlert,
  BudgetStatusResponse,
  CreateBudgetBody,
  UpdateBudgetBody,
  ClassifierPreview,
  ClassifierPreviewBody,
  ClassifierStatus,
  CreateConfigBody,
  CreateCustomProviderBody,
  CustomProvider,
  LlmConfig,
  ModelMapping,
  SecretEntry,
  TestCustomProviderBody,
  TestCustomProviderResult,
  UpdateConfigBody,
  UpdateCustomProviderBody,
} from './types'

export const routerKeys = {
  configs: ['router', 'configs'] as const,
  agent: (id: string) => ['router', 'agent', id] as const,
  agents: ['router', 'agent'] as const,
  secrets: ['router', 'secrets'] as const,
  registry: ['router', 'registry'] as const,
  custom: ['router', 'custom'] as const,
  usage: ['router', 'usage'] as const,
  budgets: ['router', 'budgets'] as const,
  budgetStatus: ['router', 'budgets', 'status'] as const,
  budgetAlerts: ['router', 'budgets', 'alerts'] as const,
  classifier: ['router', 'classifier'] as const,
}

// R-L13: one request per agent until a batch read exists; this page's own limiter, not observability's.
const routerLimiter = createLimiter(ROUTING_READ_CONCURRENCY)

const list = { staleTime: LIST_STALE_MS, refetchOnWindowFocus: true } as const

const configsQuery = {
  queryKey: routerKeys.configs,
  queryFn: ({ signal }: { signal: AbortSignal }) =>
    apiData<LlmConfig[]>('/api/llm-configs', { signal }),
  ...list,
}

/** The route's preload prefetch: the configs list. */
export const prefetchRouter = (client: QueryClient) => void client.prefetchQuery(configsQuery)

export function useConfigs(enabled = true) {
  return useQuery({ ...configsQuery, enabled })
}

/**
 * Row reads don't refetch on focus, so when a focus refetch of the configs list shows a config changed elsewhere
 * (edited, or the default moved), re-read the rows it resolves for. The page's own writes already did.
 */
export function useRowsFollowConfigs(configs: LlmConfig[] | undefined) {
  const qc = useQueryClient()
  const last = useRef<Map<string, string> | null>(null)
  useEffect(() => {
    if (!configs) return
    const now = new Map(configs.map((c) => [c.id, `${c.updated_at}:${c.is_default}`]))
    const prev = last.current
    last.current = now
    if (!prev) return
    const changed = new Set(
      [...now.keys(), ...prev.keys()].filter((id) => now.get(id) !== prev.get(id)),
    )
    if (!changed.size) return
    const defaultMoved = [...changed].some(
      (id) => now.get(id)?.endsWith(':true') || prev.get(id)?.endsWith(':true'),
    )
    void qc.invalidateQueries({
      queryKey: routerKeys.agents,
      predicate: (q) => {
        const r = q.state.data as AgentRouting | undefined
        return (
          !r ||
          changed.has(r.llm_config?.id ?? '') ||
          changed.has(r.llm_config_id ?? '') ||
          (defaultMoved && r.source !== 'attached')
        )
      },
    })
  }, [configs, qc])
}

export function useSecrets() {
  return useQuery({
    queryKey: routerKeys.secrets,
    queryFn: ({ signal }) => apiData<SecretEntry[]>('/api/secrets', { signal }),
    ...list,
  })
}

export function useRegistry() {
  return useQuery({
    queryKey: routerKeys.registry,
    queryFn: ({ signal }) => apiData<ModelMapping[]>('/api/model-registry', { signal }),
    ...list,
  })
}

export function useCustomProviders() {
  return useQuery({
    queryKey: routerKeys.custom,
    queryFn: ({ signal }) => apiData<CustomProvider[]>('/api/custom-providers', { signal }),
    ...list,
  })
}

/** The provider catalog, shared with TokenOps (same key and type), refetched on focus here. */
export function useCatalog(enabled = true) {
  return useQuery({ ...providersQuery, enabled, refetchOnWindowFocus: true })
}

/** Router-metered spend per agent for the last 30 days (E1): one request, never offset paging. */
export function useUsageByAgent(enabled: boolean) {
  const path = withQuery('/api/usage/by-agent', { days: SPEND_DAYS, limit: SPEND_LIMIT })
  return useQuery({
    queryKey: routerKeys.usage,
    queryFn: async ({ signal }) => {
      const body = await apiFetch<{ data?: AgentUsage[] }>(path, { signal })
      if (!body || !Array.isArray(body.data))
        throw new ApiError(200, body, path, `GET ${path} → unexpected response (no data array)`)
      return body.data
    },
    enabled,
    staleTime: SPEND_STALE_MS,
    refetchOnWindowFocus: false,
  })
}

const routingQuery = (id: string) => ({
  queryKey: routerKeys.agent(id),
  queryFn: ({ signal }: { signal: AbortSignal }) =>
    routerLimiter(signal, () => apiData<AgentRouting>(`/api/agents/${id}/llm-config`, { signal })),
  staleTime: ROUTING_STALE_MS,
  refetchOnWindowFocus: false,
})

/**
 * The routing sheet's read: always re-fetched when the sheet opens, since row reads don't refetch on focus and a
 * plan built on a stale `current` could undo a change made in another tab or the CLI.
 */
export function useFreshAgentRouting(id: string) {
  return useQuery({ ...routingQuery(id), refetchOnMount: 'always' })
}

/** One agent's configured routing (the agent card). `enabled` must already hold the owner/superuser gate (eng #9). */
export function useAgentRouting(id: string, enabled: boolean) {
  return useQuery({ ...routingQuery(id), enabled })
}

const toRowReads = (results: UseQueryResult<AgentRouting>[]): RowRead[] =>
  results.map((q) =>
    q.data
      ? { state: 'ok', routing: q.data }
      : q.isError
        ? { state: 'error' }
        : { state: 'pending' },
  )

/** Every owned agent's routing, as row reads (pending / error / ok), in the ids' order. */
export function useRowReads(ids: readonly string[]) {
  const qc = useQueryClient()
  // A module-level `combine` re-runs only when a query's result changes (an inline one runs every render).
  const reads = useQueries({ queries: ids.map((id) => routingQuery(id)), combine: toRowReads })
  const retry = useCallback(
    (id: string) => void qc.refetchQueries({ queryKey: routerKeys.agent(id), exact: true }),
    [qc],
  )
  return { reads, retry }
}

// ─── config mutations ───────────────────────────────────────────────────────

/** Agent rows whose resolved config is this one (or the default, whose agents change with it). */
const touchesConfig = (configId: string, includeDefault: boolean) => (q: Query) => {
  const r = q.state.data as AgentRouting | undefined
  return (
    !r ||
    r.llm_config?.id === configId ||
    r.llm_config_id === configId ||
    (includeDefault && r.source !== 'attached')
  )
}

function afterConfigWrite(qc: QueryClient, configId: string | undefined, includeDefault: boolean) {
  void qc.invalidateQueries({ queryKey: routerKeys.configs })
  void qc.invalidateQueries({
    queryKey: routerKeys.agents,
    predicate: configId ? touchesConfig(configId, includeDefault) : undefined,
  })
}

export type SaveConfig =
  | { mode: 'create'; body: CreateConfigBody }
  | { mode: 'update'; id: string; body: UpdateConfigBody; wasDefault: boolean }

/** Create or update a config. It may carry a key, so the mutation keeps no variables around (eng #5). */
export function useSaveConfig() {
  const qc = useQueryClient()
  return useMutation({
    mutationFn: (v: SaveConfig) =>
      v.mode === 'create'
        ? apiData<LlmConfig>('/api/llm-configs', {
            method: 'POST',
            headers: json,
            body: JSON.stringify(v.body),
          })
        : apiData<LlmConfig>(`/api/llm-configs/${v.id}`, {
            method: 'PATCH',
            headers: json,
            body: JSON.stringify(v.body),
          }),
    gcTime: 0,
    onSettled: (data, _err, v) => {
      const hadKey = !!v.body.secret_value
      afterConfigWrite(
        qc,
        data?.id ?? (v.mode === 'update' ? v.id : undefined),
        v.mode === 'create' ? !!v.body.is_default : v.wasDefault,
      )
      // ensure_secret runs before the config write, so a failed save may still have stored the key (eng #6).
      if (hadKey) void qc.invalidateQueries({ queryKey: routerKeys.secrets })
    },
  })
}

export function useDeleteConfig() {
  const qc = useQueryClient()
  return useMutation({
    // DELETE answers `data: null`, which apiData rejects: read it with apiFetch.
    mutationFn: (c: LlmConfig) =>
      apiFetch<unknown>(`/api/llm-configs/${c.id}`, { method: 'DELETE' }),
    onSettled: (_d, _e, c) => afterConfigWrite(qc, c.id, c.is_default),
  })
}

export function useSetDefault() {
  const qc = useQueryClient()
  return useMutation({
    mutationFn: ({ config, on }: { config: LlmConfig; on: boolean }) =>
      apiData<LlmConfig>(`/api/llm-configs/${config.id}/default`, {
        method: on ? 'POST' : 'DELETE',
      }),
    // Every row on a default (old or new) changes.
    onSettled: () => afterConfigWrite(qc, undefined, true),
  })
}

// ─── agent routing: one mutationFn per plan (eng #2) ───────────────────────

export interface PlanOutcome {
  saved: Step[]
  failed?: { step: Step; error: unknown }
  /** The PATCH response: `LlmConfigUpdateResponse` has no `inbound_format`. */
  after?: Omit<AgentRouting, 'inbound_format'>
}

/**
 * Run `requestPlan(current, desired)` as sequential PATCHes. A step that fails stops the plan and is reported with
 * what was saved before it; a 401 is rethrown so the app's session handler still runs. The agent's row is re-read
 * afterwards either way (a failed step may have written part of the change: the server isn't atomic, R-L9).
 */
export function useRoutingPlan(agentId: string, onOutcome?: (out: PlanOutcome) => void) {
  const qc = useQueryClient()
  return useMutation({
    mutationFn: async ({
      current,
      desired,
    }: {
      current: RoutingState
      desired: RoutingState
    }): Promise<PlanOutcome> => {
      const steps = requestPlan(current, desired)
      const saved: Step[] = []
      let after: PlanOutcome['after']
      for (const step of steps) {
        try {
          after = await apiData<NonNullable<PlanOutcome['after']>>(
            `/api/agents/${agentId}/llm-config`,
            { method: 'PATCH', headers: json, body: JSON.stringify(step.body) },
          )
          saved.push(step)
        } catch (error) {
          if (error instanceof ApiError && error.status === 401) throw error
          return { saved, failed: { step, error }, after }
        }
      }
      return { saved, after }
    },
    // Hook-level, so it still runs when the sheet closes mid-plan (a per-call callback would die with it).
    onSuccess: (out) => onOutcome?.(out),
    onSettled: () => {
      void qc.invalidateQueries({ queryKey: routerKeys.agent(agentId) })
      void qc.invalidateQueries({ queryKey: routerKeys.configs })
    },
  })
}

// ─── custom providers (superuser) ───────────────────────────────────────────

/** A custom-provider write changes both the provider list and the catalog it feeds. */
function afterCustomWrite(qc: QueryClient) {
  void qc.invalidateQueries({ queryKey: routerKeys.custom })
  void qc.invalidateQueries({ queryKey: providersQuery.queryKey })
}

/**
 * `CreateRequest` has no `default_model` (nasiko-cloud-rs 4d57453c; serde drops it), so a default model is set by a
 * follow-up PATCH. If that fails the provider still exists, so the save succeeds (a retry would register it twice)
 * and reports `defaultModelSet: false`.
 */
async function createCustomProvider({ default_model, ...body }: CreateCustomProviderBody) {
  const out = await apiData<{ id: string; label: string }>('/api/custom-providers', {
    method: 'POST',
    headers: json,
    body: JSON.stringify(body),
  })
  if (!default_model) return { ...out, defaultModelSet: true }
  const set = await apiData(`/api/custom-providers/${out.id}`, {
    method: 'PATCH',
    headers: json,
    body: JSON.stringify({ default_model }),
  }).then(
    () => true,
    () => false,
  )
  return { ...out, defaultModelSet: set }
}

export function useSaveCustomProvider() {
  const qc = useQueryClient()
  return useMutation({
    mutationFn: (
      v:
        | { mode: 'create'; body: CreateCustomProviderBody }
        | { mode: 'update'; id: string; body: UpdateCustomProviderBody },
    ) =>
      v.mode === 'create'
        ? createCustomProvider(v.body)
        : apiData<{ id: string }>(`/api/custom-providers/${v.id}`, {
            method: 'PATCH',
            headers: json,
            body: JSON.stringify(v.body),
          }),
    gcTime: 0,
    onSettled: () => afterCustomWrite(qc),
  })
}

export function useTestCustomProvider() {
  return useMutation({
    mutationFn: (body: TestCustomProviderBody) =>
      apiData<TestCustomProviderResult>('/api/custom-providers/test', {
        method: 'POST',
        headers: json,
        body: JSON.stringify(body),
      }),
    gcTime: 0,
  })
}

export function useSyncCustomProvider() {
  const qc = useQueryClient()
  return useMutation({
    mutationFn: (id: string) =>
      apiData<{ discovered_models: number }>(`/api/custom-providers/${id}/sync`, {
        method: 'POST',
      }),
    onSettled: () => afterCustomWrite(qc),
  })
}

export function useDeleteCustomProvider() {
  const qc = useQueryClient()
  return useMutation({
    mutationFn: (id: string) =>
      apiData<{ id: string }>(`/api/custom-providers/${id}`, { method: 'DELETE' }),
    onSettled: () => afterCustomWrite(qc),
  })
}

/** The JSON 409 of a custom-provider delete: the configs that still point at it. */
export function referencingConfigs(err: unknown): string[] | null {
  if (!(err instanceof ApiError) || err.status !== 409) return null
  const b = err.body as { referencing_configs?: unknown } | null
  return Array.isArray(b?.referencing_configs)
    ? b.referencing_configs.filter((x): x is string => typeof x === 'string')
    : null
}

const json = { 'Content-Type': 'application/json' }

// ─── Request classifier ([classifier] companion) ─────────────────────────────

/** Effective classifier backend and counters. Counters move with routing, so this refetches on focus like a list. */
export const classifierStatusQuery = {
  queryKey: routerKeys.classifier,
  queryFn: () => apiData<ClassifierStatus>('/api/llm-router/classifier'),
  ...list,
}

export function useClassifierStatus(enabled = true) {
  return useQuery({ ...classifierStatusQuery, enabled })
}

/**
 * One side-effect-free classification. Prompt text is never cached (gcTime 0); the server reads the
 * backend from deployment config, so the body carries only the text and a `regex`/`configured` choice.
 */
export function useClassifierPreview() {
  return useMutation({
    mutationFn: (body: ClassifierPreviewBody) =>
      apiData<ClassifierPreview>('/api/llm-router/classifier/preview', {
        method: 'POST',
        headers: json,
        body: JSON.stringify(body),
      }),
    gcTime: 0,
  })
}

// ─── R2 budgets (proposed R-L10, mocked) ────────────────────────────────────

/** The caller's budgets; `['router','budgets']` prefixes status and alerts too, so one invalidation re-reads all three. */
export function useBudgets(enabled = true) {
  return useQuery({
    queryKey: [...routerKeys.budgets, 'list'],
    queryFn: ({ signal }) => apiData<Budget[]>('/api/budgets', { signal }),
    enabled,
    ...list,
  })
}

export function useBudgetStatus(enabled: boolean) {
  const path = '/api/budgets/status'
  return useQuery({
    queryKey: routerKeys.budgetStatus,
    queryFn: async ({ signal }) => {
      const body = await apiFetch<BudgetStatusResponse>(path, { signal })
      if (!body || !Array.isArray(body.data))
        throw new ApiError(200, body, path, `GET ${path} → unexpected response (no data array)`)
      return body
    },
    enabled,
    ...list,
  })
}

export function useBudgetAlerts(enabled: boolean) {
  return useQuery({
    queryKey: routerKeys.budgetAlerts,
    queryFn: ({ signal }) => apiData<BudgetAlert[]>('/api/budgets/alerts', { signal }),
    enabled,
    ...list,
  })
}

export type SaveBudget =
  | { mode: 'create'; body: CreateBudgetBody }
  | { mode: 'update'; id: string; body: UpdateBudgetBody }

export function useSaveBudget() {
  const qc = useQueryClient()
  return useMutation({
    mutationFn: (v: SaveBudget) =>
      v.mode === 'create'
        ? apiData<Budget>('/api/budgets', {
            method: 'POST',
            headers: json,
            body: JSON.stringify(v.body),
          })
        : apiData<Budget>(`/api/budgets/${v.id}`, {
            method: 'PUT',
            headers: json,
            body: JSON.stringify(v.body),
          }),
    // Awaited, so the mutation stays pending until the list is re-read (no window where a stale row can be clicked again).
    onSettled: () => qc.invalidateQueries({ queryKey: routerKeys.budgets }),
  })
}

export function useDeleteBudget() {
  const qc = useQueryClient()
  return useMutation({
    // DELETE answers `data: null`, so apiFetch (apiData treats a null data as an error).
    mutationFn: (id: string) => apiFetch(`/api/budgets/${id}`, { method: 'DELETE' }),
    // Awaited, so the mutation stays pending until the list is re-read (no window where a stale row can be clicked again).
    onSettled: () => qc.invalidateQueries({ queryKey: routerKeys.budgets }),
  })
}
