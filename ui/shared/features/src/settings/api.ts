// Ported from nasiko-ui-lab@1b99bdda common/src/features/settings/api.ts. Local edits: every query key is scoped:
// `settingsQuery` is a builder taking the scope (`productKey(scope, "settings")`, the lab's `['settings']`), the
// router's secrets key is `routerKeys.secrets(scope)` and the agents catalog's settings key
// `agentKeys.settings(scope)`, the hooks read the scope from `useProductScope()`; the fresh read before the PUT is
// `qc.query` (the lab's deprecated `fetchQuery`; staleTime 0, so always a new GET); Prettier formatting.
/**
 * Settings queries and mutations (plans/feat-settings.md §4). Secrets share the router's list (`routerKeys.secrets`),
 * so a key added here shows in a config's picker and the other way round. A revealed secret value never enters the
 * query cache: it is a mutation's result (gcTime 0), held by the page for 30 s at most.
 */
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { agentKeys } from "../agents/api";
import { routerKeys } from "../router/api";
import type { SecretEntry } from "../router/types";
import type { Scope } from "@nasiko/data-access";
import { apiData, apiFetch, productKey, useProductScope } from "@nasiko/data-access/product";
import { settingsBody, type SettingsValues } from "./logic";
import { settingsSchema, type Settings, type SettingsField } from "./types";

const settingsQuery = (scope: Scope) => ({
  queryKey: productKey(scope, "settings"),
  queryFn: ({ signal }: { signal: AbortSignal }) =>
    apiFetch<Settings>("/api/settings", { signal, schema: settingsSchema }),
});

export const useSettings = (enabled = true) =>
  useQuery({ ...settingsQuery(useProductScope()), enabled });

const json = (method: string, body: unknown): RequestInit => ({
  method,
  headers: { "Content-Type": "application/json" },
  body: JSON.stringify(body),
});

/** PUT replaces the whole row (ST-2), so the body is built on a fresh GET, never on the page's copy. */
export function useSaveSettings() {
  const qc = useQueryClient();
  const scope = useProductScope();
  return useMutation({
    mutationFn: async (v: { values: SettingsValues; edited: ReadonlySet<SettingsField> }) => {
      const fresh = await qc.query({ ...settingsQuery(scope), staleTime: 0 });
      return apiFetch<Settings>("/api/settings", {
        ...json("PUT", settingsBody(fresh, v.values, v.edited)),
        schema: settingsSchema,
      });
    },
    onSuccess: (saved) => {
      qc.setQueryData(settingsQuery(scope).queryKey, saved);
      // The Agents catalog reads `catalog_tabs` under its own key.
      void qc.invalidateQueries({ queryKey: agentKeys.settings(scope) });
    },
  });
}

export function useSecretWrites() {
  const qc = useQueryClient();
  const scope = useProductScope();
  const refresh = () => qc.invalidateQueries({ queryKey: routerKeys.secrets(scope) });
  return {
    /** POST upserts, so it adds and replaces (legacy secrets-manager: PUT is update-only). */
    add: useMutation({
      gcTime: 0,
      mutationFn: (v: { name: string; value: string }) =>
        apiData<SecretEntry>("/api/secrets", json("POST", v)),
      onSuccess: refresh,
    }),
    remove: useMutation({
      mutationFn: (name: string) =>
        apiFetch(`/api/secrets/${encodeURIComponent(name)}`, { method: "DELETE" }),
      onSuccess: refresh,
    }),
    /** `GET /api/secrets/{name}`: the caller's own value, decrypted. Only on an explicit click. */
    read: useMutation({
      gcTime: 0,
      mutationFn: async (name: string) =>
        (await apiData<{ name: string; value: string }>(`/api/secrets/${encodeURIComponent(name)}`))
          .value,
    }),
  };
}
