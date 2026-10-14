// Ported from nasiko-ui-lab@1b99bdda common/src/features/mcp/api.ts. Local edits: every key is scoped,
// `productKey(scope, "mcp", …the lab's parts)` (`mcpKeys.*` take the scope first and `mcpKeys.all(scope)` is the
// feature prefix; `connectorsQuery`, `toolkitsQuery` and `connectorQuery` take it too); the arrow-function hooks are
// function declarations that read the scope from `useProductScope()`; `refreshAll` takes the scope; `json()` omits
// an absent body (exactOptionalPropertyTypes); Prettier formatting.
/**
 * MCP queries and mutations (plans/feat-mcp.md). Every read and write is under `['mcp', …]`, and every write
 * invalidates it all: a connect changes the catalog, the server's page and each agent's MCP tab at once.
 * Key-bearing writes (credentials, register with secrets) have `gcTime: 0` so the key isn't kept in the cache.
 */
import { useMutation, useQuery, useQueryClient, type QueryClient } from "@tanstack/react-query";
import { createLimiter } from "../observability/limiter";
import type { Scope } from "@nasiko/data-access";
import {
  apiData,
  apiFetch,
  productKey,
  productPrefix,
  useProductScope,
  withQuery,
} from "@nasiko/data-access/product";
import { isBuilding, rulesWith } from "./logic";
import {
  agentConnectorsSchema,
  agentToolsSchema,
  authorizeSchema,
  buildLogsSchema,
  buildStatusSchema,
  connectOutcomeSchema,
  connectorDetailSchema,
  connectorListSchema,
  connectorDtoSchema,
  consumersSchema,
  credentialResultSchema,
  credentialStatusSchema,
  oauthStatusSchema,
  probeSchema,
  shareTargetsSchema,
  sharesSchema,
  toolkitListSchema,
  uploadResultSchema,
  type AgentTool,
  type ConnectOutcome,
  type ConnectorDto,
  type AgentConnectors,
  type AgentTools,
  type Authorize,
  type BuildLogs,
  type BuildState,
  type ConnectorDetail,
  type ConnectorList,
  type Consumers,
  type CredentialResult,
  type CredentialStatus,
  type OauthStatus,
  type Probe,
  type ShareTargets,
  type Shares,
  type ToolkitList,
  type UploadResult,
  type Stance,
} from "./types";
import {
  BUILD_POLL_MS,
  LIST_STALE_MS,
  LOG_TAIL,
  SHARE_SEARCH_MIN,
  TOOL_READ_CONCURRENCY,
} from "./tuning";

const enc = encodeURIComponent;
const C = (id: string) => `/api/mcp/connectors/${enc(id)}`;
const A = (agentId: string) => `/api/mcp/agents/${enc(agentId)}`;

/** The feature every MCP key sits under (`mcpKeys.all(scope)` is its prefix). */
export const MCP_FEATURE = "mcp";

const mk = (scope: Scope, ...parts: readonly unknown[]) => productKey(scope, MCP_FEATURE, ...parts);

export const mcpKeys = {
  all: (scope: Scope) => productPrefix(scope, MCP_FEATURE),
  connectors: (scope: Scope) => mk(scope, "connectors"),
  toolkits: (scope: Scope) => mk(scope, "toolkits"),
  connector: (scope: Scope, id: string) => mk(scope, "connector", id),
  credential: (scope: Scope, id: string) => mk(scope, "connector", id, "credential"),
  oauth: (scope: Scope, id: string) => mk(scope, "connector", id, "oauth"),
  shares: (scope: Scope, id: string) => mk(scope, "connector", id, "shares"),
  consumers: (scope: Scope, id: string) => mk(scope, "connector", id, "consumers"),
  buildStatus: (scope: Scope, id: string) => mk(scope, "connector", id, "build"),
  logs: (scope: Scope, id: string) => mk(scope, "connector", id, "logs"),
  shareTargets: (scope: Scope, q: string) => mk(scope, "share-targets", q),
  agent: (scope: Scope, agentId: string) => mk(scope, "agent", agentId),
  agentTools: (scope: Scope, agentId: string, connectorId: string) =>
    mk(scope, "agent", agentId, "tools", connectorId),
};

const json = (method: string, body?: unknown): RequestInit => ({
  method,
  headers: { "Content-Type": "application/json" },
  ...(body === undefined ? {} : { body: JSON.stringify(body) }),
});

export const connectorsQuery = (scope: Scope) => ({
  queryKey: mcpKeys.connectors(scope),
  queryFn: ({ signal }: { signal: AbortSignal }) =>
    apiData<ConnectorList>("/api/mcp/connectors", { signal, schema: connectorListSchema }),
  staleTime: LIST_STALE_MS,
});

/** The catalog's servers; refetched every 5 s while an upload of theirs builds. */
export function useConnectors() {
  const scope = useProductScope();
  return useQuery({
    ...connectorsQuery(scope),
    refetchInterval: (q) => (isBuilding(q.state.data) ? BUILD_POLL_MS : false),
  });
}

export const toolkitsQuery = (scope: Scope) => ({
  queryKey: mcpKeys.toolkits(scope),
  queryFn: ({ signal }: { signal: AbortSignal }) =>
    apiData<ToolkitList>("/api/mcp/composio/toolkits", { signal, schema: toolkitListSchema }),
  staleTime: LIST_STALE_MS,
});
export function useToolkits() {
  const scope = useProductScope();
  return useQuery(toolkitsQuery(scope));
}

export const connectorQuery = (scope: Scope, id: string) => ({
  queryKey: mcpKeys.connector(scope, id),
  queryFn: ({ signal }: { signal: AbortSignal }) =>
    apiData<ConnectorDetail>(C(id), { signal, schema: connectorDetailSchema }),
});

/** A server's page; polled while its build runs. A 404 is "not found or not yours to see", never retried. */
export function useConnector(id: string) {
  const scope = useProductScope();
  return useQuery({
    ...connectorQuery(scope, id),
    refetchInterval: (q) => {
      const s = q.state.data?.build_status;
      return s === "pending" || s === "building" ? BUILD_POLL_MS : false;
    },
  });
}

export function useCredentialStatus(id: string, enabled: boolean) {
  const scope = useProductScope();
  return useQuery({
    queryKey: mcpKeys.credential(scope, id),
    queryFn: ({ signal }) =>
      apiData<CredentialStatus>(`${C(id)}/credential/status`, {
        signal,
        schema: credentialStatusSchema,
      }),
    enabled,
  });
}

export function useOauthStatus(id: string, enabled: boolean) {
  const scope = useProductScope();
  return useQuery({
    queryKey: mcpKeys.oauth(scope, id),
    queryFn: ({ signal }) =>
      apiData<OauthStatus>(`${C(id)}/oauth/status`, { signal, schema: oauthStatusSchema }),
    enabled,
  });
}

export function useShares(id: string, enabled: boolean) {
  const scope = useProductScope();
  return useQuery({
    queryKey: mcpKeys.shares(scope, id),
    queryFn: ({ signal }) => apiData<Shares>(`${C(id)}/grants`, { signal, schema: sharesSchema }),
    enabled,
  });
}

export function useConsumers(id: string, enabled: boolean) {
  const scope = useProductScope();
  return useQuery({
    queryKey: mcpKeys.consumers(scope, id),
    queryFn: ({ signal }) =>
      apiData<Consumers>(`${C(id)}/consumers`, { signal, schema: consumersSchema }),
    enabled,
  });
}

export function useShareTargets(q: string) {
  const scope = useProductScope();
  const term = q.trim();
  return useQuery({
    queryKey: mcpKeys.shareTargets(scope, term),
    queryFn: ({ signal }) =>
      apiData<ShareTargets>(withQuery("/api/mcp/share-targets", { q: term }), {
        signal,
        schema: shareTargetsSchema,
      }),
    enabled: term.length >= SHARE_SEARCH_MIN,
    select: (r) => r.users,
  });
}

export function useBuildStatus(id: string, enabled: boolean, poll: boolean) {
  const scope = useProductScope();
  return useQuery({
    queryKey: mcpKeys.buildStatus(scope, id),
    queryFn: ({ signal }) =>
      apiData<BuildState>(`${C(id)}/build-status`, { signal, schema: buildStatusSchema }),
    enabled,
    refetchInterval: poll ? BUILD_POLL_MS : false,
  });
}

export function useBuildLogs(id: string, enabled: boolean, poll: boolean) {
  const scope = useProductScope();
  return useQuery({
    queryKey: mcpKeys.logs(scope, id),
    queryFn: ({ signal }) =>
      apiFetch<BuildLogs>(withQuery(`${C(id)}/build-logs`, { tail: LOG_TAIL }), {
        signal,
        schema: buildLogsSchema,
      }),
    enabled,
    refetchInterval: poll ? BUILD_POLL_MS : false,
    select: (r) => r.data,
  });
}

export function useAgentConnectors(agentId: string, enabled: boolean) {
  const scope = useProductScope();
  return useQuery({
    queryKey: mcpKeys.agent(scope, agentId),
    queryFn: ({ signal }) =>
      apiData<AgentConnectors>(`${A(agentId)}/connectors`, {
        signal,
        schema: agentConnectorsSchema,
      }),
    enabled,
    select: (r) => r.connectors,
  });
}

// One read per connector until a batch read exists (M-5); this tab's own limiter.
const toolsLimiter = createLimiter(TOOL_READ_CONCURRENCY);

export function useAgentTools(agentId: string, connectorId: string, enabled: boolean) {
  const scope = useProductScope();
  return useQuery({
    queryKey: mcpKeys.agentTools(scope, agentId, connectorId),
    queryFn: ({ signal }) =>
      toolsLimiter(signal, () =>
        apiData<AgentTools>(`${A(agentId)}/connectors/${enc(connectorId)}/tools`, {
          signal,
          schema: agentToolsSchema,
        }),
      ),
    enabled,
    select: (r) => r.tools,
  });
}

const refreshAll = (qc: QueryClient, scope: Scope) =>
  qc.invalidateQueries({ queryKey: mcpKeys.all(scope) });

/** `POST /api/mcp/connect`: the caller decides what an OAuth outcome opens (`oauth.ts`). */
export function useConnect() {
  const qc = useQueryClient();
  const scope = useProductScope();
  return useMutation({
    gcTime: 0,
    mutationFn: (v: { id: string; value?: string }): Promise<ConnectOutcome> =>
      apiData<ConnectOutcome>("/api/mcp/connect", {
        ...json("POST", {
          connector_id: v.id,
          ...(v.value ? { credentials: { value: v.value } } : {}),
        }),
        schema: connectOutcomeSchema,
      }),
    onSuccess: () => refreshAll(qc, scope),
  });
}

export function useDisconnect() {
  const qc = useQueryClient();
  const scope = useProductScope();
  return useMutation({
    mutationFn: (id: string) => apiFetch(`/api/mcp/connections/${enc(id)}`, { method: "DELETE" }),
    onSuccess: () => refreshAll(qc, scope),
  });
}

export function useProbe() {
  return useMutation({
    mutationFn: (url: string) =>
      apiData<Probe>("/api/mcp/connectors/probe", {
        ...json("POST", { url }),
        schema: probeSchema,
      }),
  });
}

/** 201 with the new server's view; it may carry a basic password or OAuth secret, so it isn't kept. */
export function useRegister() {
  const qc = useQueryClient();
  const scope = useProductScope();
  return useMutation({
    gcTime: 0,
    mutationFn: (body: Record<string, string>) =>
      apiData<ConnectorDto>("/api/mcp/connectors", {
        ...json("POST", body),
        schema: connectorDtoSchema,
      }),
    onSuccess: () => refreshAll(qc, scope),
  });
}

/** Uploads can be large: no 30 s deadline (the zip streams to the server's temp file). */
export function useUpload() {
  const qc = useQueryClient();
  const scope = useProductScope();
  return useMutation({
    mutationFn: (
      v:
        | { kind: "zip"; name: string; version: string; file: File }
        | { kind: "github"; name: string; version: string; url: string },
    ) => {
      if (v.kind === "github")
        return apiData<UploadResult>("/api/mcp/connectors/upload-github", {
          ...json("POST", { name: v.name, version_tag: v.version, github_url: v.url }),
          schema: uploadResultSchema,
        });
      const fd = new FormData();
      fd.append("name", v.name);
      fd.append("version_tag", v.version);
      fd.append("file", v.file);
      return apiData<UploadResult>("/api/mcp/connectors/upload", {
        method: "POST",
        body: fd,
        timeout: 0,
        schema: uploadResultSchema,
      });
    },
    onSuccess: () => refreshAll(qc, scope),
  });
}

export function useUpdateConnector(id: string) {
  const qc = useQueryClient();
  const scope = useProductScope();
  return useMutation({
    mutationFn: (patch: Record<string, string | boolean>) =>
      apiData<ConnectorDto>(C(id), { ...json("PATCH", patch), schema: connectorDtoSchema }),
    onSuccess: () => refreshAll(qc, scope),
  });
}

export function useDeleteConnector(id: string) {
  const qc = useQueryClient();
  const scope = useProductScope();
  return useMutation({
    mutationFn: () => apiFetch(C(id), { method: "DELETE" }),
    onSuccess: () => {
      qc.removeQueries({ queryKey: mcpKeys.connector(scope, id) });
      return qc.invalidateQueries({ queryKey: mcpKeys.connectors(scope) });
    },
  });
}

export function useCredentialWrites(id: string) {
  const qc = useQueryClient();
  const scope = useProductScope();
  const refresh = () => refreshAll(qc, scope);
  return {
    save: useMutation({
      gcTime: 0,
      mutationFn: (value: string) =>
        apiData<CredentialResult>(`${C(id)}/credential`, {
          ...json("POST", { value }),
          schema: credentialResultSchema,
        }),
      onSuccess: refresh,
    }),
    remove: useMutation({
      mutationFn: () => apiFetch(`${C(id)}/credential`, { method: "DELETE" }),
      onSuccess: refresh,
    }),
  };
}

export function useOauthWrites(id: string) {
  const qc = useQueryClient();
  const scope = useProductScope();
  return {
    authorize: useMutation({
      mutationFn: () =>
        apiData<Authorize>(`${C(id)}/oauth/authorize`, { method: "POST", schema: authorizeSchema }),
    }),
    revoke: useMutation({
      mutationFn: () => apiFetch(`${C(id)}/oauth/token`, { method: "DELETE" }),
      onSuccess: () => refreshAll(qc, scope),
    }),
  };
}

/** Grant paths are `…/grants/{public|users/{id}|units/{id}}` (EE adds units, `mcp_sharing.rs`). */
export function useGrantWrites(id: string) {
  const qc = useQueryClient();
  const scope = useProductScope();
  const refresh = () => refreshAll(qc, scope);
  const at = (path: string, method: "POST" | "DELETE") =>
    apiFetch(`${C(id)}/grants/${path}`, { method });
  return {
    setPublic: useMutation({
      mutationFn: (on: boolean) => at("public", on ? "POST" : "DELETE"),
      onSuccess: refresh,
    }),
    add: useMutation({
      mutationFn: (v: { kind: "users" | "units"; granteeId: string }) =>
        at(`${v.kind}/${enc(v.granteeId)}`, "POST"),
      onSuccess: refresh,
    }),
    revoke: useMutation({
      mutationFn: (v: { kind: "users" | "units"; granteeId: string }) =>
        at(`${v.kind}/${enc(v.granteeId)}`, "DELETE"),
      onSuccess: refresh,
    }),
  };
}

/** The Enabled switch: shown at once in the agent's cached list, rolled back on failure. */
export function useSetAccess(agentId: string) {
  const qc = useQueryClient();
  const scope = useProductScope();
  const key = mcpKeys.agent(scope, agentId);
  return useMutation({
    mutationFn: (v: { connectorId: string; enabled: boolean }) =>
      apiData(
        `${A(agentId)}/connectors/${enc(v.connectorId)}`,
        json("PUT", { enabled: v.enabled }),
      ),
    onMutate: async (v) => {
      await qc.cancelQueries({ queryKey: key });
      const before = qc.getQueryData<AgentConnectors>(key);
      qc.setQueryData<AgentConnectors>(key, (old) =>
        old
          ? {
              ...old,
              connectors: old.connectors.map((c) =>
                c.connector_id === v.connectorId ? { ...c, enabled: v.enabled } : c,
              ),
            }
          : old,
      );
      return { before };
    },
    onError: (_e, _v, ctx) => {
      if (ctx?.before) qc.setQueryData(key, ctx.before);
    },
    onSettled: () => refreshAll(qc, scope),
  });
}

/**
 * One stance click, saved at once and shown at once (legacy agent page): the tools cache is patched first and rolled
 * back on failure. The body re-sends the connector's whole rule set (`rulesWith`).
 */
export function useSetStance(agentId: string, connectorId: string) {
  const qc = useQueryClient();
  const scope = useProductScope();
  const key = mcpKeys.agentTools(scope, agentId, connectorId);
  type Cached = { tools: AgentTool[] };
  return useMutation({
    mutationFn: (v: { tool: string; stance: Stance; tools: readonly AgentTool[] }) =>
      apiData(`${A(agentId)}/tools`, {
        ...json("PUT", { rules: rulesWith(connectorId, v.tools, v.tool, v.stance) }),
      }),
    onMutate: async (v) => {
      await qc.cancelQueries({ queryKey: key });
      const before = qc.getQueryData<Cached>(key);
      qc.setQueryData<Cached>(key, (old) =>
        old
          ? {
              ...old,
              tools: old.tools.map((t) => (t.name === v.tool ? { ...t, stance: v.stance } : t)),
            }
          : old,
      );
      return { before };
    },
    onError: (_e, _v, ctx) => {
      if (ctx?.before) qc.setQueryData(key, ctx.before);
    },
    onSettled: () => qc.invalidateQueries({ queryKey: mcpKeys.consumers(scope, connectorId) }),
  });
}
