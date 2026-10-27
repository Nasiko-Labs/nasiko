/**
 * Context optimization reads and writes (plans/feat-context-optimization.md §3, eng review F3, E6). Everything lives
 * under `['optimization', …]`; the owned-agents list it reads for compression is the Agents feature's
 * (`['agents', 'owned', owner]`), so the agent switch's invalidation reaches it.
 */
import { queryOptions, useMutation, useQuery, useQueryClient } from '@tanstack/react-query'
import type { ResolvedWindow } from '@nasiko/features/tokenops/window'
import { apiData, apiFetch, ApiError, withQuery, type Checked } from '@nasiko/data-access/client'
import { errorCode, isEndpointAbsent, isUnauthorized } from '@nasiko/data-access/detect'
import type { PreferenceEdit, SaveOutcome } from './logic'
import {
  budgetBodySchema,
  contextSavingsSchema,
  previewSchema,
  savingsAgentsSchema,
  settingsHistorySchema,
  strategyBodySchema,
  topRequestsSchema,
  tiersSchema,
  type BudgetBody,
  type ContextSavings,
  type Preview,
  type SavingsAgent,
  type SettingsChange,
  type Strategy,
  type TopRequest,
  type StrategyBody,
  type FullTiers,
  type Tiers,
} from './types'

const STRATEGY_PATH = '/api/me/context-strategy'
const BUDGET_PATH = '/api/me/pacms-budget'
const TIERS_PATH = '/api/settings/context-tiers'
const PREVIEW_PATH = '/api/me/context-preview'
/** Longer than any page visit; the page removes the previews on leaving anyway. */
const PREVIEW_KEEP_MS = 60 * 60_000
const HISTORY_PATH = '/api/me/settings-history'

const optimizationKeys = {
  strategy: ['optimization', 'strategy'] as const,
  budget: ['optimization', 'budget'] as const,
  tiers: ['optimization', 'tiers'] as const,
  preview: (strategy: Strategy) => ['optimization', 'preview', strategy] as const,
}

export const strategyQuery = queryOptions({
  queryKey: optimizationKeys.strategy,
  queryFn: ({ signal }) =>
    apiFetch<StrategyBody>(STRATEGY_PATH, { signal, schema: strategyBodySchema }),
})

export const budgetQuery = queryOptions({
  queryKey: optimizationKeys.budget,
  queryFn: ({ signal }) => apiFetch<BudgetBody>(BUDGET_PATH, { signal, schema: budgetBodySchema }),
})

/** CX-T1: `null` when the server has no such route (today's), so the pages show defaults and no figures (2B). */
export const tiersQuery = queryOptions({
  queryKey: optimizationKeys.tiers,
  queryFn: async ({ signal }): Promise<Tiers | null> => {
    try {
      return await apiFetch<Tiers>(TIERS_PATH, { signal, schema: tiersSchema })
    } catch (err) {
      if (isEndpointAbsent(err)) return null
      throw err
    }
  },
})

export type PreviewResult =
  { kind: 'ready'; preview: Preview } | { kind: 'absent' } | { kind: 'no-session' }

/**
 * CX-6 (proposed): a dry run on the latest session for one strategy, every tier in one answer. It spends embedding
 * calls on the server, so a result is kept for the page visit (eng review E6): no refetch on focus or remount.
 */
export const previewQuery = (strategy: Strategy) =>
  queryOptions({
    queryKey: optimizationKeys.preview(strategy),
    queryFn: async ({ signal }): Promise<PreviewResult> => {
      try {
        const preview = await apiFetch<Preview>(PREVIEW_PATH, {
          signal,
          method: 'POST',
          headers: { 'Content-Type': 'application/json' },
          body: JSON.stringify({ strategy }),
          schema: previewSchema,
        })
        return { kind: 'ready', preview }
      } catch (err) {
        if (errorCode(err) === 'no_session') return { kind: 'no-session' }
        if (isEndpointAbsent(err)) return { kind: 'absent' }
        throw err
      }
    },
    staleTime: Infinity,
    refetchOnWindowFocus: false,
    // Kept for the page visit (E6): switching PACMS → Top-K → PACMS must not repeat the dry run, so an unobserved
    // strategy's preview stays cached; `LastChatPreview` drops the family when it unmounts, so the next visit runs on
    // the latest chat (review: Codex).
    gcTime: PREVIEW_KEEP_MS,
    // A dry run spends embedding calls; a failure shows "Preview unavailable." with its own Retry instead of repeating it.
    retry: false,
  })

const patch = (body: unknown): RequestInit => ({
  method: 'PATCH',
  headers: { 'Content-Type': 'application/json' },
  body: JSON.stringify(body),
})

/** The server's own words for a failed write (a 422 Json rejection or "internal error"), else a fallback. */
function reason(err: unknown, fallback: string): string {
  return (err instanceof ApiError && err.serverMessage) || fallback
}

/**
 * Save (design review 2C): the strategy route (with the switch, CX-5) and the budget route are sent together, and each
 * settles on its own. A 401 from either is rethrown so the app's one expiry path takes it.
 */
export function useSavePreferences(fallback: string) {
  const qc = useQueryClient()
  return useMutation({
    mutationFn: async ({
      edit,
      strategy,
    }: {
      edit: PreferenceEdit
      /** The draft strategy: the route needs one even when only the switch changed. */
      strategy: Strategy
    }): Promise<SaveOutcome> => {
      const jobs: { fields: ('strategy' | 'enabled' | 'level')[]; run: Promise<unknown> }[] = []
      if (edit.strategy !== undefined || edit.enabled !== undefined) {
        const body: StrategyBody = { strategy: edit.strategy ?? strategy }
        if (edit.enabled !== undefined) body.enabled = edit.enabled
        const fields: ('strategy' | 'enabled')[] = []
        if (edit.strategy !== undefined) fields.push('strategy')
        if (edit.enabled !== undefined) fields.push('enabled')
        jobs.push({ fields, run: apiFetch(STRATEGY_PATH, patch(body)) })
      }
      if (edit.level !== undefined)
        jobs.push({ fields: ['level'], run: apiFetch(BUDGET_PATH, patch({ level: edit.level })) })
      const settled = await Promise.allSettled(jobs.map((j) => j.run))
      const expired = settled.find((r) => r.status === 'rejected' && isUnauthorized(r.reason))
      if (expired && expired.status === 'rejected') throw expired.reason
      const outcome: SaveOutcome = { saved: [], failed: [] }
      settled.forEach((r, i) => {
        for (const field of jobs[i].fields) {
          if (r.status === 'fulfilled') outcome.saved.push(field)
          else outcome.failed.push({ field, message: reason(r.reason, fallback) })
        }
      })
      return outcome
    },
    onSettled: () =>
      Promise.all([
        qc.invalidateQueries({ queryKey: optimizationKeys.strategy }),
        qc.invalidateQueries({ queryKey: optimizationKeys.budget }),
        // The lead's "latest change" line follows a save made on the page (review: red team).
        qc.invalidateQueries({ queryKey: ['optimization', 'history'] }),
      ]),
  })
}

const SAVINGS_PATH = '/api/observability/finops/context-savings'

/** A proposed read that a server may not have yet (bare 404 → `absent`, R2B). */
export type Proposed<T> = { kind: 'ready'; data: T } | { kind: 'absent' }

async function proposed<T>(
  path: string,
  signal: AbortSignal,
  schema: Checked<T>['schema'],
): Promise<Proposed<T>> {
  try {
    return { kind: 'ready', data: await apiData<T>(path, { signal, schema }) }
  } catch (err) {
    if (isEndpointAbsent(err)) return { kind: 'absent' }
    throw err
  }
}

/**
 * The /optimization lead (eng E1, CX-V3a): the window's totals with its buckets, hourly for 24h and daily otherwise, in
 * one read under the same key family as TokenOps' panel (whole-window, no agent filter).
 */
export function useSavingsTrend(win: Pick<ResolvedWindow, 'key' | 'params'>, enabled: boolean) {
  const series = win.params.range === '24h' ? 'hourly' : 'daily'
  const path = withQuery(SAVINGS_PATH, { ...win.params, series })
  return useQuery({
    queryKey: ['tokenops', 'context-savings', win.key, null, series] as const,
    queryFn: ({ signal }) => proposed<ContextSavings>(path, signal, contextSavingsSchema),
    enabled,
    meta: { path },
  })
}

/** CX-V3b (proposed): savings per agent for the window, history volume first (By agent, R7B). */
export function useSavingsByAgent(win: Pick<ResolvedWindow, 'key' | 'params'>, enabled: boolean) {
  const path = withQuery(`${SAVINGS_PATH}/agents`, { ...win.params })
  return useQuery({
    queryKey: ['tokenops', 'context-savings', win.key, null, 'agents'] as const,
    queryFn: ({ signal }) => proposed<SavingsAgent[]>(path, signal, savingsAgentsSchema),
    enabled,
    meta: { path },
  })
}

/** How many requests Biggest senders lists (R1C). */
const TOP_REQUESTS = 5

/** CX-H as sent: `from`/`to` are strings, though a boolean `enabled` is tolerated. */
type WireChange = Omit<SettingsChange, 'from' | 'to'> & {
  from: string | boolean
  to: string | boolean
}

/**
 * CX-V3c (proposed): the requests that carried the most history in the window, or in one chart bar's interval (C5).
 * Other users' chats come back redacted (C4).
 */
export function useTopRequests(
  win: Pick<ResolvedWindow, 'key' | 'params' | 'start' | 'end'>,
  slice: { from: string; to: string } | null,
  enabled: boolean,
) {
  // With a picked bar, the window is sent as the page's own absolute bounds, not `range`: the server would resolve a
  // range against its clock, and an hour later the bar's interval could fall outside it (review: Codex).
  const path = withQuery(`${SAVINGS_PATH}/top-requests`, {
    ...(slice
      ? { start_time: win.start.toISOString(), end_time: win.end.toISOString() }
      : win.params),
    from: slice?.from,
    to: slice?.to,
    limit: TOP_REQUESTS,
  })
  return useQuery({
    queryKey: [
      'tokenops',
      'context-savings',
      win.key,
      null,
      'top',
      slice ? `${slice.from}/${slice.to}` : null,
    ] as const,
    queryFn: ({ signal }) => proposed<TopRequest[]>(path, signal, topRequestsSchema),
    enabled,
    meta: { path },
  })
}

/**
 * CX-H (proposed): your setting changes in the window, newest first. A descriptive note only (C6), so a server without
 * the route, or a failed read, just shows none: `retry: false`, nothing on the page waits for it.
 */
export function useSettingsHistory(
  win: Pick<ResolvedWindow, 'key' | 'start' | 'end' | 'preset'>,
  enabled: boolean,
) {
  // A window that ends at now sends no end: its end is frozen, and a change saved after it froze must still show (review:
  // red team). A past window (last month, custom) keeps its end, so a later change never shows under it (adversarial).
  const past = win.preset === 'custom' || win.preset === 'last-month'
  const path = withQuery(HISTORY_PATH, {
    start_time: win.start.toISOString(),
    end_time: past ? win.end.toISOString() : undefined,
  })
  return useQuery({
    queryKey: ['optimization', 'history', win.key] as const,
    queryFn: async ({ signal }): Promise<SettingsChange[]> => {
      try {
        const rows = await apiFetch<WireChange[]>(path, { signal, schema: settingsHistorySchema })
        return rows.map((c) => ({ ...c, from: String(c.from), to: String(c.to) }))
      } catch (err) {
        if (isEndpointAbsent(err)) return []
        throw err
      }
    },
    enabled,
    retry: false,
    meta: { path },
  })
}

/**
 * CX-T1 (proposed) `PUT /api/settings/context-tiers` (superuser): the whole set, applied per request on the server,
 * guarded by the `updated_at` the form loaded (409 when another save landed first; ledger V2).
 */
export function useSaveTiers() {
  const qc = useQueryClient()
  return useMutation({
    mutationFn: ({ tiers, expectedUpdatedAt }: { tiers: FullTiers; expectedUpdatedAt: string }) =>
      apiFetch<Tiers>(TIERS_PATH, {
        method: 'PUT',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ ...tiers, expected_updated_at: expectedUpdatedAt }),
      }),
    onSettled: () => qc.invalidateQueries({ queryKey: optimizationKeys.tiers }),
  })
}
