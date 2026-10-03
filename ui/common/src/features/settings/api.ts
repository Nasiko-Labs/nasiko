/**
 * Settings queries and mutations (plans/feat-settings.md §4). Secrets share the router's list (`routerKeys.secrets`),
 * so a key added here shows in a config's picker and the other way round. A revealed secret value never enters the
 * query cache: it is a mutation's result (gcTime 0), held by the page for 30 s at most.
 */
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query'
import { agentKeys } from '@/features/agents/api'
import { routerKeys } from '@/features/router/api'
import type { SecretEntry } from '@/features/router/types'
import { apiData, apiFetch } from '@/lib/api/client'
import { settingsBody, type SettingsValues } from './logic'
import {
  contextStrategySchema,
  pacmsBudgetSchema,
  settingsSchema,
  type BudgetLevel,
  type ContextStrategy,
  type Settings,
  type SettingsField,
} from './types'

const settingsQuery = {
  queryKey: ['settings'] as const,
  queryFn: ({ signal }: { signal: AbortSignal }) =>
    apiFetch<Settings>('/api/settings', { signal, schema: settingsSchema }),
}

export const useSettings = (enabled = true) => useQuery({ ...settingsQuery, enabled })

const json = (method: string, body: unknown): RequestInit => ({
  method,
  headers: { 'Content-Type': 'application/json' },
  body: JSON.stringify(body),
})

/** PUT replaces the whole row (ST-2), so the body is built on a fresh GET, never on the page's copy. */
export function useSaveSettings() {
  const qc = useQueryClient()
  return useMutation({
    mutationFn: async (v: { values: SettingsValues; edited: ReadonlySet<SettingsField> }) => {
      const fresh = await qc.fetchQuery({ ...settingsQuery, staleTime: 0 })
      return apiFetch<Settings>('/api/settings', {
        ...json('PUT', settingsBody(fresh, v.values, v.edited)),
        schema: settingsSchema,
      })
    },
    onSuccess: (saved) => {
      qc.setQueryData(settingsQuery.queryKey, saved)
      // The Agents catalog reads `catalog_tabs` under its own key.
      void qc.invalidateQueries({ queryKey: agentKeys.settings })
    },
  })
}

export function useSecretWrites() {
  const qc = useQueryClient()
  const refresh = () => qc.invalidateQueries({ queryKey: routerKeys.secrets })
  return {
    /** POST upserts, so it adds and replaces (legacy secrets-manager: PUT is update-only). */
    add: useMutation({
      gcTime: 0,
      mutationFn: (v: { name: string; value: string }) =>
        apiData<SecretEntry>('/api/secrets', json('POST', v)),
      onSuccess: refresh,
    }),
    remove: useMutation({
      mutationFn: (name: string) =>
        apiFetch(`/api/secrets/${encodeURIComponent(name)}`, { method: 'DELETE' }),
      onSuccess: refresh,
    }),
    /** `GET /api/secrets/{name}`: the caller's own value, decrypted. Only on an explicit click. */
    read: useMutation({
      gcTime: 0,
      mutationFn: async (name: string) =>
        (await apiData<{ name: string; value: string }>(`/api/secrets/${encodeURIComponent(name)}`))
          .value,
    }),
  }
}

export interface ChatContext {
  strategy: ContextStrategy
  level: BudgetLevel
}

const chatContextQuery = {
  queryKey: ['chat-context'] as const,
  queryFn: async ({ signal }: { signal: AbortSignal }): Promise<ChatContext> => {
    const [s, b] = await Promise.all([
      apiFetch('/api/me/context-strategy', { signal, schema: contextStrategySchema }),
      apiFetch('/api/me/pacms-budget', { signal, schema: pacmsBudgetSchema }),
    ])
    return { strategy: s.strategy, level: b.level }
  },
}

export const useChatContext = () => useQuery(chatContextQuery)

/** Two routes, so only the changed ones are sent; either may fail alone, so the page re-reads both after. */
export function useSaveChatContext() {
  const qc = useQueryClient()
  return useMutation({
    mutationFn: (v: Partial<ChatContext>) =>
      Promise.all([
        v.strategy && apiFetch('/api/me/context-strategy', json('PATCH', { strategy: v.strategy })),
        v.level && apiFetch('/api/me/pacms-budget', json('PATCH', { level: v.level })),
      ]),
    onSettled: () => qc.invalidateQueries({ queryKey: chatContextQuery.queryKey }),
  })
}
