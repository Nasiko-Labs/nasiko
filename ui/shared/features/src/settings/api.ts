/**
 * Settings queries and mutations (plans/feat-settings.md §4). Secrets share the router's list (`routerKeys.secrets`),
 * so a key added here shows in a config's picker and the other way round. A revealed secret value never enters the
 * query cache: it is a mutation's result (gcTime 0), held by the page for 30 s at most.
 */
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query'
import { agentKeys } from '@nasiko/features/agents/api'
import { routerKeys } from '@nasiko/features/router/api'
import type { SecretEntry } from '@nasiko/features/router/types'
import { apiData, apiFetch } from '@nasiko/data-access/client'
import { settingsBody, type SettingsValues } from './logic'
import { settingsSchema, type Settings, type SettingsField } from './types'

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
